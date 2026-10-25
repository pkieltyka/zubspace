// Measures a client's real frame cadence - how many requestAnimationFrame callbacks the page actually
// runs - so authored-zone startup cost is judged by a direct witness instead of a derived count.
//
//   make bench-frame-cadence                        # chaos-zone, one client
//   PROBE_ARENA=chaos-zone make bench-frame-cadence
//   PROBE_PEER=1 PROBE_ARENA=chaos-zone make bench-frame-cadence
//
// A second client (`PROBE_PEER=1`) joins the same zone first and is used to time a DevTools
// `Runtime.evaluate` round trip, because a page that is too busy to draw is also too busy to answer
// commands. Prints one JSON line: `CADENCE {"arena":..., "samples":[fps,fps],"latency":[ms,...]}`.
// The static server on 7000 and a game server on 9001 must already be running for the arena probed.

import {
  openChromeClient,
  startChrome,
} from "./combat_replay.mjs";
import {
  longTaskWitnessInstallScript,
  longTaskWitnessReadScript,
} from "./long_task_witness.mjs";

const arena = process.env.PROBE_ARENA || "chaos-zone";
const basePort = Number(process.env.PROBE_PORT || 9831);
const withPeer = process.env.PROBE_PEER === "1";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// This probe has to load with the QA trace flag ON, which is not what anyone would call the shipped client, and
// that is a real limitation of the harness rather than a preference. Measured against a local pair:
// The default is the shipped configuration: no trace flag, no metrics flag, and the run still reaches gameplay.
// That was not true until the always-on lifecycle channel existed. Without it an unflagged page published zero
// `uiMetrics` keys, so the harness waits behind `enterShip` could only be satisfied by turning the recorder on -
// which meant every cadence figure this probe printed described an instrumented client, and a probe that defaulted
// the flag off spent 30 minutes in 60 doomed attempts, which is indistinguishable from a hang.
//
// Readiness now arrives on `window.__zubspaceLifecycle` regardless of any flag, published only when it changes, so
// the default measurement is the configuration players actually get. `PROBE_QA_TRACE=1` opts the recorder back in for
// the express purpose of measuring what it costs - that is what `make bench-capture-overhead` alternates.
const qaTrace = process.env.PROBE_QA_TRACE === "1";

function clientUrl(name) {
  return (
    "http://127.0.0.1:7000/index.html?rtc=http%3A%2F%2F127.0.0.1%3A9001%2Fv1%2Frtc%2Foffer" +
    `&password=secret&arena=${arena}${qaTrace ? "&trace=1" : ""}&name=${name}`
  );
}

// The harness serialises `Runtime.evaluate` by value, so a promise-returning expression arrives as an
// empty object. Count frames in the page and read the counter back after the window elapses.
const COUNTER_START = `(() => {
  window.__cadence = { frames: 0, startedAt: performance.now(), running: true };
  const step = () => {
    if (!window.__cadence || !window.__cadence.running) { return; }
    window.__cadence.frames += 1;
    requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
  return "started";
})()`;
const COUNTER_READ = `(() => {
  const counter = window.__cadence;
  counter.running = false;
  return Math.round((counter.frames * 1000) / (performance.now() - counter.startedAt));
})()`;

// A long task attributes a stall to one synchronous main-thread task instead of to the frame budget in
// general. Shared with the gates through `long_task_witness.mjs`, with the frame-gap sampler switched
// off: this probe already drives its own rAF counter, and a second animation callback would be part of
// what it measures.
const LONGTASK_OBSERVE = longTaskWitnessInstallScript({ frameSampler: false });
const LONGTASK_READ = longTaskWitnessReadScript({ longTaskLimit: 6 });

// The readiness stages retry patiently by design, but a caller that expects a stage to fail - the recorder-off arm
// of `make bench-capture-overhead`, for one - must not pay 60 x 30 s to learn what it already suspects.
const tolerantAttempts = Number(process.env.PROBE_ENTER_ATTEMPTS || 60);

async function tolerant(label, run, attempts = tolerantAttempts) {
  let lastError = null;
  const startedAt = Date.now();
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      return await run();
    } catch (error) {
      lastError = error;
      // The original body was a bare `catch {}`, which is why this probe could spend ten minutes in one stage and
      // print nothing at all: every reason was discarded, and 60 attempts times a stage that fails slowly is a
      // hang as far as an operator can tell. Report the reason, and report that the stage is still failing at all.
      if (attempt === 0 || (attempt + 1) % 10 === 0) {
        console.error(`cadence: ${label} attempt ${attempt + 1}/${attempts} after `
          + `${Math.round((Date.now() - startedAt) / 1000)}s: ${String(error?.message ?? error).slice(0, 160)}`);
      }
      await sleep(250);
    }
  }
  throw new Error(`${label} unreadable after ${attempts} attempts over `
    + `${Math.round((Date.now() - startedAt) / 1000)}s; last reason: ${String(lastError?.message ?? lastError)
      .slice(0, 220)}`);
}

const chrome = startChrome(basePort, `cadence-${arena}`);
let peer = null;
if (withPeer) {
  const peerChrome = startChrome(basePort + 1, `cadence-${arena}-peer`);
  peer = await openChromeClient(peerChrome, clientUrl(`cadencepeer`));
  await tolerant("peer enterShip", () => peer.enterShip());
}
const client = await openChromeClient(chrome, clientUrl(`cadenceprobe`));
await tolerant("longtask observer", () => client.evaluate(LONGTASK_OBSERVE));
await tolerant("enterShip", () => client.enterShip());

const samples = [];
const latency = [];
for (let window = 0; window < 2; window += 1) {
  await tolerant("counter start", () => client.evaluate(COUNTER_START));
  await sleep(5_500);
  samples.push(await tolerant("counter read", () => client.evaluate(COUNTER_READ)));
  if (peer) {
    const startedAt = Date.now();
    await tolerant("peer evaluate", () => peer.evaluate("1+1"));
    latency.push(Date.now() - startedAt);
  }
}

const metrics = await tolerant("metrics", () => client.metricsState());
const longtasksEarly = JSON.parse(await tolerant("longtasks early", () => client.evaluate(LONGTASK_READ)));
// A duration that grows between the two reads means the entry is an unfinished task still being
// attributed, not a stall that actually completed. Compare `early` and `late` for the same `start`.
await sleep(25_000);
const longtasksLate = JSON.parse(await tolerant("longtasks late", () => client.evaluate(LONGTASK_READ)));
console.log("CADENCE", JSON.stringify({
  // Report the configuration, so a number cannot be read as the shipped client when it was taken with the
  // QA rings installed.
  qaTrace,
  arena,
  samples,
  latency,
  longtasks: { early: longtasksEarly, late: longtasksLate },
  viewport: [metrics.uiMetrics?.screenWidth, metrics.uiMetrics?.screenHeight],
  position: [metrics.uiMetrics?.playerX, metrics.uiMetrics?.playerY],
}));

await client.close().catch(() => {});
if (peer) { await peer.close().catch(() => {}); }
process.exit(0);
