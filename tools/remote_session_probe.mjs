#!/usr/bin/env node
// Play a scripted session on a deployed Zubspace site and measure its traffic.
//
//   node tools/remote_session_probe.mjs https://dev.zubspace.gg/ [seconds]
//
// Joins with headless Chrome over the ordinary URL, then repeats a fixed
// 10-second pattern until the time is up (default 120 s): thrust and turn,
// held bullets, two bombs, one mine, then coasting. Chrome's own WebRTC stats
// are read when the pattern starts and when it ends, so the result counts real
// UDP datagrams and bytes per direction (candidate pair) next to the
// DataChannel messages and bytes per channel. Prints one JSON object with the
// window's start and end times for matching server metrics and logs. This is a
// measurement, not an acceptance gate.
import { acknowledgeControlsWelcome } from "./browser_controls_welcome.mjs";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const url = process.argv[2];
const seconds = Number(process.argv[3] ?? 120);
if (!url || !(seconds > 0)) {
  console.error("usage: node tools/remote_session_probe.mjs <site-url> [seconds]");
  process.exit(2);
}
const chromeBin = process.env.CHROME_BIN || "google-chrome";
const port = 9400 + Math.floor(Math.random() * 400);
const profile = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-session-probe-"));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Keep every RTCPeerConnection the page creates so its stats can be read.
const capturePeerConnections = `(() => {
  const Native = window.RTCPeerConnection;
  if (!Native) return;
  const created = [];
  window.__probePeerConnections = created;
  function Captured(...args) {
    const connection = new Native(...args);
    created.push(connection);
    return connection;
  }
  Captured.prototype = Native.prototype;
  Object.setPrototypeOf(Captured, Native);
  window.RTCPeerConnection = Captured;
})();`;

// Totals from every open peer connection: the selected candidate pair carries
// the UDP datagrams; data-channel entries carry application messages.
const readStats = `(async () => {
  const totals = { pair: null, channels: {} };
  for (const connection of window.__probePeerConnections || []) {
    if (connection.connectionState === "closed") continue;
    const report = await connection.getStats();
    let selectedPairId = null;
    report.forEach((stat) => {
      if (stat.type === "transport" && stat.selectedCandidatePairId) selectedPairId = stat.selectedCandidatePairId;
    });
    report.forEach((stat) => {
      if (stat.type === "candidate-pair" && (stat.id === selectedPairId || (!selectedPairId && stat.nominated && stat.state === "succeeded"))) {
        totals.pair = {
          packetsSent: stat.packetsSent ?? 0, packetsReceived: stat.packetsReceived ?? 0,
          bytesSent: stat.bytesSent ?? 0, bytesReceived: stat.bytesReceived ?? 0,
          rttMs: stat.currentRoundTripTime == null ? null : stat.currentRoundTripTime * 1000,
        };
      }
      if (stat.type === "data-channel") {
        const channel = totals.channels[stat.label] ??= { messagesSent: 0, bytesSent: 0, messagesReceived: 0, bytesReceived: 0 };
        channel.messagesSent += stat.messagesSent ?? 0;
        channel.bytesSent += stat.bytesSent ?? 0;
        channel.messagesReceived += stat.messagesReceived ?? 0;
        channel.bytesReceived += stat.bytesReceived ?? 0;
      }
    });
  }
  return totals;
})()`;

const KEYS = {
  ArrowUp: { code: "ArrowUp", keyCode: 38 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
  Control: { code: "ControlLeft", keyCode: 17, modifier: 2 },
  Shift: { code: "ShiftLeft", keyCode: 16, modifier: 8 },
  Tab: { code: "Tab", keyCode: 9 },
};

const chrome = spawn(chromeBin, [
  "--headless=new", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist",
  "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--mute-audio",
  "--autoplay-policy=no-user-gesture-required", "--window-size=1440,1000",
  `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "about:blank",
], { stdio: "ignore", detached: true });

const result = { url, seconds, console: [], exceptions: [] };
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
      if (result.console.length < 20) result.console.push(`${message.params.type}: ${text}`.slice(0, 300));
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
  const held = new Set();
  const modifiers = () => [...held].reduce((bits, name) => bits | (KEYS[name].modifier ?? 0), 0);
  const keyDown = async (name) => {
    held.add(name);
    const { code, keyCode } = KEYS[name];
    await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: name, code, windowsVirtualKeyCode: keyCode, modifiers: modifiers() });
  };
  const keyUp = async (name) => {
    held.delete(name);
    const { code, keyCode } = KEYS[name];
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code, windowsVirtualKeyCode: keyCode, modifiers: modifiers() });
  };
  const tap = async (name, ms = 60) => {
    await keyDown(name);
    await sleep(ms);
    await keyUp(name);
  };
  const releaseAll = async () => {
    for (const name of [...held]) await keyUp(name).catch(() => {});
  };

  await send("Runtime.enable");
  await send("Page.enable");
  await send("Page.addScriptToEvaluateOnNewDocument", { source: capturePeerConnections });
  await send("Page.navigate", { url });
  await waitFor("the join form", 'document.querySelector("#player-name-form")?.classList.contains("visible")', 30_000);
  result.buildLabel = await evaluate('document.querySelector("#menu-build")?.textContent');
  result.name = `probe${Math.floor(Math.random() * 1000)}`;
  await send("Input.insertText", { text: result.name });
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
  await acknowledgeControlsWelcome(send, evaluate);
  await waitFor("an authoritative gameplay life", "window.__zubspaceLifecycle?.inGameplay === true", 30_000);
  // Let the join settle so the window measures steady play, not the join burst.
  await sleep(3000);

  const before = await evaluate(readStats);
  if (!before?.pair) throw new Error("no selected candidate pair in the page's WebRTC stats");
  const startedAt = Date.now();
  result.windowStart = new Date(startedAt).toISOString();
  let cycles = 0;
  let deadSamples = 0;
  while (Date.now() - startedAt < seconds * 1000) {
    // 0-3 s: thrust while turning one way, then the other.
    await keyDown("ArrowUp");
    await keyDown(cycles % 2 ? "ArrowLeft" : "ArrowRight");
    await sleep(1500);
    await keyUp(cycles % 2 ? "ArrowLeft" : "ArrowRight");
    await keyDown(cycles % 2 ? "ArrowRight" : "ArrowLeft");
    await sleep(1500);
    await keyUp(cycles % 2 ? "ArrowRight" : "ArrowLeft");
    await keyUp("ArrowUp");
    // 3-5 s: held bullets.
    await keyDown("Control");
    await sleep(2000);
    await keyUp("Control");
    // 5-6 s: two bombs.
    await tap("Tab");
    await sleep(440);
    await tap("Tab");
    await sleep(440);
    // 6-7 s: one mine.
    await keyDown("Shift");
    await tap("Tab");
    await keyUp("Shift");
    await sleep(880);
    // 7-10 s: coast.
    if (!(await evaluate("window.__zubspaceLifecycle?.inGameplay === true"))) deadSamples++;
    await sleep(3000);
    cycles++;
  }
  await releaseAll();
  const after = await evaluate(readStats);
  const elapsed = (Date.now() - startedAt) / 1000;
  result.windowEnd = new Date().toISOString();
  result.elapsedSeconds = Number(elapsed.toFixed(1));
  result.cycles = cycles;
  result.cyclesWhileDead = deadSamples;

  const rate = (value) => Number((value / elapsed).toFixed(1));
  const pairDelta = (field) => (after.pair?.[field] ?? 0) - before.pair[field];
  result.udp = {
    up: { datagramsPerSecond: rate(pairDelta("packetsSent")), bytesPerSecond: rate(pairDelta("bytesSent")) },
    down: { datagramsPerSecond: rate(pairDelta("packetsReceived")), bytesPerSecond: rate(pairDelta("bytesReceived")) },
    rttMs: after.pair?.rttMs ?? null,
  };
  result.channels = {};
  let messagesUp = 0, messagesDown = 0, payloadUp = 0, payloadDown = 0;
  for (const [label, end] of Object.entries(after.channels)) {
    const start = before.channels[label] ?? { messagesSent: 0, bytesSent: 0, messagesReceived: 0, bytesReceived: 0 };
    const sentMessages = end.messagesSent - start.messagesSent;
    const sentBytes = end.bytesSent - start.bytesSent;
    const receivedMessages = end.messagesReceived - start.messagesReceived;
    const receivedBytes = end.bytesReceived - start.bytesReceived;
    messagesUp += sentMessages; payloadUp += sentBytes;
    messagesDown += receivedMessages; payloadDown += receivedBytes;
    result.channels[label] = {
      up: { messagesPerSecond: rate(sentMessages), bytesPerSecond: rate(sentBytes) },
      down: { messagesPerSecond: rate(receivedMessages), bytesPerSecond: rate(receivedBytes) },
    };
  }
  // Candidate-pair bytes exclude IP/UDP headers but include DTLS and SCTP
  // framing, SCTP control chunks (SACKs) and ICE consent checks.
  const ratio = (a, b) => (b > 0 ? Number((a / b).toFixed(2)) : null);
  result.summary = {
    messagesPerDatagramUp: ratio(messagesUp, pairDelta("packetsSent")),
    messagesPerDatagramDown: ratio(messagesDown, pairDelta("packetsReceived")),
    framingBytesPerDatagramUp: ratio(pairDelta("bytesSent") - payloadUp, pairDelta("packetsSent")),
    framingBytesPerDatagramDown: ratio(pairDelta("bytesReceived") - payloadDown, pairDelta("packetsReceived")),
    payloadShareUp: ratio(payloadUp, pairDelta("bytesSent")),
    payloadShareDown: ratio(payloadDown, pairDelta("bytesReceived")),
  };
  result.ok = true;
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
