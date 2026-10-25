#!/usr/bin/env node
import { fetchGameStatusResponse } from "./game_status.mjs";

import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import "../js/zubspace_rtc_config.js";
import { browserLaunchEnvironment } from "./e2e_gate_hygiene.mjs";
import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
} from "./rtc_gate_audit.mjs";

const serverBin = "target/debug/zubspace_server";
const smokeLogLevel = process.env.ZUBSPACE_SMOKE_LOG_LEVEL || "debug";
const iceGatheringTimeoutMs = globalThis.zubspaceRtcIceGatheringTimeoutMs(
  process.env.ZUBSPACE_SMOKE_ICE_GATHER_TIMEOUT_MS,
);
const waitForIceGatheringCompleteSource =
  globalThis.zubspaceWaitForIceGatheringComplete.toString();
const chromeDebugPort = Number(process.env.CHROME_DEBUG_PORT || 9341);
const chromeCandidates = process.env.CHROME_BIN
  ? [process.env.CHROME_BIN]
  : ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"];

function u32le(value) {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value >>> 0, true);
  return bytes;
}

function reliable(id, payload) {
  const packet = new Uint8Array(6 + payload.length);
  packet[0] = 0x00;
  packet[1] = 0x03;
  packet.set(u32le(id), 2);
  packet.set(payload, 6);
  return packet;
}

function loginPacket(name, identity) {
  const packet = new Uint8Array(165);
  packet[0] = 0x24;
  packet.set(new TextEncoder().encode(name).slice(0, 32), 2);
  const value = BigInt(identity);
  const view = new DataView(packet.buffer);
  view.setUint32(66, Number(value & 0xffffffffn), true);
  view.setUint32(89, Number((value >> 32n) & 0xffffffffn), true);
  return packet;
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function waitUntil(label, fn, timeoutMs = 8000) {
  const started = Date.now();
  let lastError = null;
  while (Date.now() - started < timeoutMs) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`);
}

function startServer(port) {
  if (!fs.existsSync(serverBin)) {
    throw new Error(`missing ${serverBin}; run cargo build --bin zubspace_server first`);
  }
  const child = spawn(serverBin, [
    "--bind",
    `127.0.0.1:${port}`,
    "--zone",
    "chaos-zone",
    "--tick-rate",
    "40",
    "--log-level",
    smokeLogLevel,
    "--bot-count",
    "0",
  ]);
  const output = [];
  child.stdout.on("data", (data) => output.push(data.toString()));
  child.stderr.on("data", (data) => output.push(data.toString()));
  return { child, output };
}

async function stopChild(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  await Promise.race([exited, sleep(1500)]);
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
  }
}

function startChrome() {
  const userDataDir = path.join(os.tmpdir(), `zubspace-browser-rtc-smoke-${process.pid}`);
  fs.rmSync(userDataDir, { force: true, recursive: true });
  let lastError = null;
  for (const bin of chromeCandidates) {
    try {
      const child = spawn(bin, [
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        "--disable-dev-shm-usage",
        "--no-first-run",
        "--password-store=basic",
        "--remote-debugging-address=127.0.0.1",
        `--remote-debugging-port=${chromeDebugPort}`,
        `--user-data-dir=${userDataDir}`,
        "about:blank",
      ], {
        env: browserLaunchEnvironment(process.env, "rtc smoke Chrome"),
      });
      child.stdout.on("data", () => {});
      child.stderr.on("data", () => {});
      return { child, userDataDir };
    } catch (error) {
      lastError = error;
    }
  }
  throw new Error(`failed to start Chrome: ${lastError?.message || "no candidate worked"}`);
}

async function chromeJson(pathname, options = {}) {
  const response = await fetch(`http://127.0.0.1:${chromeDebugPort}${pathname}`, options);
  if (!response.ok) {
    throw new Error(`Chrome ${pathname} returned ${response.status}`);
  }
  return response.json();
}

async function openChromePage() {
  await waitUntil("Chrome debugging endpoint", () => chromeJson("/json/version"));
  await chromeJson(`/json/new?${encodeURIComponent("about:blank")}`, { method: "PUT" });
  const pages = await chromeJson("/json/list");
  const page = pages.find((entry) => entry.type === "page" && entry.url === "about:blank");
  if (!page?.webSocketDebuggerUrl) {
    throw new Error("Chrome did not expose a debuggable about:blank page");
  }
  return page.webSocketDebuggerUrl;
}

async function evaluateInChrome(webSocketDebuggerUrl, expression) {
  const socket = new WebSocket(webSocketDebuggerUrl);
  let nextId = 0;
  const pending = new Map();
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      pending.get(message.id)(message);
      pending.delete(message.id);
    }
  };
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  function send(method, params = {}) {
    const id = ++nextId;
    socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve) => pending.set(id, resolve));
  }
  await send("Runtime.enable");
  const result = await send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  socket.close();
  if (result.result.exceptionDetails) {
    throw new Error(result.result.exceptionDetails.text);
  }
  return result.result.result.value;
}

function rtcSmokeExpression({ offerUrl, login, arenaLogin }) {
  return `
(async () => {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const waitForIceGatheringComplete = (${waitForIceGatheringCompleteSource});
  const events = [];
  const received = [];
  const peer = new RTCPeerConnection({ iceServers: [] });
  const game = peer.createDataChannel("zubspace-game", { ordered: false, maxPacketLifeTime: 80 });
  const eventChannel = peer.createDataChannel("zubspace-events", { ordered: false });
  const reliable = peer.createDataChannel("zubspace-reliable", { ordered: true });
  for (const channel of [game, eventChannel, reliable]) {
    channel.binaryType = "arraybuffer";
    channel.onopen = () => events.push(channel.label + ":open");
    channel.onclose = () => events.push(channel.label + ":close");
    channel.onerror = () => events.push(channel.label + ":error");
    channel.onmessage = (event) => {
      const bytes = Array.from(new Uint8Array(event.data));
      received.push(bytes);
      events.push(channel.label + ":message:" + bytes.length + ":" + bytes.slice(0, 10).join(","));
    };
  }
  await peer.setLocalDescription(await peer.createOffer());
  await waitForIceGatheringComplete(peer, ${iceGatheringTimeoutMs});
  const response = await fetch(${JSON.stringify(offerUrl)}, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(peer.localDescription),
  });
  const answer = await response.json();
  events.push("offer-status:" + response.status);
  if (!response.ok) throw new Error(JSON.stringify(answer));
  await peer.setRemoteDescription(answer);
  const started = Date.now();
  while (reliable.readyState !== "open" && Date.now() - started < 8000) await wait(50);
  if (reliable.readyState !== "open") throw new Error("reliable channel did not open");
  reliable.send(new Uint8Array(${JSON.stringify([...login])}));
  await wait(500);
  reliable.send(new Uint8Array(${JSON.stringify([...arenaLogin])}));
  await wait(2500);
  peer.close();
  const payloads = received
    .filter((packet) => packet[0] === 0 && packet[1] === 3)
    .map((packet) => packet.slice(6));
  return JSON.stringify({
    events,
    gameChannel: {
      ordered: game.ordered,
      maxPacketLifeTime: game.maxPacketLifeTime,
      maxRetransmits: game.maxRetransmits,
    },
    eventChannel: {
      ordered: eventChannel.ordered,
      maxPacketLifeTime: eventChannel.maxPacketLifeTime,
      maxRetransmits: eventChannel.maxRetransmits,
    },
    reliableChannel: {
      ordered: reliable.ordered,
      maxPacketLifeTime: reliable.maxPacketLifeTime,
      maxRetransmits: reliable.maxRetransmits,
    },
    sawPasswordResponse: payloads.some((payload) => payload[0] === 0x0a && payload[1] === 0x00),
    sawArenaJoin: payloads.some((payload) => payload[0] === 0x02),
    sawAssignedPlayer: payloads.some((payload) => payload[0] === 0x01),
  });
})()`;
}

function rtcChannelContractExpression({ offerUrl, channelOptions }) {
  return `
(async () => {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const waitForIceGatheringComplete = (${waitForIceGatheringCompleteSource});
  const peer = new RTCPeerConnection({ iceServers: [] });
  const events = [];
  const game = peer.createDataChannel("zubspace-game", ${JSON.stringify(channelOptions.game)});
  const eventChannel = peer.createDataChannel("zubspace-events", ${JSON.stringify(channelOptions.event)});
  const reliable = peer.createDataChannel("zubspace-reliable", ${JSON.stringify(channelOptions.reliable)});
  for (const channel of [game, eventChannel, reliable]) {
    channel.onopen = () => events.push(channel.label + ":open");
    channel.onclose = () => events.push(channel.label + ":close");
    channel.onerror = () => events.push(channel.label + ":error");
  }
  await peer.setLocalDescription(await peer.createOffer());
  await waitForIceGatheringComplete(peer, ${iceGatheringTimeoutMs});
  const response = await fetch(${JSON.stringify(offerUrl)}, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(peer.localDescription),
  });
  const answer = await response.json();
  if (!response.ok) throw new Error(JSON.stringify(answer));
  await peer.setRemoteDescription(answer);
  const started = Date.now();
  while (
    !["failed", "closed", "disconnected"].includes(peer.connectionState) &&
    Date.now() - started < 5000
  ) await wait(25);
  const result = {
    events,
    peerState: peer.connectionState,
    game: {
      ordered: game.ordered,
      maxPacketLifeTime: game.maxPacketLifeTime,
      maxRetransmits: game.maxRetransmits,
    },
    event: {
      ordered: eventChannel.ordered,
      maxPacketLifeTime: eventChannel.maxPacketLifeTime,
      maxRetransmits: eventChannel.maxRetransmits,
    },
    reliable: {
      ordered: reliable.ordered,
      maxPacketLifeTime: reliable.maxPacketLifeTime,
      maxRetransmits: reliable.maxRetransmits,
    },
  };
  peer.close();
  return JSON.stringify(result);
})()`;
}

async function main() {
  const serverPort = await freePort();
  const { child: server, output } = startServer(serverPort);
  let chrome = null;
  try {
    await waitUntil("RTC signaling endpoint", async () => {
      const response = await fetch(`http://127.0.0.1:${serverPort}/rtc`);
      return response.ok;
    });
    chrome = startChrome();
    const pageWs = await openChromePage();
    const identity = String(100000000000 + Math.floor(Math.random() * 900000000000));
    const resultText = await evaluateInChrome(
      pageWs,
      rtcSmokeExpression({
        offerUrl: `http://127.0.0.1:${serverPort}/v1/rtc/offer`,
        login: reliable(0, loginPacket("rtcsmoke", identity)),
        arenaLogin: reliable(1, new Uint8Array([0x01, 0x00])),
      }),
    );
    const result = JSON.parse(resultText);
    if (!result.events.includes("zubspace-game:open")) {
      throw new Error(`game DataChannel did not open: ${JSON.stringify(result)}`);
    }
    if (
      result.gameChannel?.ordered !== false ||
      result.gameChannel?.maxPacketLifeTime !== 80 ||
      result.gameChannel?.maxRetransmits !== null
    ) {
      throw new Error(`game DataChannel policy changed: ${JSON.stringify(result.gameChannel)}`);
    }
    if (!result.events.includes("zubspace-reliable:open")) {
      throw new Error(`reliable DataChannel did not open: ${JSON.stringify(result)}`);
    }
    if (!result.events.includes("zubspace-events:open")) {
      throw new Error(`event DataChannel did not open: ${JSON.stringify(result)}`);
    }
    if (
      result.eventChannel?.ordered !== false ||
      result.eventChannel?.maxPacketLifeTime !== null ||
      result.eventChannel?.maxRetransmits !== null
    ) {
      throw new Error(`event DataChannel policy changed: ${JSON.stringify(result.eventChannel)}`);
    }
    if (
      result.reliableChannel?.ordered !== true ||
      result.reliableChannel?.maxPacketLifeTime !== null ||
      result.reliableChannel?.maxRetransmits !== null
    ) {
      throw new Error(
        `reliable DataChannel policy changed: ${JSON.stringify(result.reliableChannel)}`,
      );
    }
    if (!result.sawPasswordResponse || !result.sawAssignedPlayer || !result.sawArenaJoin) {
      throw new Error(`RTC login/join did not complete: ${JSON.stringify(result)}`);
    }
    const serverLog = output.join("");
    if (!/server rtc data channel open label="zubspace-game"/.test(serverLog)) {
      throw new Error("server did not log game DataChannel open");
    }
    if (!/server rtc data channel open label="zubspace-events"/.test(serverLog)) {
      throw new Error("server did not log event DataChannel open");
    }
    if (!/server login accepted/.test(serverLog) || !/server arena joined/.test(serverLog)) {
      throw new Error(`server did not log accepted login and join:\n${serverLog}`);
    }
    const validCleanupStatus = await waitUntil("valid Chrome peer cleanup", async () => {
      const status = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`).then((response) =>
        response.json()
      );
      return status.rtc_sessions === 0 && status.clients === 0 ? status : null;
    });
    const validPressureAudit = assertRtcPressureClean(
      validCleanupStatus,
      "valid Chrome channel contract cleanup",
    );

    const mismatchScenarios = [
      {
        name: "fully reliable game",
        options: {
          game: { ordered: false },
          event: { ordered: false },
          reliable: { ordered: true },
        },
      },
      {
        name: "partially reliable event",
        options: {
          game: { ordered: false, maxPacketLifeTime: 80 },
          event: { ordered: false, maxRetransmits: 2 },
          reliable: { ordered: true },
        },
      },
      {
        name: "partially reliable reliable",
        options: {
          game: { ordered: false, maxPacketLifeTime: 80 },
          event: { ordered: false },
          reliable: { ordered: true, maxPacketLifeTime: 80 },
        },
      },
    ];
    const mismatchResults = [];
    let expectedViolations = 0;
    for (const scenario of mismatchScenarios) {
      const mismatch = JSON.parse(
        await evaluateInChrome(
          pageWs,
          rtcChannelContractExpression({
            offerUrl: `http://127.0.0.1:${serverPort}/v1/rtc/offer`,
            channelOptions: scenario.options,
          }),
        ),
      );
      expectedViolations += 1;
      const status = await waitUntil(`${scenario.name} rejection cleanup`, async () => {
        const current = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`).then((response) =>
          response.json()
        );
        return current.rtc_channel_protocol_violations === expectedViolations &&
          current.rtc_sessions === 0 &&
          current.rtc_ingress_execution?.in_flight === 0 &&
          current.rtc_ingress_execution?.waiting === 0 &&
          current.clients === 0
          ? current
          : null;
      });
      mismatchResults.push({
        name: scenario.name,
        browser: mismatch,
        status,
        pressureAudit: assertRtcPressureClean(status, `${scenario.name} cleanup`, {requireIngressDrained: true}),
      });
    }
    const serverLogAudit = assertRtcServerLogClean(output, "Chrome channel contract final");
    console.log(
      JSON.stringify(
        {
          ok: true,
          transport: "rtc",
          serverPort,
          events: result.events,
          gameChannel: result.gameChannel,
          eventChannel: result.eventChannel,
          reliableChannel: result.reliableChannel,
          validPressureAudit,
          mismatchResults,
          serverLogAudit,
        },
        null,
        2,
      ),
    );
  } finally {
    await stopChild(chrome?.child);
    if (chrome?.userDataDir) {
      fs.rmSync(chrome.userDataDir, { force: true, recursive: true });
    }
    await stopChild(server);
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
