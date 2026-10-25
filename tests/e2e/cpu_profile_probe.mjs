// Names what a page's main thread is actually doing by CPU-profiling it from outside, so no game code has
// to be instrumented. Attaches to a fresh tab, starts the sampling profiler before the page boots, waits
// through the phase of interest, and prints the hottest self-time frames.
//
//   PROBE_ARENA=chaos-zone make bench-client-cpu
//   PROBE_SECONDS=70 PROBE_SAMPLE_US=250 PROBE_ARENA=chaos-zone make bench-client-cpu
//
// Needs the static server on 7000 and a game server on 9001 for the arena probed. This is how the
// authored-zone boot stall was attributed: the WASM name section survives the release build, so the frames
// come back as real Rust symbols. These probes are diagnostics, never acceptance; the default arena
// moved to chaos-zone when the retired zone - the one with the heaviest LVZ set - left the build.

import { startChrome, waitForChrome } from "./combat_replay.mjs";

const arena = process.env.PROBE_ARENA || "chaos-zone";
const port = Number(process.env.PROBE_PORT || 9881);
const sampleUs = Number(process.env.PROBE_SAMPLE_US || 250);
const profileSeconds = Number(process.env.PROBE_SECONDS || 60);
// A start delay lets the profile cover a known phase - e.g. PROBE_START_DELAY_MS=8000 PROBE_SECONDS=6
// profiles a boot stall that starts a few seconds after load, without the idle frames around it.
const startDelayMs = Number(process.env.PROBE_START_DELAY_MS || 0);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Default off: a profile of the instrumented client is not a profile of the shipped one. Previously this
// probe appended trace=1 unconditionally, so retained hot-frame reports carry the QA ring recorder's own cost and
// were never labelled as instrumented-build numbers.
const qaTrace = process.env.PROBE_QA_TRACE === "1";

const url =
  "http://127.0.0.1:7000/index.html?rtc=http%3A%2F%2F127.0.0.1%3A9001%2Fv1%2Frtc%2Foffer" +
  `&password=secret&arena=${arena}${qaTrace ? "&trace=1" : ""}&name=profileprobe`;

const chrome = startChrome(port, `profile-${arena}`);
await waitForChrome(chrome);

const { webSocketDebuggerUrl } = await (
  await fetch(`http://127.0.0.1:${port}/json/version`)
).json();
const ws = new WebSocket(webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve, { once: true });
  ws.addEventListener("error", () => reject(new Error("devtools connect failed")), { once: true });
});

const pending = new Map();
let nextId = 0;
function send(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
}
ws.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  const waiting = pending.get(message.id);
  if (!waiting) return;
  pending.delete(message.id);
  if (message.error) waiting.reject(new Error(`${message.id}: ${JSON.stringify(message.error)}`));
  else waiting.resolve(message.result ?? {});
});

// A background tab is timer-throttled, which would distort both the profile and the RTC handshake.
const { targetId } = await send("Target.createTarget", { url });
const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
await send("Profiler.enable", {}, sessionId);
await send("Profiler.setSamplingInterval", { interval: sampleUs }, sessionId);
if (startDelayMs > 0) {
  await sleep(startDelayMs);
}
await send("Profiler.start", {}, sessionId);
console.log(`PROFILING ${arena} for ${profileSeconds}s`);
await sleep(profileSeconds * 1000);
const { profile } = await send("Profiler.stop", {}, sessionId);

// Self time: the protocol already attributes a sample to its deepest frame as that node's own hitCount.
const hits = new Map();
let total = 0;
for (const node of profile.nodes) {
  const count = node.hitCount ?? 0;
  if (!count) continue;
  total += count;
  const frame = node.callFrame;
  const script = (frame.url || "").split("/").pop();
  const key = `${frame.functionName || "(anonymous)"} @${script}:${frame.lineNumber}`;
  hits.set(key, (hits.get(key) ?? 0) + count);
}
// Inclusive time is computed by walking each sample's ancestor chain, not just its leaf. A merge function
// that spends 87% of boot inside one child looks like 0% self time otherwise.
// The protocol links parents to children only, so the ancestor map has to be derived. Without it an
// "inclusive" ranking silently equals the self ranking.
const parents = new Map();
for (const node of profile.nodes) {
  for (const child of node.children ?? []) {
    parents.set(child, node.id);
  }
}
const byId = new Map(profile.nodes.map((node) => [node.id, node]));
const totalHits = new Map();
for (const node of profile.nodes) {
  const count = node.hitCount ?? 0;
  if (!count) continue;
  const seen = new Set();
  let cursorId = node.id;
  while (cursorId !== undefined && !seen.has(cursorId)) {
    seen.add(cursorId);
    const frame = byId.get(cursorId)?.callFrame;
    if (frame) {
      const script = (frame.url || "").split("/").pop();
      const key = `${frame.functionName || "(anonymous)"} @${script}:${frame.lineNumber}`;
      totalHits.set(key, (totalHits.get(key) ?? 0) + count);
    }
    cursorId = parents.get(cursorId);
  }
}
const rank = (map) => [...map.entries()].sort((a, b) => b[1] - a[1]);
const ranked = rank(hits);
const rankedTotal = rank(totalHits);
console.log(
  "PROFILE",
  JSON.stringify({
    arena,
    nodes: profile.nodes.length,
    samples: total,
    windowSeconds: Math.round((profile.endTime - profile.startTime) / 1e6),
    top: ranked.slice(0, 14).map(([name, count]) => `${Math.round((100 * count) / total)}% ${name}`),
  }),
);
console.log(`TOP self of ${total} samples (${Math.round((profile.endTime - profile.startTime) / 1e6)}s window)`);
for (const [name, count] of ranked.slice(0, 10)) {
  console.log(`  ${String(Math.round((100 * count) / total)).padStart(3)}% ${name}`);
}
console.log("ALL total (inclusive, ancestors counted)");
for (const [name, count] of rankedTotal.slice(0, 22)) {
  console.log(`  ${String(Math.round((100 * count) / total)).padStart(3)}% ${name}`);
}

await send("Target.closeTarget", { targetId }).catch(() => {});
process.kill(-chrome.child.pid, "SIGKILL");
process.exit(0);
