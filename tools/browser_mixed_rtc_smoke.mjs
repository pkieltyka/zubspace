#!/usr/bin/env node
import { fetchGameStatusResponse } from "./game_status.mjs";

import fs from "node:fs";
import { hudLineCountMatchesMode, smokeGameplayReady, smokeLiveStateExpression } from "./smoke_ui_contract.mjs";
import path from "node:path";
import { collectArtifactProvenance } from "../tests/e2e/artifact_provenance.mjs";
import { artifactFileManifest } from "../tests/e2e/artifact_paths.mjs";
import { spawn } from "node:child_process";
import { browserLaunchEnvironment } from "./e2e_gate_hygiene.mjs";

import {
  assertCoastingPositionRelay,
  assertCoastingPositionStream,
  createSession,
  deleteSession,
  driveAndRelease as driveFirefoxAndRelease,
  enterShip as enterFirefoxShip,
  execute as executeFirefox,
  freePort,
  openClient as openFirefoxClient,
  pageState as firefoxPageState,
  repoRoot,
  sampleMs,
  screenshot as screenshotFirefox,
  sleep,
  startGeckodriver,
  startRealServer,
  startStaticServer,
  stopProcess,
  summarizePacketLog,
  timelineRate,
  waitForGeckodriver,
  waitForServerListening,
} from "./browser_firefox_rtc_smoke.mjs";
import { parseServerDamageEvents } from "./server_events.mjs";
import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
} from "./rtc_gate_audit.mjs";

const chromeBin = process.env.CHROME_BIN || "google-chrome";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const serverBin = "target/debug/zubspace_server";
const smokeTickRate = process.env.ZUBSPACE_SMOKE_TICK_RATE || "60";
const smokeBotCount = Number(process.env.ZUBSPACE_SMOKE_BOT_COUNT || "0");
const smokeCombat = process.env.ZUBSPACE_SMOKE_COMBAT === "1";
const serverBotIdBase = 0x0300;
const runStartedAt = new Date().toISOString();

function startChrome(port) {
  const userDataDir = `/tmp/zubspace-mixed-rtc-chrome-${port}-${process.pid}`;
  const child = spawn(chromeBin, [
    "--headless=new",
    "--enable-unsafe-swiftshader",
    "--use-gl=angle",
    "--use-angle=swiftshader",
    "--password-store=basic",
    "--disable-extensions",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--autoplay-policy=no-user-gesture-required",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${userDataDir}`,
    "about:blank",
  ], {
    env: browserLaunchEnvironment(process.env, "mixed smoke Chrome"),
  });
  child.stdout.on("data", () => {});
  child.stderr.on("data", () => {});
  return { child, userDataDir, port };
}

async function waitForChrome(chrome) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (chrome.child.exitCode !== null || chrome.child.signalCode !== null) {
      throw new Error("Chrome exited before DevTools became available");
    }
    try {
      const response = await fetch(`http://127.0.0.1:${chrome.port}/json/version`);
      if (response.ok) return;
    } catch {
      // Chrome is still starting.
    }
    await sleep(100);
  }
  throw new Error("timed out waiting for Chrome DevTools");
}

async function openChromeClient(chromePort, pageUrl) {
  const target = await (
    await fetch(`http://127.0.0.1:${chromePort}/json/new?${encodeURIComponent("about:blank")}`, {
      method: "PUT",
    })
  ).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  let nextId = 1;
  const pending = new Map();
  const events = [];

  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const { resolve, reject, timeout } = pending.get(message.id);
      pending.delete(message.id);
      clearTimeout(timeout);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result || {});
      return;
    }
    if (message.method === "Runtime.consoleAPICalled") {
      const type = message.params.type;
      const text = (message.params.args || [])
        .map((arg) => arg.value ?? arg.description ?? "")
        .join(" ");
      if (["error", "warning", "assert"].includes(type) || text === "fix") {
        events.push({ type, text });
      }
    } else if (message.method === "Runtime.exceptionThrown") {
      events.push({
        type: "exception",
        text: message.params.exceptionDetails?.text || "exception",
      });
    } else if (message.method === "Log.entryAdded") {
      const entry = message.params.entry;
      if (["error", "warning"].includes(entry.level)) {
        events.push({ type: entry.level, text: entry.text });
      }
    }
  };

  ws.onclose = () => {
    for (const request of pending.values()) {
      clearTimeout(request.timeout);
      request.reject(new Error("DevTools WebSocket closed"));
    }
    pending.clear();
  };

  const send = (method, params = {}, timeoutMs = 20_000) => {
    const id = nextId++;
    ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`DevTools command timed out: ${method}`));
      }, timeoutMs);
      pending.set(id, { resolve, reject, timeout });
    });
  };

  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
    setTimeout(() => reject(new Error("DevTools WebSocket timeout")), 5_000);
  });

  await send("Runtime.enable");
  await send("Page.enable");
  await send("Log.enable");
  await send("Emulation.setDeviceMetricsOverride", {
    width: 1280,
    height: 720,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await send("Page.navigate", { url: pageUrl });
  await send("Page.bringToFront").catch(() => {});
  let bootstrapState = null;
  await waitUntil("Chrome WASM page", async () => {
    try {
      const state = await evaluateChrome(send, `(() => ({
        canvas: !!document.querySelector('canvas'),
        hasWasm: performance.getEntriesByType('resource').some((entry) => /zubspace\\.wasm/.test(entry.name)),
        readyState: document.readyState,
        location: window.location.href,
        hasMetrics: !!window.__zubspaceUiMetrics,
        rtcStatus: window.__zubspaceRtcStatusForTest?.() ?? null,
      }))()`);
      bootstrapState = state;
      // UI metrics are exported by the running Rust/WASM render loop. This is
      // a stronger bootstrap signal than a PerformanceResourceTiming entry,
      // which Chromium may omit after a successful no-store WASM fetch.
      return state.canvas && state.hasMetrics;
    } catch (error) {
      // A poll can race the scheduled navigation and lose its execution
      // context. Only a later successful evaluation can satisfy readiness.
      bootstrapState = { evaluationError: error.message };
      return false;
    }
  }, 90_000).catch((error) => {
    throw new Error(`${error.message}: ${JSON.stringify(bootstrapState)}`);
  });
  await evaluateChrome(send, "document.querySelector('canvas')?.focus(); true");
  await waitUntil("Chrome RTC channels", async () => {
    const state = await evaluateChrome(send, smokeLiveStateExpression);
    const messages = state.networkLog.map((entry) => entry.message);
    const labels = state.networkLog
      .filter((entry) => entry.message === "rtc data channel open")
      .map((entry) => entry.detail?.label);
    return (
      messages.includes("rtc answer accepted") &&
      labels.includes("zubspace-game") &&
      labels.includes("zubspace-events") &&
      labels.includes("zubspace-reliable")
    );
  }, 30_000);
  await waitUntil("Chrome loading overlay", async () => {
    return evaluateChrome(
      send,
      `(() => {
        const overlay = document.querySelector('#loading-overlay');
        return !overlay || overlay.classList.contains('hidden');
      })()`,
    );
  }, 30_000);
  await evaluateChrome(send, "document.querySelector('canvas')?.focus(); true");

  return {
    events,
    evaluate(expression) {
      return evaluateChrome(send, expression);
    },
    async enterShip() {
      await evaluateChrome(send, "document.querySelector('canvas')?.focus(); true");
      // Ordinary join already selects a playable ship. Esc/2 retries can
      // cross asynchronous admission and dispatch gameplay Rocket instead.
      await waitUntil("Chrome ordinary join playable", () =>
        evaluateChrome(send, "window.__zubspaceLifecycle?.inGameplay === true"), 60_000);
    },
    async driveAndRelease() {
      await evaluateChrome(send, "document.querySelector('canvas')?.focus(); true");
      await dispatchChromeKey(send, "keyDown", "ArrowUp", "ArrowUp", 38);
      await dispatchChromeKey(send, "keyDown", "ArrowRight", "ArrowRight", 39);
      await sleep(700);
      await dispatchChromeKey(send, "keyUp", "ArrowRight", "ArrowRight", 39).catch(() => {});
      await dispatchChromeKey(send, "keyUp", "ArrowUp", "ArrowUp", 38).catch(() => {});
      return { releasedAt: Date.now() };
    },
    async liveState() {
      return evaluateChrome(send, smokeLiveStateExpression);
    },
    async pageState() {
      return chromePageState(send);
    },
    async screenshot() {
      return (await send("Page.captureScreenshot", { format: "png" })).data;
    },
    async close() {
      await send("Page.close").catch(() => {});
      ws.close();
    },
  };
}

async function dispatchChromeKey(send, type, key, code, keyCode) {
  const params = {
    type,
    key,
    code,
    windowsVirtualKeyCode: keyCode,
    nativeVirtualKeyCode: keyCode,
  };
  if (type === "keyDown" && key.length === 1) {
    params.text = key;
    params.unmodifiedText = key;
  }
  await send("Input.dispatchKeyEvent", params);
}

async function evaluateChrome(send, expression) {
  const result = await send("Runtime.evaluate", {
    returnByValue: true,
    expression,
  });
  return result.result.value;
}

async function chromePageState(send) {
  return evaluateChrome(
    send,
    `(() => ({
      networkLog: window.__zubspaceNetworkLog || [],
      packetLogFirst: window.__zubspacePacketLogFirst || [],
      packetLog: window.__zubspacePacketLog || [],
      uiMetrics: window.__zubspaceUiMetrics || null,
      metricsTrace: window.__zubspaceMetricsTrace || [],
      damageTrace: window.__zubspaceDamageTrace || [],
      currentUrl: window.location.href,
    }))()`,
  );
}

async function readFailureDiagnostic(read) {
  let timer;
  try {
    const value = await Promise.race([
      read(),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("diagnostic read exceeded 5000ms")), 5000); }),
    ]);
    return value ?? { unavailable: true, reason: "client was not created" };
  } catch (error) {
    return { unavailable: true, reason: error.message };
  } finally {
    clearTimeout(timer);
  }
}

async function waitUntil(description, predicate, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${description}`);
}

function summarizeClient(state) {
  return summarizePacketLog(mergedPacketLog(state));
}

function serverDamageMatchesWeapon(event, { shooterId, damagedId, weapon }) {
  return (
    event &&
    event.shooterId === shooterId &&
    event.victimId === damagedId &&
    (event.weapon & 0x001f) === (weapon & 0x001f)
  );
}

async function waitForServerDamageEvent({
  server,
  sinceCount,
  label,
  shooterId,
  damagedId,
  weaponEntry,
}) {
  const deadline = Date.now() + 4_000;
  while (Date.now() < deadline) {
    const events = parseServerDamageEvents(server.logs).slice(sinceCount);
    const match = events.find((event) =>
      serverDamageMatchesWeapon(event, {
        shooterId,
        damagedId,
        weapon: weaponEntry.weapon,
      }),
    );
    if (match) {
      return match;
    }
    await sleep(50);
  }
  throw new Error(`${label} did not produce authoritative server damage: ${JSON.stringify({
    shooterId,
    damagedId,
    weaponEntry,
    serverDamageEvents: parseServerDamageEvents(server.logs).slice(-12),
    serverLogs: server.logs.join("").slice(-4000),
  })}`);
}

function mergedPacketLog(state) {
  const bySeq = new Map();
  for (const entry of [...(state.packetLogFirst || []), ...(state.packetLog || [])]) {
    bySeq.set(entry.seq ?? `${entry.direction}:${entry.at}:${JSON.stringify(entry.bytes || [])}`, entry);
  }
  return [...bySeq.values()].sort((left, right) => (left.seq || 0) - (right.seq || 0));
}

function latestClientPositionPacket(state) {
  const entry = [...(state.packetLog || [])]
    .reverse()
    .find(
      (packet) =>
        packet.direction === "sent" &&
        Array.isArray(packet.bytes) &&
        packet.bytes[0] === 0x03 &&
        packet.bytes.length >= 22 &&
        packet.bytes[20] === 0 &&
        packet.bytes[21] === 0,
    );
  return entry ? Buffer.from(entry.bytes) : null;
}

const POSITION_SEQUENCE_MAGIC = 0x5a5a0000;
const POSITION_SEQUENCE_MASK = 0xffff0000;

function clientPositionSequence(packet) {
  if (!packet || packet.length < 32) {
    return null;
  }
  const items = packet.readUInt32LE(28);
  return (items & POSITION_SEQUENCE_MASK) === POSITION_SEQUENCE_MAGIC ? items & 0xffff : null;
}

function latestSentClientPositionSequence(state) {
  let latest = null;
  for (const entry of [...(state.packetLogFirst || []), ...(state.packetLog || [])]) {
    if (
      entry.direction === "sent" &&
      Array.isArray(entry.bytes) &&
      entry.bytes[0] === 0x03 &&
      entry.bytes.length >= 32
    ) {
      const sequence = clientPositionSequence(Buffer.from(entry.bytes));
      if (sequence !== null) {
        latest = latest === null ? sequence : Math.max(latest, sequence);
      }
    }
  }
  return latest;
}

function clientPositionPacketWithSequence(packet, sequence) {
  const source = Buffer.from(packet || fallbackClientPositionPacket());
  const output = Buffer.alloc(Math.max(source.length, 32));
  source.copy(output);
  output.writeUInt32LE(0, 2);
  output.writeUInt32LE((POSITION_SEQUENCE_MAGIC | (sequence & 0xffff)) >>> 0, 28);
  return output;
}

function clientPositionPacketWithSequenceOffset(packet, offset) {
  const sequence = clientPositionSequence(packet) ?? 0;
  return clientPositionPacketWithSequence(packet, (sequence + offset) & 0xffff);
}

function baseClientPositionPacketForTest(state) {
  const packet = latestClientPositionPacket(state) || fallbackClientPositionPacket();
  const latestSequence = latestSentClientPositionSequence(state) ?? clientPositionSequence(packet) ?? 0;
  return clientPositionPacketWithSequence(packet, latestSequence);
}

function fallbackClientPositionPacket() {
  return Buffer.from([
    0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x03, 0x00, 0x00,
  ]);
}

function weaponPacketFromPosition(basePacket, weapon, sequenceOffset) {
  const packet = clientPositionPacketWithSequenceOffset(
    basePacket || fallbackClientPositionPacket(),
    sequenceOffset,
  );
  packet.writeUInt16LE(weapon, 20);
  return packet;
}

function positionedPacketFromPosition(basePacket, { x, y, direction = 0, offsetTicks = 20 }) {
  const packet = clientPositionPacketWithSequenceOffset(
    basePacket || fallbackClientPositionPacket(),
    offsetTicks,
  );
  packet[1] = direction & 0x3f;
  packet.writeInt16LE(0, 6);
  packet.writeUInt16LE(y, 8);
  packet.writeUInt16LE(x, 12);
  packet.writeInt16LE(0, 14);
  packet.writeUInt16LE(0, 20);
  return packet;
}

function prizePacketFromPosition(basePacket, prizeId, _offsetTicks) {
  const packet = Buffer.alloc(11);
  const base = Buffer.from(basePacket || fallbackClientPositionPacket());
  packet[0] = 0x07;
  packet.writeUInt32LE(0, 1);
  packet.writeUInt16LE(base.readUInt16LE(12), 5);
  packet.writeUInt16LE(base.readUInt16LE(8), 7);
  packet.writeInt16LE(prizeId, 9);
  return packet;
}

function damageReportPayload({ timestamp, shooterId, weapon, energy, damage, timestampOffset = 0 }) {
  const payload = Buffer.alloc(14);
  payload[0] = 0x32;
  payload.writeUInt32LE(timestamp >>> 0, 1);
  payload.writeUInt16LE(shooterId, 5);
  payload.writeUInt16LE(weapon, 7);
  payload.writeUInt16LE(energy, 9);
  payload.writeUInt16LE(damage, 11);
  payload[13] = timestampOffset;
  return payload;
}

async function chromeSendRaw(chrome, packet) {
  const sent = await chrome.evaluate(
    `window.__zubspaceSendRawForTest?.(${JSON.stringify([...packet])}) === true`,
  );
  if (sent !== true) {
    throw new Error("Chrome raw network test hook did not send");
  }
}

async function firefoxSendRaw(firefox, packet) {
  const sent = await executeFirefox(
    firefox,
    `return window.__zubspaceSendRawForTest?.(${JSON.stringify([...packet])}) === true;`,
  );
  if (sent !== true) {
    throw new Error("Firefox raw network test hook did not send");
  }
}

async function sendReliablePayloadForTest(client, payload) {
  const expression = `window.__zubspaceSendReliablePayloadForTest?.(${JSON.stringify([...payload])}) === true`;
  const sent =
    typeof client?.evaluate === "function"
      ? await client.evaluate(expression)
      : await executeFirefox(client, `return ${expression};`);
  if (sent !== true) {
    throw new Error("browser reliable payload test hook did not send");
  }
}

async function sendPrizeForTest({ sender, senderState, sendRaw, prizeId }) {
  const basePacket = baseClientPositionPacketForTest(senderState);
  await sendRaw(sender, prizePacketFromPosition(basePacket, prizeId, 20));
}

async function sendPositionForTest({ sender, readSenderState, sendRaw, x, y, direction = 0 }) {
  for (let index = 0; index < 3; index += 1) {
    const basePacket = baseClientPositionPacketForTest(await readSenderState());
    await sendRaw(sender, positionedPacketFromPosition(basePacket, {
      x,
      y,
      direction,
      offsetTicks: 20 + index * 3,
    }));
    await sleep(80);
  }
}

const controlledChromePosition = { x: 8200, y: 8200, direction: 10 };
const controlledFirefoxPosition = { x: 8240, y: 8200, direction: 30 };

async function placeBrowsersBackInCombat({ chrome, firefox, chromeSendRaw, firefoxSendRaw, settleMs = 600 }) {
  await Promise.all([
    sendPositionForTest({
      sender: chrome,
      readSenderState: () => chrome.pageState(),
      sendRaw: chromeSendRaw,
      ...controlledChromePosition,
    }),
    sendPositionForTest({
      sender: firefox,
      readSenderState: () => firefoxPageState(firefox),
      sendRaw: firefoxSendRaw,
      ...controlledFirefoxPosition,
    }),
  ]);
  await sleep(settleMs);
}

function combatPlayerReady(metrics, minEnergy) {
  return !!(
    metrics &&
    (metrics.playerRespawnTimer ?? 0) <= 0.05 &&
    Number.isFinite(metrics.playerEnergy) &&
    metrics.playerEnergy >= minEnergy
  );
}

async function placeBrowsersInReadyCombat({
  label,
  chrome,
  firefox,
  chromeSendRaw,
  firefoxSendRaw,
  minEnergy = 1600,
  settleMs = 350,
  timeoutMs = 16_000,
}) {
  await waitUntil(`${label} alive and charged`, async () => {
    const [chromeState, firefoxState] = await Promise.all([
      chrome.pageState(),
      firefoxPageState(firefox),
    ]);
    return (
      combatPlayerReady(chromeState.uiMetrics, minEnergy) &&
      combatPlayerReady(firefoxState.uiMetrics, minEnergy)
    );
  }, timeoutMs);
  await placeBrowsersBackInCombat({ chrome, firefox, chromeSendRaw, firefoxSendRaw, settleMs });
  await waitUntil(`${label} combat placement visible`, async () => {
    const [chromeState, firefoxState] = await Promise.all([
      chrome.pageState(),
      firefoxPageState(firefox),
    ]);
    return (
      combatPlayerReady(chromeState.uiMetrics, minEnergy) &&
      combatPlayerReady(firefoxState.uiMetrics, minEnergy) &&
      (chromeState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
      (firefoxState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1
    );
  }, timeoutMs);
}

async function sendPositionWeaponForTest({
  sender,
  senderState,
  sendRaw,
  weapon,
  count = 6,
  readSenderState = null,
}) {
  for (let index = 0; index < count; index += 1) {
    const currentState = readSenderState ? await readSenderState() : senderState;
    const basePacket = baseClientPositionPacketForTest(currentState);
    await sendRaw(sender, weaponPacketFromPosition(basePacket, weapon, 20));
    await sleep(140);
  }
}

async function sendDamageReportForTest({
  damaged,
  shooterId,
  weaponEntry,
  reportedWeapon = weaponEntry.weapon,
  energy,
  damage,
}) {
  await sendReliablePayloadForTest(
    damaged,
    damageReportPayload({
      timestamp: weaponEntry.timestamp,
      shooterId,
      weapon: reportedWeapon,
      energy,
      damage,
    }),
  );
}

const lowWeaponTypeMatches = (weaponTypes) => (entry) =>
  weaponTypes.includes(entry.weapon & 0x001f);
const bombWeaponMatches = (entry) =>
  [3, 4].includes(entry.weapon & 0x001f) && (entry.weapon & 0x8000) === 0;
const mineWeaponMatches = (entry) =>
  [3, 4].includes(entry.weapon & 0x001f) && (entry.weapon & 0x8000) !== 0;
const shrapnelBombWeaponMatches = (entry) =>
  bombWeaponMatches(entry) && ((entry.weapon >> 10) & 0x001f) > 0;
const weaponWithBits = (
  weaponType,
  {
    level = 1,
    shrapnelLevel = 1,
    shrapnel = 0,
    shrapnelBouncing = false,
    alternate = false,
  } = {},
) =>
  (weaponType & 0x001f) |
  (((Math.max(1, Math.min(4, level)) - 1) & 0x03) << 5) |
  ((shrapnelBouncing ? 1 : 0) << 7) |
  (((Math.max(1, Math.min(4, shrapnelLevel)) - 1) & 0x03) << 8) |
  ((Math.max(0, Math.min(31, shrapnel)) & 0x1f) << 10) |
  ((alternate ? 1 : 0) << 15);

async function assertRemoteWeapon({
  label,
  senderLabel,
  receiverLabel,
  sender,
  sendRaw,
  readSenderState,
  readReceiverState,
  weapon,
  matchesWeapon,
  metric,
  activeMetric = null,
  count = 6,
  acceptDamageFallback = false,
}) {
  const receiverStateBefore = await readReceiverState();
  const metricBefore = receiverStateBefore.uiMetrics?.[metric] ?? 0;
  const activeMetricBefore = activeMetric ? (receiverStateBefore.uiMetrics?.[activeMetric] ?? 0) : 0;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const currentSenderState = await readSenderState();
    await sendPositionWeaponForTest({
      sender,
      senderState: currentSenderState,
      sendRaw,
      weapon,
      count,
      readSenderState,
    });
    await sleep(300);
    const receiverState = await readReceiverState();
    const receiverSummary = summarizeClient(receiverState);
    if (
      (receiverState.uiMetrics?.[metric] ?? 0) > metricBefore &&
      (!activeMetric || (receiverState.uiMetrics?.[activeMetric] ?? 0) > activeMetricBefore)
    ) {
      const nextSenderState = await readSenderState();
      const senderSummary = summarizeClient(nextSenderState);
      if (!senderSummary.sentPositionWeapons.some(matchesWeapon)) {
        throw new Error(`${senderLabel} did not log sent ${label} weapon`);
      }
      if (
        !senderSummary.sentPositionWeapons.some(
          (entry) => matchesWeapon(entry) && entry.channel === "zubspace-events",
        )
      ) {
        throw new Error(`${senderLabel} did not send ${label} over zubspace-events`);
      }
      if (
        !senderSummary.sentPositionWeapons.some(
          (entry) => matchesWeapon(entry) && entry.channel === "zubspace-reliable",
        )
      ) {
        throw new Error(`${senderLabel} did not duplicate ${label} over zubspace-reliable`);
      }
      const sentWeapon = [...senderSummary.sentPositionWeapons]
        .reverse()
        .find((entry) => matchesWeapon(entry) && entry.channel === "zubspace-events");
      return {
        sender: clientReport(senderSummary),
        receiver: clientReport(receiverSummary),
        receiverMetric: receiverState.uiMetrics?.[metric] ?? 0,
        receiverActiveMetric: activeMetric ? (receiverState.uiMetrics?.[activeMetric] ?? 0) : null,
        receiverWeaponPackets: receiverSummary.receivedPositionWeapons.filter(matchesWeapon).length,
        sentWeapon,
      };
    }
  }
  const finalSenderState = await readSenderState();
  const finalSenderSummary = summarizeClient(finalSenderState);
  const finalReceiverState = await readReceiverState();
  const finalReceiverSummary = summarizeClient(finalReceiverState);
  const receiverDamageFromSender =
    acceptDamageFallback &&
    finalReceiverState.uiMetrics?.lastNetworkDamagePlayerId === finalSenderSummary.selfId &&
    Number.isFinite(finalReceiverState.uiMetrics?.lastNetworkDamageEnergy);
  if (receiverDamageFromSender) {
    return {
      sender: clientReport(finalSenderSummary),
      receiver: clientReport(finalReceiverSummary),
      receiverMetric: finalReceiverState.uiMetrics?.[metric] ?? 0,
      receiverActiveMetric: activeMetric ? (finalReceiverState.uiMetrics?.[activeMetric] ?? 0) : null,
      receiverWeaponPackets: finalReceiverSummary.receivedPositionWeapons.filter(matchesWeapon).length,
      sentWeapon: [...finalSenderSummary.sentPositionWeapons]
        .reverse()
        .find((entry) => matchesWeapon(entry) && entry.channel === "zubspace-events") ?? null,
      damageFallback: true,
    };
  }
  throw new Error(
    `${receiverLabel} did not receive/apply remote ${label} from ${senderLabel}: ${JSON.stringify({
      sender: clientReport(finalSenderSummary),
      senderUi: finalSenderState.uiMetrics ?? null,
      receiver: clientReport(finalReceiverSummary),
      receiverUi: finalReceiverState.uiMetrics ?? null,
      receiverMetric: finalReceiverState.uiMetrics?.[metric] ?? 0,
      receiverWeapons: finalReceiverSummary.receivedPositionWeapons.slice(-12),
      senderWeapons: finalSenderSummary.sentPositionWeapons.slice(-12),
    })}`,
  );
}

function damageReportMatches(report, damagedId, shooterId, weaponEntry) {
  return (
    report.playerId === damagedId &&
    report.entries.some(
      (entry) =>
        entry.attackerId === shooterId &&
        (entry.weapon & 0x001f) === (weaponEntry.weapon & 0x001f) &&
        entry.damage > 0,
    )
  );
}

function matchingDamageReport(report, damagedId, shooterId, weaponEntry) {
  if (!damageReportMatches(report, damagedId, shooterId, weaponEntry)) {
    return null;
  }
  return report.entries.find(
    (entry) =>
      entry.attackerId === shooterId &&
      (entry.weapon & 0x001f) === (weaponEntry.weapon & 0x001f) &&
      entry.damage > 0,
  ) || null;
}

function damageReportMatchesServerEvent(report, event) {
  return (
    report.playerId === event.victimId &&
    report.entries.some(
      (entry) =>
        entry.attackerId === event.shooterId &&
        (entry.weapon & 0x001f) === (event.weapon & 0x001f) &&
        entry.damage === event.damage &&
        entry.energy === event.energyAfter,
    )
  );
}

function damageTraceMatchesServerEvent(entry, event) {
  return (
    entry &&
    entry.victimId === event.victimId &&
    entry.attackerId === event.shooterId &&
    (entry.weapon & 0x001f) === (event.weapon & 0x001f) &&
    entry.damage === event.damage &&
    Math.abs(entry.energyAfter - event.energyAfter) <= 1
  );
}

function latestDamageTraceMatch(state, event) {
  return [...(state.damageTrace || [])].reverse().find((entry) =>
    damageTraceMatchesServerEvent(entry, event),
  ) || null;
}

function selfDamageTraceMatchesServerEvent(entry, event) {
  return (
    entry &&
    entry.selfPlayerId === event.victimId &&
    entry.selfDamageAttackerId === event.shooterId &&
    (entry.selfDamageWeapon & 0x001f) === (event.weapon & 0x001f) &&
    entry.selfDamage === event.damage &&
    Math.abs(entry.selfDamageEnergyAfter - event.energyAfter) <= 1
  );
}

function latestSelfDamageTraceMatch(state, event) {
  return [...(state.damageTrace || [])].reverse().find((entry) =>
    selfDamageTraceMatchesServerEvent(entry, event),
  ) || null;
}

function remoteEnergyTraceMatchesServerEvent(entry, event) {
  return (
    entry &&
    entry.selfPlayerId === event.shooterId &&
    damageEnergyClose(entry.remoteEnergy, event.energyAfter)
  );
}

function latestRemoteEnergyTraceMatch(state, event) {
  return [...(state.damageTrace || [])].reverse().find((entry) =>
    remoteEnergyTraceMatchesServerEvent(entry, event),
  ) || null;
}

function damageEnergyClose(value, expected, tolerance = 96) {
  return Number.isFinite(value) && Math.abs(value - expected) <= tolerance;
}

function hudDamageTupleMatches(metrics, event, forSelf) {
  return !!(
    metrics &&
    metrics.lastNetworkDamagePlayerId === event.victimId &&
    metrics.lastNetworkDamageAttackerId === event.shooterId &&
    (metrics.lastNetworkDamageWeapon & 0x001f) === (event.weapon & 0x001f) &&
    metrics.lastNetworkDamageAmount === event.damage &&
    Math.abs(metrics.lastNetworkDamageEnergy - event.energyAfter) <= 1 &&
    metrics.lastNetworkDamageForSelf === forSelf
  );
}

function hudSelfDamageTupleMatches(metrics, event) {
  return !!(
    metrics &&
    metrics.selfPlayerId === event.victimId &&
    metrics.lastSelfDamageAttackerId === event.shooterId &&
    (metrics.lastSelfDamageWeapon & 0x001f) === (event.weapon & 0x001f) &&
    metrics.lastSelfDamageAmount === event.damage &&
    Math.abs(metrics.lastSelfDamageEnergy - event.energyAfter) <= 1
  );
}

async function assertAuthoritativeDamageSync({
  label,
  event,
  readShooterState,
  readDamagedState,
  beforeShooterState,
  beforeDamagedState,
}) {
  const beforeRemoteEnergy = beforeShooterState?.uiMetrics?.remoteEnergy;
  const beforeDamagedEnergy = beforeDamagedState?.uiMetrics?.playerEnergy;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    await sleep(50);
    const [shooterState, damagedState] = await Promise.all([
      readShooterState(),
      readDamagedState(),
    ]);
    const shooterSummary = summarizeClient(shooterState);
    const damagedSummary = summarizeClient(damagedState);
    const shooterReceived = shooterSummary.receivedDamageReports.some((report) =>
      damageReportMatchesServerEvent(report, event),
    );
    const damagedReceived = damagedSummary.receivedDamageReports.some((report) =>
      damageReportMatchesServerEvent(report, event),
    );
    const shooterTrace = latestDamageTraceMatch(shooterState, event);
    const shooterRemoteTrace = latestRemoteEnergyTraceMatch(shooterState, event);
    const damagedTrace = latestDamageTraceMatch(damagedState, event);
    const damagedSelfTrace = latestSelfDamageTraceMatch(damagedState, event);
    const shooterHud =
      (event.lethal && shooterReceived) ||
      hudDamageTupleMatches(shooterState.uiMetrics, event, false) ||
      damageTraceMatchesServerEvent(shooterTrace, event) ||
      remoteEnergyTraceMatchesServerEvent(shooterRemoteTrace, event);
    const damagedHud =
      (event.lethal && damagedReceived) ||
      hudDamageTupleMatches(damagedState.uiMetrics, event, true) ||
      hudSelfDamageTupleMatches(damagedState.uiMetrics, event) ||
      damageTraceMatchesServerEvent(damagedTrace, event) ||
      selfDamageTraceMatchesServerEvent(damagedSelfTrace, event);
    const shooterRemoteEnergy =
      shooterRemoteTrace?.remoteEnergy ?? shooterTrace?.remoteEnergy ?? shooterState.uiMetrics?.remoteEnergy;
    const damagedPlayerEnergy =
      damagedSelfTrace?.playerEnergy ?? damagedTrace?.playerEnergy ?? damagedState.uiMetrics?.playerEnergy;
    const shooterRemoteDropped =
      !Number.isFinite(beforeRemoteEnergy) ||
      (Number.isFinite(shooterRemoteEnergy) && shooterRemoteEnergy < beforeRemoteEnergy - 1);
    const damagedHudDropped =
      !Number.isFinite(beforeDamagedEnergy) ||
      (Number.isFinite(damagedPlayerEnergy) && damagedPlayerEnergy < beforeDamagedEnergy - 1);
    const shooterRemoteSynced =
      event.lethal ||
      (damageEnergyClose(shooterRemoteEnergy, event.energyAfter) && shooterRemoteDropped);
    const damagedHudSynced =
      event.lethal ||
      (damageEnergyClose(damagedPlayerEnergy, event.energyAfter) && damagedHudDropped);

    if (
      (shooterReceived || shooterTrace || shooterRemoteTrace) &&
      (damagedReceived || damagedTrace || damagedSelfTrace) &&
      shooterHud &&
      damagedHud &&
      (shooterTrace || shooterRemoteTrace) &&
      (damagedTrace || damagedSelfTrace) &&
      shooterRemoteSynced &&
      damagedHudSynced
    ) {
      return {
        server: event,
        shooter: clientReport(shooterSummary),
        damaged: clientReport(damagedSummary),
        beforeRemoteEnergy,
        beforeDamagedEnergy,
        remoteEnergy: shooterRemoteEnergy,
        damagedEnergy: damagedPlayerEnergy,
        shooterTrace,
        shooterRemoteTrace,
        damagedTrace,
        damagedSelfTrace,
      };
    }
  }

  const [shooterState, damagedState] = await Promise.all([
    readShooterState(),
    readDamagedState(),
  ]);
  throw new Error(`${label} did not sync authoritative damage into both browser views: ${JSON.stringify({
    server: event,
    beforeRemoteEnergy,
    beforeDamagedEnergy,
    shooter: clientReport(summarizeClient(shooterState)),
    damaged: clientReport(summarizeClient(damagedState)),
    shooterDamage: summarizeClient(shooterState).receivedDamageReports.slice(-8),
    damagedDamage: summarizeClient(damagedState).receivedDamageReports.slice(-8),
    shooterDamageTrace: (shooterState.damageTrace || []).slice(-8),
    damagedDamageTrace: (damagedState.damageTrace || []).slice(-8),
    shooterUi: shooterState.uiMetrics,
    damagedUi: damagedState.uiMetrics,
  })}`);
}

function sentDamageReportMatches(report, shooterId, weaponEntry) {
  return report.entries.some(
    (entry) =>
      entry.shooterId === shooterId &&
      (entry.weapon & 0x001f) === (weaponEntry.weapon & 0x001f) &&
      entry.damage > 0,
  );
}

function observedDamageSync({
  shooterState,
  damagedState,
  shooterId,
  damagedId,
  weaponEntry,
}) {
  const shooterSummary = summarizeClient(shooterState);
  const damagedSummary = summarizeClient(damagedState);
  const shooterSawDamage = shooterSummary.receivedDamageReports.some((report) =>
    damageReportMatches(report, damagedId, shooterId, weaponEntry),
  );
  const damagedWatchEntry = [...damagedSummary.receivedDamageReports]
    .reverse()
    .map((report) => matchingDamageReport(report, damagedId, shooterId, weaponEntry))
    .find(Boolean);
  const shooterUiEnergy = shooterState.uiMetrics?.lastNetworkDamageEnergy;
  const damagedUiEnergy = damagedState.uiMetrics?.lastNetworkDamageEnergy;
  const shooterSawSnapshotDamage =
    shooterState.uiMetrics?.lastNetworkDamagePlayerId === damagedId &&
    Number.isFinite(shooterUiEnergy);
  const damagedSawSnapshotDamage =
    damagedState.uiMetrics?.lastNetworkDamageForSelf === true &&
    damagedState.uiMetrics?.lastNetworkDamagePlayerId === damagedId &&
    Number.isFinite(damagedUiEnergy);
  const damagedSawDamage = !!damagedWatchEntry || damagedSawSnapshotDamage;
  if ((!shooterSawDamage && !shooterSawSnapshotDamage) || !damagedSawDamage) {
    return null;
  }
  const acceptedEnergy = damagedWatchEntry?.energy ?? damagedUiEnergy ?? shooterUiEnergy;
  const remoteEnergy = shooterState.uiMetrics?.remoteEnergy;
  const damagedEnergy = damagedState.uiMetrics?.playerEnergy;
  const lastNetworkDamageEnergy = damagedState.uiMetrics?.lastNetworkDamageEnergy;
  const remoteEnergyUpdated =
    Number.isFinite(acceptedEnergy) &&
    Number.isFinite(remoteEnergy) &&
    (Math.abs(remoteEnergy - acceptedEnergy) <= 24 ||
      (Number.isFinite(shooterUiEnergy) && Math.abs(shooterUiEnergy - acceptedEnergy) <= 1));
  const damagedEnergyObserved =
    Number.isFinite(acceptedEnergy) &&
    damagedState.uiMetrics?.lastNetworkDamageForSelf === true &&
    Number.isFinite(lastNetworkDamageEnergy) &&
    Math.abs(lastNetworkDamageEnergy - acceptedEnergy) <= 1 &&
    Number.isFinite(damagedEnergy) &&
    Math.abs(damagedEnergy - acceptedEnergy) <= 96;

  if (
    (shooterSawDamage || shooterSawSnapshotDamage || remoteEnergyUpdated) &&
    (damagedSawDamage || damagedEnergyObserved)
  ) {
    return {
      shooter: clientReport(shooterSummary),
      damaged: clientReport(damagedSummary),
      remoteEnergy,
      damagedEnergy,
      acceptedEnergy,
      shooterSawDamage: shooterSawDamage || shooterSawSnapshotDamage,
      damagedSawDamage,
    };
  }
  return null;
}

async function assertDamageSync({
  label,
  shooterId,
  damagedId,
  damaged,
  sendRaw,
  readShooterState,
  readDamagedState,
  weaponEntry,
  reportedWeapon = weaponEntry?.weapon,
  damage = 10,
  expectRemoteEnergy = true,
  allowPreexisting = true,
}) {
  if (!weaponEntry || !Number.isFinite(weaponEntry.timestamp)) {
    throw new Error(`${label} cannot assert damage without a sent weapon timestamp`);
  }
  const reportedWeaponEntry = { ...weaponEntry, weapon: reportedWeapon };
  const matchesReceivedDamage = (report) =>
    damageReportMatches(report, damagedId, shooterId, reportedWeaponEntry) ||
    damageReportMatches(report, damagedId, shooterId, weaponEntry);
  const matchingReceivedDamage = (report) =>
    matchingDamageReport(report, damagedId, shooterId, reportedWeaponEntry) ||
    matchingDamageReport(report, damagedId, shooterId, weaponEntry);
  const [shooterBefore, damagedBefore] = await Promise.all([
    readShooterState(),
    readDamagedState(),
  ]);
  const beforeRemoteEnergy = shooterBefore.uiMetrics?.remoteEnergy;
  const beforeDamagedEnergy = damagedBefore.uiMetrics?.playerEnergy;
  if (allowPreexisting) {
    const existingDamage = observedDamageSync({
      shooterState: shooterBefore,
      damagedState: damagedBefore,
      shooterId,
      damagedId,
      weaponEntry: reportedWeaponEntry,
    });
    if (existingDamage) {
      return {
        ...existingDamage,
        beforeRemoteEnergy,
        beforeDamagedEnergy,
        preexisting: true,
      };
    }
  }
  const postEnergy = Math.max(
    0,
    Math.floor((Number.isFinite(beforeDamagedEnergy) ? beforeDamagedEnergy : 1000) - damage),
  );

  await sendDamageReportForTest({
    damaged,
    sendRaw,
    shooterId,
    weaponEntry,
    reportedWeapon,
    energy: postEnergy,
    damage,
  });

  for (let attempt = 0; attempt < 80; attempt += 1) {
    await sleep(50);
    const [shooterState, damagedState] = await Promise.all([
      readShooterState(),
      readDamagedState(),
    ]);
    const shooterSummary = summarizeClient(shooterState);
    const damagedSummary = summarizeClient(damagedState);
    const shooterSawDamage = shooterSummary.receivedDamageReports.some(matchesReceivedDamage);
    const damagedWatchEntry = [...damagedSummary.receivedDamageReports]
      .reverse()
      .map(matchingReceivedDamage)
      .find(Boolean);
    const damagedSawDamage = !!damagedWatchEntry;
    const damagedSentDamage = damagedSummary.sentDamageReports.some((report) =>
      sentDamageReportMatches(report, shooterId, reportedWeaponEntry),
    );
    const remoteEnergy = shooterState.uiMetrics?.remoteEnergy;
    const damagedEnergy = damagedState.uiMetrics?.playerEnergy;
    const shooterUiEnergy = shooterState.uiMetrics?.lastNetworkDamageEnergy;
    const damagedUiEnergy = damagedState.uiMetrics?.lastNetworkDamageEnergy;
    const shooterSawSnapshotDamage =
      shooterState.uiMetrics?.lastNetworkDamagePlayerId === damagedId &&
      Number.isFinite(shooterUiEnergy);
    const damagedSawSnapshotDamage =
      damagedState.uiMetrics?.lastNetworkDamageForSelf === true &&
      damagedState.uiMetrics?.lastNetworkDamagePlayerId === damagedId &&
      Number.isFinite(damagedUiEnergy);
    const acceptedEnergy = damagedWatchEntry?.energy ?? damagedUiEnergy ?? shooterUiEnergy;
    const authoritativeDamageObserved = damagedSawDamage && Number.isFinite(acceptedEnergy);
    const lastNetworkDamageEnergy = damagedState.uiMetrics?.lastNetworkDamageEnergy;
    const remoteEnergyUpdated =
      !expectRemoteEnergy
        ? shooterSawDamage || shooterSawSnapshotDamage
        :
      Number.isFinite(acceptedEnergy)
        ? (Number.isFinite(remoteEnergy) && Math.abs(remoteEnergy - acceptedEnergy) <= 24) ||
          (Number.isFinite(shooterUiEnergy) && Math.abs(shooterUiEnergy - acceptedEnergy) <= 1) ||
          authoritativeDamageObserved
        : !Number.isFinite(beforeRemoteEnergy) ||
          (Number.isFinite(remoteEnergy) && remoteEnergy < beforeRemoteEnergy);
    const damagedEnergyUpdated =
      Number.isFinite(acceptedEnergy)
        ? authoritativeDamageObserved ||
          damagedState.uiMetrics?.lastNetworkDamageForSelf === true &&
          Number.isFinite(lastNetworkDamageEnergy) &&
          Math.abs(lastNetworkDamageEnergy - acceptedEnergy) <= 1 &&
          Number.isFinite(damagedEnergy) &&
          Math.abs(damagedEnergy - acceptedEnergy) <= 96
        : !Number.isFinite(beforeDamagedEnergy) ||
          (Number.isFinite(damagedEnergy) && damagedEnergy < beforeDamagedEnergy);

    if (
      (shooterSawDamage || shooterSawSnapshotDamage || authoritativeDamageObserved) &&
      (damagedSawDamage || damagedSawSnapshotDamage) &&
      damagedSentDamage &&
      remoteEnergyUpdated &&
      damagedEnergyUpdated
    ) {
      return {
        shooter: clientReport(shooterSummary),
        damaged: clientReport(damagedSummary),
        remoteEnergy,
        damagedEnergy,
        acceptedEnergy,
        beforeRemoteEnergy,
        beforeDamagedEnergy,
      };
    }
  }

  const [shooterState, damagedState] = await Promise.all([
    readShooterState(),
    readDamagedState(),
  ]);
  throw new Error(`${label} did not sync watch_damage/energy: ${JSON.stringify({
    shooter: clientReport(summarizeClient(shooterState)),
    damaged: clientReport(summarizeClient(damagedState)),
    shooterDamage: summarizeClient(shooterState).receivedDamageReports.slice(-5),
    damagedDamage: summarizeClient(damagedState).receivedDamageReports.slice(-5),
    sentDamage: summarizeClient(damagedState).sentDamageReports.slice(-5),
    beforeRemoteEnergy,
    beforeDamagedEnergy,
    shooterUi: shooterState.uiMetrics,
    damagedUi: damagedState.uiMetrics,
    weaponEntry,
    reportedWeapon,
  })}`);
}

function deathMatches(death, killerId, killedId) {
  return death.killerId === killerId && death.killedId === killedId;
}

function localRespawnDelayActive(metrics) {
  if (!metrics) return false;
  if ((metrics.playerRespawnSeq ?? 0) <= 0) return false;
  if ((metrics.playerRespawnTimer ?? 0) < 0.5) return false;
  return (
    Number.isFinite(metrics.playerEnergy) &&
    metrics.playerEnergy <= 1
  );
}

function scoreUpdateAtLeast(summary, playerId, wins, losses) {
  return summary.receivedScoreUpdates.some(
    (update) =>
      update.playerId === playerId &&
      update.wins >= wins &&
      update.losses >= losses,
  );
}

async function assertDeathAndScoreSync({
  label,
  shooterId,
  damagedId,
  readShooterState,
  readDamagedState,
}) {
  let sawRespawnDelay = false;
  let respawnDelayMetrics = null;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    await sleep(50);
    const [shooterState, damagedState] = await Promise.all([
      readShooterState(),
      readDamagedState(),
    ]);
    const shooterSummary = summarizeClient(shooterState);
    const damagedSummary = summarizeClient(damagedState);
    const shooterSawDeath = shooterSummary.receivedDeaths.some((death) =>
      deathMatches(death, shooterId, damagedId),
    );
    const damagedSawDeath = damagedSummary.receivedDeaths.some((death) =>
      deathMatches(death, shooterId, damagedId),
    );
    const shooterSawScores =
      scoreUpdateAtLeast(shooterSummary, shooterId, 1, 0) &&
      scoreUpdateAtLeast(shooterSummary, damagedId, 0, 1);
    const damagedSawScores =
      scoreUpdateAtLeast(damagedSummary, shooterId, 1, 0) &&
      scoreUpdateAtLeast(damagedSummary, damagedId, 0, 1);
    const shooterUiScore =
      (shooterState.uiMetrics?.selfWins ?? 0) >= 1;
    const damagedUiScore = (damagedState.uiMetrics?.selfLosses ?? 0) >= 1;
    if (localRespawnDelayActive(damagedState.uiMetrics)) {
      sawRespawnDelay = true;
      respawnDelayMetrics = damagedState.uiMetrics;
    }
    if (
      shooterSawDeath &&
      damagedSawDeath &&
      ((shooterSawScores && damagedSawScores) || (shooterUiScore && damagedUiScore)) &&
      sawRespawnDelay
    ) {
      return {
        shooter: clientReport(shooterSummary),
        damaged: clientReport(damagedSummary),
        shooterDeaths: shooterSummary.receivedDeaths.filter((death) =>
          deathMatches(death, shooterId, damagedId),
        ),
        damagedDeaths: damagedSummary.receivedDeaths.filter((death) =>
          deathMatches(death, shooterId, damagedId),
        ),
        shooterScores: shooterSummary.receivedScoreUpdates.slice(-6),
        damagedScores: damagedSummary.receivedScoreUpdates.slice(-6),
        shooterUi: shooterState.uiMetrics,
        damagedUi: damagedState.uiMetrics,
        damagedRespawnDelayActive: true,
        respawnDelayMetrics,
      };
    }
  }

  const [shooterState, damagedState] = await Promise.all([
    readShooterState(),
    readDamagedState(),
  ]);
  throw new Error(`${label} did not sync death/scoreboard state: ${JSON.stringify({
    shooter: clientReport(summarizeClient(shooterState)),
    damaged: clientReport(summarizeClient(damagedState)),
    shooterDeaths: summarizeClient(shooterState).receivedDeaths.slice(-5),
    damagedDeaths: summarizeClient(damagedState).receivedDeaths.slice(-5),
    shooterScores: summarizeClient(shooterState).receivedScoreUpdates.slice(-8),
    damagedScores: summarizeClient(damagedState).receivedScoreUpdates.slice(-8),
    shooterUi: shooterState.uiMetrics,
    damagedUi: damagedState.uiMetrics,
    damagedRespawnDelayActive: sawRespawnDelay,
    respawnDelayMetrics,
  })}`);
}

async function assertLethalDamageSync({
  shooterLabel,
  receiverLabel,
  shooterId,
  damagedId,
  shooter,
  damaged,
  shooterSendRaw,
  damagedSendRaw,
  readShooterState,
  readDamagedState,
}) {
  const damageSteps = [];
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const weaponResult = await assertRemoteWeapon({
      label: `lethal bomb ${attempt + 1}`,
      senderLabel: shooterLabel,
      receiverLabel,
      sender: shooter,
      sendRaw: shooterSendRaw,
      readSenderState: readShooterState,
      readReceiverState: readDamagedState,
      weapon: 3,
      matchesWeapon: bombWeaponMatches,
      metric: "remoteBombSpawns",
      count: 1,
    });
    const damageResult = await assertDamageSync({
      label: `${shooterLabel} lethal bomb damage ${attempt + 1} on ${receiverLabel}`,
      shooterId,
      damagedId,
      damaged,
      sendRaw: damagedSendRaw,
      readShooterState,
      readDamagedState,
      weaponEntry: weaponResult.sentWeapon,
      damage: 750,
      expectRemoteEnergy: false,
      allowPreexisting: false,
    });
    damageSteps.push({ weaponResult, damageResult });
    if ((damageResult.acceptedEnergy ?? 1) <= 0) {
      return {
        damageSteps,
        death: await assertDeathAndScoreSync({
          label: `${shooterLabel} lethal damage on ${receiverLabel}`,
          shooterId,
          damagedId,
          readShooterState,
          readDamagedState,
        }),
      };
    }
    await sleep(1700);
  }
  throw new Error(`${shooterLabel} did not produce lethal accepted damage on ${receiverLabel}: ${JSON.stringify(damageSteps)}`);
}

function assertRemoteVisible(label, state) {
  if ((state.uiMetrics?.remoteVisiblePlayers ?? 0) < 1) {
    throw new Error(`${label} did not render a remote visible player: ${JSON.stringify(state.uiMetrics)}`);
  }
}

function mixedUiMetricsOk(metrics) {
  return !!(
    metrics &&
    metrics.energyInside &&
    metrics.energyCenterDelta <= 2 &&
    metrics.energyDigitsInside &&
    metrics.energyDigitsRightAligned &&
    metrics.hudTextInside &&
    !metrics.hudRadarOverlap &&
    metrics.radarBottomRight &&
    metrics.radarLabelInside &&
    metrics.rightStatusPanelInside &&
    metrics.rightStatusPanelClearOfRadar &&
    metrics.leftItemPanelInside &&
    metrics.shipLabelInside &&
    metrics.notificationInside &&
    metrics.textPositiveScale &&
    metrics.hudTextScale <= 1 &&
    metrics.shipLabelScale <= 1 &&
    metrics.notificationScale <= 1 &&
    hudLineCountMatchesMode(metrics)
  );
}

function assertMixedUiMetrics(label, state) {
  if (!mixedUiMetricsOk(state.uiMetrics)) {
    throw new Error(`${label} UI metrics are not fitted: ${JSON.stringify(state.uiMetrics)}`);
  }
}

function assertServerBotRoster(label, summary, expectedCount) {
  const expectedNames = Array.from({ length: expectedCount }, (_, index) => `Bot ${index + 1}`);
  const missingNames = expectedNames.filter((name) => !summary.rosterNames.includes(name));
  if (missingNames.length > 0) {
    throw new Error(
      `${label} did not receive shared server bot roster: ${JSON.stringify({
        missingNames,
        rosterNames: summary.rosterNames,
        receivedReliablePayloadTypes: summary.receivedReliablePayloadTypes,
        receivedPacketHeads: summary.receivedPacketHeads,
      })}`,
    );
  }
}

function assertServerBotPositions(label, summary, expectedCount) {
  const botPositionSources = [...new Set(summary.receivedPositionSources.filter((id) => id >= serverBotIdBase))];
  if (botPositionSources.length < expectedCount) {
    throw new Error(
      `${label} did not receive shared server bot positions: ${JSON.stringify({
        botPositionSources,
      })}`,
    );
  }
}

function remoteRenderSampleFromMetrics(metrics) {
  metrics ||= {};
  if (
    !Number.isFinite(metrics.remoteDisplayX) ||
    !Number.isFinite(metrics.remoteDisplayY) ||
    !Number.isFinite(metrics.remoteDisplayOrientation)
  ) {
    return null;
  }
  return {
    at: Date.now(),
    x: metrics.remoteDisplayX,
    y: metrics.remoteDisplayY,
    orientation: metrics.remoteDisplayOrientation,
    positionX: Number.isFinite(metrics.remotePositionX) ? metrics.remotePositionX : null,
    positionY: Number.isFinite(metrics.remotePositionY) ? metrics.remotePositionY : null,
  };
}

// Canonical position admission may evaluate multiple authenticated subpixel
// branches near collision boundaries. Keep the cross-engine presentation
// sample long enough to observe movement even when that fail-closed work and
// debug logging delay the first accepted relay; the driver continues coasting
// after key release, so this remains real motion rather than a synthetic hook.
async function sampleRemoteRenderMotion(label, readMetrics, durationMs = 3000, intervalMs = 40) {
  const samples = [];
  const deadline = Date.now() + durationMs;
  while (Date.now() < deadline) {
    const sample = remoteRenderSampleFromMetrics(await readMetrics());
    if (sample) {
      samples.push(sample);
    }
    await sleep(intervalMs);
  }
  if (samples.length < 8) {
    throw new Error(`${label} did not publish enough remote render samples: ${JSON.stringify(samples)}`);
  }
  return samples;
}

function orientationDelta(a, b) {
  return Math.abs(((b - a + 0.5) % 1 + 1) % 1 - 0.5);
}

function summarizeRemoteRenderSamples(samples) {
  let pathDistance = 0;
  let orientationTravel = 0;
  let movingSteps = 0;
  let maxStep = 0;
  for (let index = 1; index < samples.length; index += 1) {
    const previous = samples[index - 1];
    const current = samples[index];
    const distance = Math.hypot(current.x - previous.x, current.y - previous.y);
    const orientation = orientationDelta(previous.orientation, current.orientation);
    pathDistance += distance;
    orientationTravel += orientation;
    maxStep = Math.max(maxStep, distance);
    if (distance > 0.05 || orientation > 0.0005) {
      movingSteps += 1;
    }
  }
  return {
    samples: samples.length,
    pathDistance: Number(pathDistance.toFixed(2)),
    orientationTravel: Number(orientationTravel.toFixed(4)),
    movingSteps,
    maxStep: Number(maxStep.toFixed(2)),
  };
}

function assertRemoteRenderMotion(label, samples) {
  const summary = summarizeRemoteRenderSamples(samples);
  if (summary.movingSteps < 4 || (summary.pathDistance < 3 && summary.orientationTravel < 0.01)) {
    throw new Error(`${label} remote render samples did not move smoothly: ${JSON.stringify({ summary, samples })}`);
  }
  if (summary.maxStep > 64) {
    throw new Error(`${label} remote render samples jumped too far between polls: ${JSON.stringify({ summary, samples })}`);
  }
  return summary;
}

function clientReport(summary) {
  return {
    selfId: summary.selfId,
    roster: summary.rosters.map((roster) => ({
      id: roster.id,
      name: roster.name,
      frequency: roster.frequency,
    })),
    sentPositions: summary.sentPositionCount,
    receivedPositions: summary.receivedPositionCount,
    sentPerSecond: timelineRate(summary.sentPositions),
    receivedPerSecond: timelineRate(summary.receivedPositionUpdates),
    receivedTimestampGaps: summary.receivedTimestampGaps,
  };
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run cargo build --target wasm32-unknown-unknown --bin zubspace first`);
  }
  if (!fs.existsSync(path.join(repoRoot, serverBin))) {
    throw new Error(`missing ${serverBin}; run cargo build --bin zubspace_server first`);
  }

  const serverPort = await freePort();
  const chromePort = await freePort();
  const geckoPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const chromeProcess = startChrome(chromePort);
  const firefox = startGeckodriver(geckoPort, "firefox");
  let chrome = null;

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(chromeProcess), waitForGeckodriver(firefox)]);
    await createSession(firefox);

    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=chaos-zone&packetlog=1&trace=1`;
    [chrome] = await Promise.all([
      openChromeClient(chromePort, `${pageBase}&name=chrome`),
      openFirefoxClient(firefox, `${pageBase}&name=firefox`),
    ]);

    await Promise.all([chrome.enterShip(), enterFirefoxShip(firefox)]);
    await sleep(1000);
    await waitUntil("mixed clients remote visibility", async () => {
      const [chromeReady, firefoxReady] = await Promise.all([
        chrome.liveState(),
        executeFirefox(firefox, `return ${smokeLiveStateExpression};`),
      ]);
      return (
        smokeGameplayReady(chromeReady.uiMetrics) &&
        smokeGameplayReady(firefoxReady.uiMetrics) &&
        mixedUiMetricsOk(chromeReady.uiMetrics) &&
        mixedUiMetricsOk(firefoxReady.uiMetrics) &&
        (chromeReady.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
        (firefoxReady.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1
      );
    }, 20_000);
    let chromeEarlySummary = null;
    let firefoxEarlySummary = null;
    if (smokeBotCount > 0) {
      const [chromeEarlyState, firefoxEarlyState] = await Promise.all([
        chrome.pageState(),
        firefoxPageState(firefox),
      ]);
      chromeEarlySummary = summarizeClient(chromeEarlyState);
      firefoxEarlySummary = summarizeClient(firefoxEarlyState);
      assertServerBotRoster("Chrome", chromeEarlySummary, smokeBotCount);
      assertServerBotRoster("Firefox", firefoxEarlySummary, smokeBotCount);
    }
    const initialWeaponResults = [];
    const initialDamageResults = [];
    const [initialChromeState, initialFirefoxState] = await Promise.all([
      chrome.liveState(),
      executeFirefox(firefox, `return ${smokeLiveStateExpression};`),
    ]);
    // Current admitted identity is sufficient here. Complete packet/roster,
    // relay and channel audits still use the terminal artifact snapshots.
    const initialChromeSummary = { selfId: initialChromeState.uiMetrics?.selfPlayerId };
    const initialFirefoxSummary = { selfId: initialFirefoxState.uiMetrics?.selfPlayerId };
    if (initialChromeSummary.selfId == null || initialFirefoxSummary.selfId == null) {
      throw new Error(`missing initial assigned player ids: ${JSON.stringify({
        chromeSelfId: initialChromeSummary.selfId,
        firefoxSelfId: initialFirefoxSummary.selfId,
      })}`);
    }

    let weaponResults = [];
    let damageResults = [];
    const lethalResults = [];
    if (smokeCombat) {

    async function assertInitialDamageWeapon({
      direction,
      label,
      weapon,
      matchesWeapon,
      metric,
      count,
      prizeId,
      reportedWeapon,
      senderLabel,
      receiverLabel,
      sender,
      damaged,
      senderId,
      damagedId,
      senderSendRaw,
      damagedSendRaw,
      readSenderState,
      readDamagedState,
      assertAuthoritativeDamage = true,
    }) {
      if (prizeId != null) {
        await sendPrizeForTest({
          sender,
          senderState: await readSenderState(),
          sendRaw: senderSendRaw,
          prizeId,
        });
        await sleep(250);
      }
      const [beforeShooterState, beforeDamagedState] = await Promise.all([
        readSenderState(),
        readDamagedState(),
      ]);
      const serverDamageStart = parseServerDamageEvents(server.logs).length;
      const result = await assertRemoteWeapon({
        label,
        senderLabel,
        receiverLabel,
        sender,
        sendRaw: senderSendRaw,
        readSenderState,
        readReceiverState: readDamagedState,
        weapon,
        matchesWeapon,
        metric,
        count,
      });
      initialWeaponResults.push({
        phase: "pre-movement",
        direction,
        label,
        ...result,
      });
      if (assertAuthoritativeDamage) {
        const serverEvent = await waitForServerDamageEvent({
          server,
          sinceCount: serverDamageStart,
          label: `${senderLabel} ${label} damage on ${receiverLabel}`,
          shooterId: senderId,
          damagedId,
          weaponEntry: result.sentWeapon,
        });
        initialDamageResults.push({
          phase: "pre-movement",
          direction,
          label,
          ...(await assertAuthoritativeDamageSync({
            label: `${senderLabel} ${label} damage on ${receiverLabel}`,
            event: serverEvent,
            readShooterState: readSenderState,
            readDamagedState,
            beforeShooterState,
            beforeDamagedState,
          })),
        });
      }
    }

    for (const initialCase of [
      {
        direction: "chrome-to-firefox",
        label: "bullet",
        senderLabel: "Chrome",
        receiverLabel: "Firefox",
        sender: chrome,
        damaged: firefox,
        senderId: initialChromeSummary.selfId,
        damagedId: initialFirefoxSummary.selfId,
        senderSendRaw: chromeSendRaw,
        damagedSendRaw: firefoxSendRaw,
        readSenderState: () => chrome.pageState(),
        readDamagedState: () => firefoxPageState(firefox),
        weapon: 2,
        matchesWeapon: lowWeaponTypeMatches([1, 2]),
        metric: "remoteBulletSpawns",
        count: 1,
      },
      {
        direction: "firefox-to-chrome",
        label: "bullet",
        senderLabel: "Firefox",
        receiverLabel: "Chrome",
        sender: firefox,
        damaged: chrome,
        senderId: initialFirefoxSummary.selfId,
        damagedId: initialChromeSummary.selfId,
        senderSendRaw: firefoxSendRaw,
        damagedSendRaw: chromeSendRaw,
        readSenderState: () => firefoxPageState(firefox),
        readDamagedState: () => chrome.pageState(),
        weapon: 2,
        matchesWeapon: lowWeaponTypeMatches([1, 2]),
        metric: "remoteBulletSpawns",
        count: 1,
      },
    ]) {
      await placeBrowsersInReadyCombat({
        label: `${initialCase.direction} ${initialCase.label}`,
        chrome,
        firefox,
        chromeSendRaw,
        firefoxSendRaw,
      });
      await assertInitialDamageWeapon(initialCase);
    }

    for (const initialCase of [
      {
        direction: "chrome-to-firefox",
        label: "bomb",
        senderLabel: "Chrome",
        receiverLabel: "Firefox",
        sender: chrome,
        damaged: firefox,
        senderId: initialChromeSummary.selfId,
        damagedId: initialFirefoxSummary.selfId,
        senderSendRaw: chromeSendRaw,
        damagedSendRaw: firefoxSendRaw,
        readSenderState: () => chrome.pageState(),
        readDamagedState: () => firefoxPageState(firefox),
        weapon: 3,
        matchesWeapon: bombWeaponMatches,
        metric: "remoteBombSpawns",
        count: 1,
      },
      {
        direction: "firefox-to-chrome",
        label: "bomb",
        senderLabel: "Firefox",
        receiverLabel: "Chrome",
        sender: firefox,
        damaged: chrome,
        senderId: initialFirefoxSummary.selfId,
        damagedId: initialChromeSummary.selfId,
        senderSendRaw: firefoxSendRaw,
        damagedSendRaw: chromeSendRaw,
        readSenderState: () => firefoxPageState(firefox),
        readDamagedState: () => chrome.pageState(),
        weapon: 3,
        matchesWeapon: bombWeaponMatches,
        metric: "remoteBombSpawns",
        count: 1,
      },
    ]) {
      await placeBrowsersInReadyCombat({
        label: `${initialCase.direction} ${initialCase.label}`,
        chrome,
        firefox,
        chromeSendRaw,
        firefoxSendRaw,
      });
      await assertInitialDamageWeapon(initialCase);
    }

    weaponResults = [...initialWeaponResults];
    damageResults = [...initialDamageResults];
    let lethalDamageObserved = false;
    await placeBrowsersInReadyCombat({
      label: "lethal setup",
      chrome,
      firefox,
      chromeSendRaw,
      firefoxSendRaw,
      minEnergy: 100,
    });
    for (let attempt = 0; attempt < 4 && !lethalDamageObserved; attempt += 1) {
      const [beforeShooterState, beforeDamagedState] = await Promise.all([
        chrome.pageState(),
        firefoxPageState(firefox),
      ]);
      const serverDamageStart = parseServerDamageEvents(server.logs).length;
      await sendPositionWeaponForTest({
        sender: chrome,
        sendRaw: chromeSendRaw,
        weapon: 3,
        count: 1,
        readSenderState: () => chrome.pageState(),
      });
      await sleep(300);
      const afterWeaponState = await chrome.pageState();
      const afterWeaponSummary = summarizeClient(afterWeaponState);
      const sentWeapon =
        [...afterWeaponSummary.sentPositionWeapons]
          .reverse()
          .find((entry) => bombWeaponMatches(entry) && entry.channel === "zubspace-events") ??
        { weapon: 3 };
      const weaponResult = {
        sender: clientReport(afterWeaponSummary),
        sentWeapon,
        serverAuthoritativeOnly: true,
      };
      weaponResults.push({
        phase: "pre-movement",
        direction: "chrome-to-firefox",
        label: `lethal-bomb-${attempt + 1}`,
        ...weaponResult,
      });
      const serverEvent = await waitForServerDamageEvent({
        server,
        sinceCount: serverDamageStart,
        label: `Chrome lethal bomb ${attempt + 1} damage on Firefox`,
        shooterId: initialChromeSummary.selfId,
        damagedId: initialFirefoxSummary.selfId,
        weaponEntry: weaponResult.sentWeapon,
      });
      if (serverEvent.lethal) {
        lethalDamageObserved = true;
        damageResults.push({
          phase: "pre-movement",
          direction: "chrome-to-firefox",
          label: `lethal-bomb-${attempt + 1}`,
          server: serverEvent,
          beforeRemoteEnergy: beforeShooterState.uiMetrics?.remoteEnergy,
          beforeDamagedEnergy: beforeDamagedState.uiMetrics?.playerEnergy,
          lethal: true,
        });
        lethalResults.push({
          direction: "chrome-to-firefox",
          label: "authoritative-death-score",
          death: await assertDeathAndScoreSync({
            label: "Chrome authoritative lethal damage on Firefox",
            shooterId: initialChromeSummary.selfId,
            damagedId: initialFirefoxSummary.selfId,
            readShooterState: () => chrome.pageState(),
            readDamagedState: () => firefoxPageState(firefox),
          }),
        });
      } else {
        const damage = await assertAuthoritativeDamageSync({
          label: `Chrome lethal bomb ${attempt + 1} damage on Firefox`,
          event: serverEvent,
          readShooterState: () => chrome.pageState(),
          readDamagedState: () => firefoxPageState(firefox),
          beforeShooterState,
          beforeDamagedState,
        });
        damageResults.push({
          phase: "pre-movement",
          direction: "chrome-to-firefox",
          label: `lethal-bomb-${attempt + 1}`,
          ...damage,
        });
        await sleep(700);
      }
    }
    if (!lethalDamageObserved) {
      throw new Error("Chrome did not produce controlled authoritative lethal bomb damage on Firefox");
    }
    await waitUntil("post-lethal respawn visibility", async () => {
      const [chromeReady, firefoxReady] = await Promise.all([
        chrome.liveState(),
        executeFirefox(firefox, `return ${smokeLiveStateExpression};`),
      ]);
      return (
        smokeGameplayReady(chromeReady.uiMetrics) &&
        smokeGameplayReady(firefoxReady.uiMetrics) &&
        mixedUiMetricsOk(chromeReady.uiMetrics) &&
        mixedUiMetricsOk(firefoxReady.uiMetrics) &&
        (chromeReady.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
        (firefoxReady.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
        (chromeReady.uiMetrics?.playerEnergy ?? 0) > 0 &&
        (firefoxReady.uiMetrics?.playerEnergy ?? 0) > 0
      );
    }, 10_000);
    }

    const [chromeDrive, firefoxRemoteRender] = await Promise.all([
      chrome.driveAndRelease(),
      sampleRemoteRenderMotion("Firefox observing Chrome", () =>
        executeFirefox(firefox, "return window.__zubspaceUiMetrics || null;"),
      ),
    ]);
    await sleep(400);
    const [firefoxDrive, chromeRemoteRender] = await Promise.all([
      driveFirefoxAndRelease(firefox),
      sampleRemoteRenderMotion("Chrome observing Firefox", () =>
        chrome.evaluate("window.__zubspaceUiMetrics || null"),
      ),
    ]);
    const visualResults = {
      chromeObservingFirefox: assertRemoteRenderMotion("Chrome observing Firefox", chromeRemoteRender),
      firefoxObservingChrome: assertRemoteRenderMotion("Firefox observing Chrome", firefoxRemoteRender),
    };
    const sampleStart = Date.now();
    await sleep(Number.isFinite(sampleMs) ? Math.max(sampleMs, 0) : 3000);
    const sampleElapsedMs = Date.now() - sampleStart;

    const [chromeState, firefoxState] = await Promise.all([
      chrome.pageState(),
      firefoxPageState(firefox),
    ]);
    assertMixedUiMetrics("Chrome final", chromeState);
    assertMixedUiMetrics("Firefox final", firefoxState);
    assertRemoteVisible("Chrome", chromeState);
    assertRemoteVisible("Firefox", firefoxState);

    const chromeSummary = summarizeClient(chromeState);
    const firefoxSummary = summarizeClient(firefoxState);
    if (chromeSummary.selfId == null || firefoxSummary.selfId == null) {
      throw new Error(`missing assigned player ids: ${JSON.stringify({
        chromeSelfId: chromeSummary.selfId,
        firefoxSelfId: firefoxSummary.selfId,
      })}`);
    }
    assertCoastingPositionStream("chrome local", chromeSummary.sentPositions, chromeDrive.releasedAt);
    assertCoastingPositionStream("firefox local", firefoxSummary.sentPositions, firefoxDrive.releasedAt);
    if (smokeCombat) {
      assertCoastingPositionRelay("chrome receiving firefox", chromeSummary.receivedPositionUpdates, firefoxDrive.releasedAt);
      assertCoastingPositionRelay("firefox receiving chrome", firefoxSummary.receivedPositionUpdates, chromeDrive.releasedAt);
    }
    if (smokeBotCount > 0) {
      assertServerBotPositions("Chrome", chromeSummary, smokeBotCount);
      assertServerBotPositions("Firefox", firefoxSummary, smokeBotCount);
    }

    const seriousChromeEvents = chrome.events.filter((event) => event.type === "error" || event.type === "exception");
    if (seriousChromeEvents.length > 0) {
      throw new Error(`Chrome console errors: ${JSON.stringify(seriousChromeEvents)}`);
    }

    let pressureStatus = null;
    await waitUntil("mixed RTC ingress queues drain", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) return null;
      const status = await response.json();
      try {
        const audit = assertRtcPressureClean(status, "mixed smoke final");
        pressureStatus = status;
        return audit;
      } catch {
        return null;
      }
    }, 5_000);
    const pressureAudit = assertRtcPressureClean(pressureStatus, "mixed smoke final");
    const serverLogAudit = assertRtcServerLogClean(server.logs, "mixed smoke final");

    console.log(JSON.stringify({
      ok: true,
      transport: "rtc",
      browserPair: "chrome-firefox",
      arena: "chaos-zone",
      tickRate: Number(smokeTickRate),
      botCount: smokeBotCount,
      combatAssertions: smokeCombat,
      sampleElapsedMs,
      chrome: {
        ...clientReport(chromeSummary),
        uiMetrics: chromeState.uiMetrics,
      },
      firefox: {
        ...clientReport(firefoxSummary),
        uiMetrics: firefoxState.uiMetrics,
      },
      visualResults,
      weaponResults,
      damageResults,
      lethalResults,
      pressureAudit,
      serverLogAudit,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(-20),
    }, null, 2));
  } catch (error) {
    // nix-shell removes its TMPDIR on exit. Use the retained run root (or
    // stable /tmp), so the printed failure location survives that cleanup.
    const failureRoot = process.env.ZUBSPACE_E2E_ARTIFACT_ROOT || "/tmp";
    fs.mkdirSync(failureRoot, { recursive: true });
    const failure = fs.mkdtempSync(path.join(failureRoot, "zubspace-mixed-rtc-failure-"));
    fs.writeFileSync(path.join(failure, "error.txt"), `${error.stack || error.message}\n`);
    fs.writeFileSync(path.join(failure, "server.log"), server.logs.join(""));
    const [chromeState, firefoxState, chromeImage, firefoxImage] = await Promise.all([
      readFailureDiagnostic(() => chrome?.pageState()),
      readFailureDiagnostic(() => firefox.sessionId ? firefoxPageState(firefox) : null),
      readFailureDiagnostic(() => chrome?.screenshot()),
      readFailureDiagnostic(() => firefox.sessionId ? screenshotFirefox(firefox) : null),
    ]);
    fs.writeFileSync(path.join(failure, "chrome-state.json"), `${JSON.stringify(chromeState, null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "firefox-state.json"), `${JSON.stringify(firefoxState, null, 2)}\n`);
    const captureStatus = {};
    for (const [label, value] of [["chrome", chromeImage], ["firefox", firefoxImage]]) {
      if (typeof value === "string") {
        fs.writeFileSync(path.join(failure, `${label}.png`), Buffer.from(value, "base64"));
        captureStatus[label] = { screenshot: true };
      } else captureStatus[label] = value;
    }
    const provenance = collectArtifactProvenance({ e2eScript: "tools/browser_mixed_rtc_smoke.mjs", runStartedAt });
    fs.writeFileSync(path.join(failure, "provenance.json"), `${JSON.stringify(provenance, null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "result.json"), `${JSON.stringify({
      ok: false, failure: error.message, captureStatus, provenance,
      artifactFiles: artifactFileManifest(failure),
    }, null, 2)}\n`);
    // Keep the CLI bounded so immediate process exit cannot truncate the
    // diagnostic location behind a multi-megabyte DEBUG server log.
    error.stack = `${error.name}: ${String(error.message).split("\n")[0].slice(0, 1500)}\nretained mixed RTC failure: ${failure}`;
    throw error;
  } finally {
    await chrome?.close().catch(() => {});
    await deleteSession(firefox).catch(() => {});
    await Promise.all([
      stopProcess(chromeProcess.child),
      stopProcess(firefox.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(chromeProcess.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
