#!/usr/bin/env node
// Join a deployed Zubspace site with headless Chrome and report what happened.
//
//   node tools/remote_join_probe.mjs https://dev.zubspace.gg/ [seconds-in-game]
//
// Loads the page, enters a name, waits for an authoritative gameplay life over
// the real Cloudflare and WebRTC path, stays for the given time (default 15),
// then reads the same-origin /v1/status. Prints one JSON object and exits nonzero
// unless the browser is still in gameplay at the end. A screenshot lands in
// .debug/. No window opens; this is a deployment check, not an acceptance gate.
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const url = process.argv[2];
const holdSeconds = Number(process.argv[3] ?? 15);
if (!url) {
  console.error("usage: node tools/remote_join_probe.mjs <site-url> [seconds-in-game]");
  process.exit(2);
}
const chromeBin = process.env.CHROME_BIN || "google-chrome";
const port = 9400 + Math.floor(Math.random() * 400);
const profile = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-remote-probe-"));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const chrome = spawn(chromeBin, [
  "--headless=new", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist",
  "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--mute-audio",
  "--autoplay-policy=no-user-gesture-required", "--window-size=1440,1000",
  `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "about:blank",
], { stdio: "ignore", detached: true });

const result = { url, started: new Date().toISOString(), console: [], exceptions: [] };
let ws;
try {
  let target;
  for (let attempt = 0; attempt < 100 && !target; attempt++) {
    try {
      target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: "PUT" })).json();
    } catch {
      await sleep(100);
    }
  }
  if (!target) throw new Error("Chrome DevTools did not start");
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve) => ws.addEventListener("open", resolve, { once: true }));
  let nextId = 0;
  const pending = new Map();
  ws.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result);
    } else if (message.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(message.params.type)) {
      const text = (message.params.args || []).map((arg) => arg.value ?? arg.description ?? "").join(" ");
      result.console.push(`${message.params.type}: ${text}`.slice(0, 300));
    } else if (message.method === "Runtime.exceptionThrown") {
      result.exceptions.push(message.params.exceptionDetails?.exception?.description?.slice(0, 300) ?? "exception");
    }
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) =>
    (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.value;
  const waitFor = async (label, expression, timeoutMs) => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (await evaluate(expression).catch(() => false)) return;
      await sleep(250);
    }
    throw new Error(`timed out waiting for ${label}`);
  };
  const key = (type) => send("Input.dispatchKeyEvent", { type, key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });

  await send("Runtime.enable");
  await send("Page.enable");
  const loadStarted = Date.now();
  await send("Page.navigate", { url });
  await waitFor("the join form", 'document.querySelector("#player-name-form")?.classList.contains("visible")', 30_000);
  result.joinFormMs = Date.now() - loadStarted;
  result.buildLabel = await evaluate('document.querySelector("#menu-build")?.textContent');
  await send("Input.insertText", { text: `probe${Math.floor(Math.random() * 1000)}` });
  await key("keyDown");
  await key("keyUp");
  const joinStarted = Date.now();
  await waitFor("an authoritative gameplay life", "window.__zubspaceLifecycle?.inGameplay === true", 30_000);
  result.joinToGameplayMs = Date.now() - joinStarted;
  await sleep(holdSeconds * 1000);
  // Bots may have just killed the probe; a respawn countdown is still a live
  // session, so allow it to come back before judging.
  const respawnStarted = Date.now();
  for (let i = 0; i < 40 && !(await evaluate("window.__zubspaceLifecycle?.inGameplay === true")); i++) await sleep(250);
  result.stillInGameplay = await evaluate("window.__zubspaceLifecycle?.inGameplay === true");
  if (Date.now() - respawnStarted > 300) result.waitedForRespawnMs = Date.now() - respawnStarted;
  // Builds before the /v1 prefix answer only /status.
  result.status = await evaluate(`fetch(new URL("v1/status", location.href))
    .then((r) => r.ok ? r : fetch(new URL("status", location.href))).then((r) => r.json()).then((s) => ({
    total: s.total_players ?? s.total ?? s.num_players ?? ((s.players ?? 0) + (s.bots ?? 0)),
    rtc_sessions: s.rtc_sessions, uptime_seconds: s.uptime ?? s.uptime_seconds }))`);
  result.zoneStatus = await evaluate(`(async () => {
    try {
      const response = await fetch(new URL("v1/zones", location.href));
      if (!response.ok) return null;
      const catalog = await response.json();
      const zone = catalog.zones?.find((zone) => zone.id === window.__zubspaceArena);
      if (!zone || typeof zone.total_players !== "number") return null;
      const status = await fetch(new URL("v1/zones/" + zone.id + "/status", location.href)).then((r) => r.json());
      return { zone: zone.id, total: zone.total_players, snapshot_acked: status.snapshot_acked_clients };
    } catch { return null; }
  })()`);
  const shot = await send("Page.captureScreenshot", { format: "png" });
  fs.mkdirSync(".debug", { recursive: true });
  result.screenshot = path.resolve(".debug", `remote-probe-${new URL(url).hostname}.png`);
  fs.writeFileSync(result.screenshot, Buffer.from(shot.data, "base64"));
  result.ok = result.stillInGameplay === true;
} catch (error) {
  result.ok = false;
  result.error = error.message;
} finally {
  ws?.close();
  try { process.kill(-chrome.pid, "SIGTERM"); } catch {}
  await sleep(300);
  fs.rmSync(profile, { recursive: true, force: true });
}
console.log(JSON.stringify(result, null, 1));
process.exit(result.ok ? 0 : 1);
