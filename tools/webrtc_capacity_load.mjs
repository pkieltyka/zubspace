#!/usr/bin/env node
import { fetchGameStatusResponse } from "./game_status.mjs";

import dgram from "node:dgram";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import "../js/zubspace_rtc_config.js";
import {
  MAX_PLAYER_CLIENTS,
  assertRtcUdpPortOwnership,
  capacityBrowserUrl,
  capacityExpectedCandidatePaths,
  parseCapacityClientCount,
} from "./webrtc_capacity_config.mjs";
import {
  capacityCdpRpcTimeoutMs,
  capacitySignalingExchange,
  evaluateCdpExpression,
  fetchJsonWithDeadline,
  runWithCleanup,
} from "./webrtc_capacity_runner.mjs";
import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
} from "./rtc_gate_audit.mjs";

const serverBin = "target/release/zubspace_server";
const clientCount = parseCapacityClientCount(process.env.ZUBSPACE_RTC_LOAD_CLIENTS || "200");
const holdSeconds = Number(process.env.ZUBSPACE_RTC_LOAD_HOLD_SECONDS || 10);
const offerBatchSize = Number(process.env.ZUBSPACE_RTC_LOAD_OFFER_BATCH || clientCount);
const serverLogLevel = process.env.ZUBSPACE_RTC_LOAD_LOG_LEVEL || "warn";
const iceUrls = (process.env.ZUBSPACE_RTC_LOAD_ICE_URLS || "")
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const iceUsername = process.env.ZUBSPACE_RTC_LOAD_ICE_USERNAME || "";
const iceCredential = process.env.ZUBSPACE_RTC_LOAD_ICE_CREDENTIAL || "";
const icePolicy = process.env.ZUBSPACE_RTC_LOAD_ICE_POLICY || "all";
const externalOfferUrl = (process.env.ZUBSPACE_RTC_LOAD_OFFER_URL || "").trim();
const externalStatusUrl = (process.env.ZUBSPACE_RTC_LOAD_STATUS_URL || "").trim();
const externalBrowserUrl = (process.env.ZUBSPACE_RTC_LOAD_BROWSER_URL || "").trim();
const signalingToken = process.env.ZUBSPACE_RTC_LOAD_SIGNALING_TOKEN || "";
const signalingTimeoutMs = globalThis.zubspaceRtcSignalingTimeoutMs(
  process.env.ZUBSPACE_RTC_LOAD_SIGNALING_TIMEOUT_MS,
);
const iceGatheringTimeoutMs = globalThis.zubspaceRtcIceGatheringTimeoutMs(
  process.env.ZUBSPACE_RTC_LOAD_ICE_GATHER_TIMEOUT_MS,
);
const waitForIceGatheringCompleteSource =
  globalThis.zubspaceWaitForIceGatheringComplete.toString();
const createRtcSignalingDeadlineSource =
  globalThis.zubspaceCreateRtcSignalingDeadline.toString();
const capacitySignalingExchangeSource = capacitySignalingExchange.toString();
const expectedCandidatePaths = capacityExpectedCandidatePaths(
  process.env.ZUBSPACE_RTC_LOAD_EXPECT_CANDIDATE_PATHS || "",
  { icePolicy, remoteMode: Boolean(externalOfferUrl) },
);
const configuredBrowserUrl = capacityBrowserUrl(externalBrowserUrl, {
  remoteMode: Boolean(externalOfferUrl),
});
const chromeCandidates = process.env.CHROME_BIN
  ? [process.env.CHROME_BIN]
  : ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"];

if (!Number.isFinite(holdSeconds) || holdSeconds < 1 || holdSeconds > 600) {
  throw new Error(`ZUBSPACE_RTC_LOAD_HOLD_SECONDS must be in 1..600, got ${holdSeconds}`);
}
if (!Number.isSafeInteger(offerBatchSize) || offerBatchSize < 1) {
  throw new Error(`ZUBSPACE_RTC_LOAD_OFFER_BATCH must be a positive integer`);
}
if (offerBatchSize > MAX_PLAYER_CLIENTS) {
  throw new Error(`ZUBSPACE_RTC_LOAD_OFFER_BATCH cannot exceed ${MAX_PLAYER_CLIENTS}`);
}
const iceParams = new URLSearchParams({
  ice_username: iceUsername,
  ice_credential: iceCredential,
  ice_policy: icePolicy,
});
for (const url of iceUrls) {
  iceParams.append("ice", url);
}
const browserRtcConfiguration = globalThis.zubspaceRtcConfiguration(`?${iceParams}`);
const cdpRpcTimeoutMs = capacityCdpRpcTimeoutMs({
  raw: process.env.ZUBSPACE_RTC_LOAD_CDP_RPC_TIMEOUT_MS,
  signalingTimeoutMs,
  holdMs: Math.round(holdSeconds * 1000),
  clientCount,
  batchSize: offerBatchSize,
});

async function freeTcpPort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function udpRangeAvailable(base, count) {
  const sockets = [];
  try {
    for (let offset = 0; offset < count; offset += 1) {
      const socket = dgram.createSocket("udp4");
      sockets.push(socket);
      await new Promise((resolve, reject) => {
        socket.once("error", reject);
        socket.bind(base + offset, "0.0.0.0", resolve);
      });
    }
    return true;
  } catch {
    return false;
  } finally {
    await Promise.all(
      sockets.map(
        (socket) =>
          new Promise((resolve) => {
            try {
              socket.close(resolve);
            } catch {
              resolve();
            }
          }),
      ),
    );
  }
}

async function findUdpRange(count) {
  const configured = Number(process.env.ZUBSPACE_RTC_LOAD_UDP_BASE || 0);
  // Stay below Linux's usual ephemeral-client range (32768+) so Chrome's
  // concurrently-created ICE sockets cannot claim a just-preflighted server
  // port between server startup and the final offer batch.
  const automaticBaseMin = 12000;
  const automaticBaseSpan = 30000 - automaticBaseMin - count + 1;
  const candidates = configured
    ? [configured]
    : Array.from(
        { length: 24 },
        (_, index) =>
          automaticBaseMin + ((process.pid * 997 + index * 1237) % automaticBaseSpan),
      );
  for (const base of candidates) {
    if (base > 0 && base + count - 1 <= 65535 && (await udpRangeAvailable(base, count))) {
      return { min: base, max: base + count - 1 };
    }
  }
  throw new Error(`could not find ${count} contiguous free UDP ports`);
}

async function waitUntil(label, fn, timeoutMs = 30000) {
  const started = Date.now();
  let lastError;
  while (Date.now() - started < timeoutMs) {
    const remainingMs = Math.max(1, timeoutMs - (Date.now() - started));
    try {
      const value = await fn(remainingMs);
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`);
}

function startServer(port, udpRange) {
  if (!fs.existsSync(serverBin)) {
    throw new Error(`missing ${serverBin}; run cargo build --release --bin zubspace_server first`);
  }
  const child = spawn(serverBin, [
    "--bind",
    `127.0.0.1:${port}`,
    "--zone",
    "chaos-zone",
    "--max-players",
    String(clientCount),
    "--tick-rate",
    "60",
    "--log-level",
    serverLogLevel,
    "--bot-count",
    "0",
    "--rtc-udp-port-range",
    `${udpRange.min}-${udpRange.max}`,
    "--signaling-offer-rate",
    String(clientCount + 20),
  ], {
    env: {
      ...process.env,
      ZUBSPACE_SERVER_RTC_ICE_URLS: iceUrls.join(","),
      ZUBSPACE_SERVER_RTC_ICE_USERNAME: iceUsername,
      ZUBSPACE_SERVER_RTC_ICE_CREDENTIAL: iceCredential,
      ZUBSPACE_SERVER_RTC_ICE_POLICY: icePolicy,
    },
  });
  const logs = [];
  child.stdout.on("data", (data) => logs.push(data.toString()));
  child.stderr.on("data", (data) => logs.push(data.toString()));
  return { child, logs };
}

function startChrome(debugPort) {
  const userDataDir = path.join(os.tmpdir(), `zubspace-webrtc-capacity-${process.pid}`);
  fs.rmSync(userDataDir, { force: true, recursive: true });
  for (const binary of chromeCandidates) {
    const probe = spawnSync("sh", ["-c", `command -v "$1"`, "sh", binary], {
      encoding: "utf8",
    });
    if (probe.status !== 0) continue;
    const child = spawn(binary, [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--no-first-run",
      "--password-store=basic",
      "--remote-debugging-address=127.0.0.1",
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${userDataDir}`,
      "about:blank",
    ]);
    child.stdout.on("data", () => {});
    child.stderr.on("data", () => {});
    return { child, userDataDir, binary };
  }
  throw new Error(`no Chrome/Chromium executable found (${chromeCandidates.join(", ")})`);
}

async function stopChild(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  await Promise.race([exited, sleep(1500)]);
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
}

async function chromeJson(debugPort, pathname, request = {}, timeoutMs = signalingTimeoutMs) {
  return fetchJsonWithDeadline(`http://127.0.0.1:${debugPort}${pathname}`, {
    timeoutMs,
    label: `Chrome ${pathname}`,
    request,
  });
}

async function openChromePage(debugPort, browserUrl) {
  await waitUntil("Chrome debugging endpoint", (remainingMs) =>
    chromeJson(
      debugPort,
      "/json/version",
      {},
      Math.min(signalingTimeoutMs, remainingMs),
    ),
  );
  const created = await chromeJson(
    debugPort,
    `/json/new?${encodeURIComponent(browserUrl)}`,
    { method: "PUT" },
  );
  if (created.webSocketDebuggerUrl) return created.webSocketDebuggerUrl;
  const pages = await chromeJson(debugPort, "/json/list");
  const page = pages.find((entry) => entry.type === "page" && entry.url === browserUrl);
  if (!page?.webSocketDebuggerUrl) throw new Error("Chrome did not expose a debuggable page");
  return page.webSocketDebuggerUrl;
}

async function evaluateInChrome(webSocketDebuggerUrl, expression) {
  return evaluateCdpExpression(webSocketDebuggerUrl, expression, {
    timeoutMs: cdpRpcTimeoutMs,
  });
}

export function loadExpression(offerUrl, {
  count = clientCount,
  batchSize = offerBatchSize,
  holdMs = Math.round(holdSeconds * 1000),
} = {}) {
  parseCapacityClientCount(String(count));
  if (!Number.isSafeInteger(batchSize) || batchSize < 1 || batchSize > MAX_PLAYER_CLIENTS) {
    throw new Error("capacity expression batch size is out of bounds");
  }
  if (!Number.isSafeInteger(holdMs) || holdMs < 1000 || holdMs > 600000) {
    throw new Error("capacity expression hold must be 1000..600000 ms");
  }
  normalizedHttpUrl("capacity offer URL", offerUrl);
  return `
(async () => {
  const count = ${count};
  const batchSize = ${batchSize};
  const holdMs = ${holdMs};
  const offerUrl = ${JSON.stringify(offerUrl)};
  const signalingToken = ${JSON.stringify(signalingToken)};
  const rtcConfiguration = ${JSON.stringify(browserRtcConfiguration)};
  const iceGatheringTimeoutMs = ${iceGatheringTimeoutMs};
  const signalingTimeoutMs = ${signalingTimeoutMs};
  const waitForIceGatheringComplete = (${waitForIceGatheringCompleteSource});
  const createRtcSignalingDeadline = (${createRtcSignalingDeadlineSource});
  const capacitySignalingExchange = (${capacitySignalingExchangeSource});
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const peers = [];
  const handshakeMs = [];
  let receivedMessages = 0;
  let receivedBytes = 0;
  let receivedSnapshots = 0;
  let serverReliableAcksSent = 0;
  let snapshotAcksSent = 0;
  let fullSnapshotsReceived = 0;
  let deltaSnapshotsReceived = 0;
  let snapshotIdGaps = 0;
  const lastSnapshotIds = new Array(count).fill(null);
  const snapshotCounts = new Array(count).fill(0);
  const deltaSnapshotCounts = new Array(count).fill(0);
  const u32le = (value) => {
    const bytes = new Uint8Array(4);
    new DataView(bytes.buffer).setUint32(0, value >>> 0, true);
    return bytes;
  };
  const reliablePacket = (id, payload) => {
    const packet = new Uint8Array(6 + payload.length);
    packet[0] = 0;
    packet[1] = 3;
    packet.set(u32le(id), 2);
    packet.set(payload, 6);
    return packet;
  };
  const loginPacket = (index) => {
    const packet = new Uint8Array(165);
    packet[0] = 0x24;
    packet.set(new TextEncoder().encode("load" + index).slice(0, 32), 2);
    const identity = 100000000000n + BigInt(index);
    const view = new DataView(packet.buffer);
    view.setUint32(66, Number(identity & 0xffffffffn), true);
    view.setUint32(89, Number((identity >> 32n) & 0xffffffffn), true);
    return packet;
  };
  async function connect(index) {
    const started = performance.now();
    const peer = new RTCPeerConnection(rtcConfiguration);
    const game = peer.createDataChannel("zubspace-game", { ordered: false, maxPacketLifeTime: 80 });
    const events = peer.createDataChannel("zubspace-events", { ordered: false });
    const reliable = peer.createDataChannel("zubspace-reliable", { ordered: true });
    let nextReliableMessageId = 0;
    const sendReliablePayload = (payload) => {
      reliable.send(reliablePacket(nextReliableMessageId, payload));
      nextReliableMessageId = (nextReliableMessageId + 1) >>> 0;
    };
    for (const channel of [game, events, reliable]) {
      channel.binaryType = "arraybuffer";
      channel.onmessage = (event) => {
        receivedMessages += 1;
        receivedBytes += typeof event.data === "string" ? event.data.length : event.data.byteLength;
        if (typeof event.data !== "string") {
          const bytes = new Uint8Array(event.data);
          if (
            channel === reliable &&
            bytes.length >= 6 &&
            bytes[0] === 0x00 &&
            bytes[1] === 0x03 &&
            reliable.readyState === "open"
          ) {
            const ack = new Uint8Array(6);
            ack[0] = 0x00;
            ack[1] = 0x04;
            ack.set(bytes.subarray(2, 6), 2);
            reliable.send(ack);
            serverReliableAcksSent += 1;
          }
          if (bytes.length >= 4 && bytes[0] === 0x70) {
            const snapshotId = bytes[2] | (bytes[3] << 8);
            const previous = lastSnapshotIds[index];
            if (previous !== null) {
              const delta = (snapshotId - previous + 65536) % 65536;
              if (delta > 1) snapshotIdGaps += delta - 1;
            }
            lastSnapshotIds[index] = snapshotId;
            receivedSnapshots += 1;
            snapshotCounts[index] += 1;
            if ((bytes[1] & 1) !== 0) {
              fullSnapshotsReceived += 1;
            } else {
              deltaSnapshotsReceived += 1;
              deltaSnapshotCounts[index] += 1;
            }
            if (reliable.readyState === "open") {
              sendReliablePayload(new Uint8Array([0x71, snapshotId & 0xff, snapshotId >>> 8]));
              snapshotAcksSent += 1;
            }
          }
        }
      };
    }
    try {
      await peer.setLocalDescription(await peer.createOffer());
      await waitForIceGatheringComplete(peer, iceGatheringTimeoutMs);
      await capacitySignalingExchange({
        peer,
        offerUrl,
        signalingToken,
        timeoutMs: signalingTimeoutMs,
        createDeadline: createRtcSignalingDeadline,
        fetchImpl: fetch,
      });
      const deadline = performance.now() + 20000;
      while (
        (game.readyState !== "open" || events.readyState !== "open" || reliable.readyState !== "open") &&
        performance.now() < deadline
      ) await wait(20);
      if (game.readyState !== "open" || events.readyState !== "open" || reliable.readyState !== "open") {
        throw new Error("channels did not all open");
      }
      sendReliablePayload(loginPacket(index));
      await wait(20);
      sendReliablePayload(new Uint8Array([1, 0]));
      handshakeMs.push(performance.now() - started);
      peers.push(peer);
    } catch (error) {
      peer.close();
      throw new Error("peer " + index + " setup failed: " + error.message);
    }
  }
  const overallStarted = performance.now();
  for (let start = 0; start < count; start += batchSize) {
    await Promise.all(
      Array.from({ length: Math.min(batchSize, count - start) }, (_, offset) => connect(start + offset)),
    );
  }
  const allOpenMs = performance.now() - overallStarted;
  await wait(holdMs);
  let bytesSent = 0;
  let bytesReceived = 0;
  const roundTripTimes = [];
  let peersWithSelectedCandidatePair = 0;
  const selectedCandidatePaths = {};
  for (const peer of peers) {
    const stats = await peer.getStats();
    const reports = [...stats.values()];
    const selectedPairId = reports.find(
      (report) => report.type === "transport" && report.selectedCandidatePairId,
    )?.selectedCandidatePairId;
    const pair =
      (selectedPairId ? stats.get(selectedPairId) : null) ||
      reports.find(
        (report) =>
          report.type === "candidate-pair" && report.state === "succeeded" && report.nominated,
      );
    if (!pair) continue;
    const localCandidate = stats.get(pair.localCandidateId);
    const remoteCandidate = stats.get(pair.remoteCandidateId);
    if (!localCandidate?.candidateType || !remoteCandidate?.candidateType) continue;
    const candidatePath = localCandidate.candidateType + "->" + remoteCandidate.candidateType;
    selectedCandidatePaths[candidatePath] = (selectedCandidatePaths[candidatePath] || 0) + 1;
    peersWithSelectedCandidatePair += 1;
    bytesSent += pair.bytesSent || 0;
    bytesReceived += pair.bytesReceived || 0;
    if (Number.isFinite(pair.currentRoundTripTime)) roundTripTimes.push(pair.currentRoundTripTime);
  }
  const sorted = [...handshakeMs].sort((a, b) => a - b);
  const percentile = (values, fraction) => values[Math.min(values.length - 1, Math.floor(values.length * fraction))] || 0;
  const rttSorted = roundTripTimes.sort((a, b) => a - b);
  window.__zubspaceCapacityPeers = peers;
  // A combined-load observer may sample continuing background receipt after
  // setup. These counters confer no gameplay authority and send no input.
  window.__zubspaceCapacityReceiptCounts = () => ({
    snapshots: [...snapshotCounts], deltas: [...deltaSnapshotCounts],
    openPeers: peers.filter(peer => peer.connectionState === "connected").length,
  });
  return {
    connected: peers.length,
    allOpenMs,
    handshakeMs: {
      average: handshakeMs.reduce((sum, value) => sum + value, 0) / handshakeMs.length,
      p95: percentile(sorted, 0.95),
      p99: percentile(sorted, 0.99),
      max: sorted[sorted.length - 1] || 0,
    },
    receivedMessages,
    receivedBytes,
    serverReliableAcksSent,
    snapshots: {
      received: receivedSnapshots,
      acksSent: snapshotAcksSent,
      fullReceived: fullSnapshotsReceived,
      deltaReceived: deltaSnapshotsReceived,
      idGaps: snapshotIdGaps,
      observedGapRate: snapshotIdGaps / Math.max(1, receivedSnapshots + snapshotIdGaps),
      peersReceiving: snapshotCounts.filter((value) => value > 0).length,
      peersReceivingDeltas: deltaSnapshotCounts.filter((value) => value > 0).length,
      minimumPerPeer: Math.min(...snapshotCounts),
      maximumPerPeer: Math.max(...snapshotCounts),
    },
    candidatePairBytes: { sent: bytesSent, received: bytesReceived },
    selectedCandidatePairs: {
      peers: peersWithSelectedCandidatePair,
      paths: selectedCandidatePaths,
    },
    rttSeconds: {
      samples: rttSorted.length,
      p95: percentile(rttSorted, 0.95),
      max: rttSorted[rttSorted.length - 1] || 0,
    },
  };
})()`;
}

function processSample(pid) {
  const status = fs.readFileSync(`/proc/${pid}/status`, "utf8");
  const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8").trim().split(/\s+/);
  const value = (name) => Number(status.match(new RegExp(`^${name}:\\s+(\\d+)`, "m"))?.[1] || 0);
  return {
    at: Date.now(),
    cpuTicks: Number(stat[13]) + Number(stat[14]),
    rssKiB: value("VmRSS"),
    highWaterRssKiB: value("VmHWM"),
    threads: value("Threads"),
  };
}

function normalizedHttpUrl(name, value) {
  let url;
  try {
    url = new URL(value);
  } catch (error) {
    throw new Error(`${name} must be an absolute HTTP(S) URL: ${error.message}`);
  }
  if (!new Set(["http:", "https:"]).has(url.protocol)) {
    throw new Error(`${name} must use HTTP or HTTPS, got ${url.protocol}`);
  }
  return url.href;
}

async function main() {
  const chromeDebugPort = await freeTcpPort();
  const remoteMode = Boolean(externalOfferUrl);
  let server = null;
  let logs = [];
  let udpRange = null;
  let offerUrl;
  let statusUrl;
  let browserUrl;
  if (remoteMode) {
    offerUrl = normalizedHttpUrl("ZUBSPACE_RTC_LOAD_OFFER_URL", externalOfferUrl);
    statusUrl = externalStatusUrl
      ? normalizedHttpUrl("ZUBSPACE_RTC_LOAD_STATUS_URL", externalStatusUrl)
      : new URL("/v1/status", offerUrl).href;
    browserUrl = normalizedHttpUrl("ZUBSPACE_RTC_LOAD_BROWSER_URL", configuredBrowserUrl);
  } else {
    const serverPort = await freeTcpPort();
    udpRange = await findUdpRange(clientCount);
    const started = startServer(serverPort, udpRange);
    server = started.child;
    logs = started.logs;
    offerUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    statusUrl = `http://127.0.0.1:${serverPort}/v1/status`;
    browserUrl = configuredBrowserUrl;
  }
  let chrome;
  await runWithCleanup(async () => {
    const statusBefore = await waitUntil("server status endpoint", async (remainingMs) => {
      if (server && server.exitCode !== null) throw new Error(logs.join(""));
      return fetchJsonWithDeadline(statusUrl, {
        fetchImpl: fetchGameStatusResponse,
        timeoutMs: Math.min(signalingTimeoutMs, remainingMs),
        label: "server status endpoint",
      });
    });
    assertRtcUdpPortOwnership(statusBefore, 0, "before load");
    if (
      statusBefore.rtc_sessions !== 0 ||
      statusBefore.clients !== 0 ||
      statusBefore.rtc_udp_ports_degraded !== 0
    ) {
      throw new Error(
        `capacity/NAT validation requires an idle, non-degraded server: ${JSON.stringify(statusBefore)}`,
      );
    }
    if (statusBefore.rtc_effective_capacity < clientCount) {
      throw new Error(
        `server effective RTC capacity is below requested load: ${JSON.stringify(statusBefore)}`,
      );
    }
    const pressureBefore = assertRtcPressureClean(statusBefore, "capacity before load");
    const measuredStartedAt = Date.now();
    const before = server ? processSample(server.pid) : null;
    chrome = startChrome(chromeDebugPort);
    const page = await openChromePage(chromeDebugPort, browserUrl);
    if (remoteMode) {
      const expectedOrigin = new URL(browserUrl).origin;
      await waitUntil("external browser origin", async (remainingMs) =>
        evaluateCdpExpression(
          page,
          `location.origin === ${JSON.stringify(expectedOrigin)} && document.readyState === "complete"`,
          { timeoutMs: Math.min(cdpRpcTimeoutMs, remainingMs) },
        ),
      );
    }
    const result = await evaluateInChrome(page, loadExpression(offerUrl));
    if (result.connected !== clientCount) {
      throw new Error(`expected ${clientCount} connected peers, got ${result.connected}`);
    }
    const observedCandidatePaths = Object.keys(result.selectedCandidatePairs.paths);
    const unexpectedCandidatePaths = observedCandidatePaths.filter(
      (candidatePath) => !expectedCandidatePaths.includes(candidatePath),
    );
    if (
      result.selectedCandidatePairs.peers !== clientCount ||
      observedCandidatePaths.length === 0 ||
      unexpectedCandidatePaths.length > 0
    ) {
      throw new Error(
        `selected ICE candidate paths did not match ${expectedCandidatePaths.join(",")}: ${JSON.stringify(result.selectedCandidatePairs)}`,
      );
    }
    if (result.snapshots.peersReceiving !== clientCount) {
      const diagnosticStatus = await fetchJsonWithDeadline(statusUrl, {
        fetchImpl: fetchGameStatusResponse,
        timeoutMs: signalingTimeoutMs,
        label: "failed-load diagnostic status request",
      }).catch((error) => ({ error: error.message }));
      throw new Error(
        `not every peer received authoritative snapshots: ${JSON.stringify({
          receivedMessages: result.receivedMessages,
          receivedBytes: result.receivedBytes,
          snapshots: result.snapshots,
          status: diagnosticStatus,
          serverLogTail: logs.join("").slice(-64_000),
        })}`,
      );
    }
    if (result.serverReliableAcksSent === 0) {
      throw new Error("load peers did not ACK any server application-reliable join packets");
    }
    if (
      result.snapshots.acksSent === 0 ||
      result.snapshots.acksSent !== result.snapshots.received
    ) {
      throw new Error(
        `snapshot ACK traffic was not reliably enveloped for every received frame: ${JSON.stringify(result.snapshots)}`,
      );
    }
    if (
      result.snapshots.fullReceived === 0 ||
      result.snapshots.deltaReceived === 0 ||
      result.snapshots.peersReceivingDeltas !== clientCount
    ) {
      throw new Error(
        `load did not exercise initial FULL and acknowledged delta snapshots for every peer: ${JSON.stringify(result.snapshots)}`,
      );
    }
    const during = server ? processSample(server.pid) : null;
    const statusDuring = await waitUntil("steady-load ingress queues drain", async (remainingMs) => {
      const status = await fetchJsonWithDeadline(statusUrl, {
        fetchImpl: fetchGameStatusResponse,
        timeoutMs: Math.min(signalingTimeoutMs, remainingMs),
        label: "steady-load status request",
      });
      try {
        assertRtcPressureClean(status, "capacity steady load");
        return status;
      } catch {
        return null;
      }
    });
    assertRtcUdpPortOwnership(statusDuring, clientCount, "during steady load");
    if (statusDuring.rtc_sessions !== clientCount || statusDuring.clients !== clientCount) {
      throw new Error(`server did not retain full RTC load: ${JSON.stringify(statusDuring)}`);
    }
    if (statusDuring.snapshot_acked_clients !== clientCount) {
      throw new Error(
        `server did not observe snapshot ACKs from every load peer: ${JSON.stringify(statusDuring)}`,
      );
    }
    if (
      statusDuring.rtc_effective_capacity !== statusBefore.rtc_effective_capacity ||
      statusDuring.rtc_udp_ports_degraded !== 0
    ) {
      throw new Error(`server degraded RTC capacity during the load: ${JSON.stringify(statusDuring)}`);
    }
    if (
      statusDuring.rtc_outbound_reliable_overflows !== 0 ||
      statusDuring.rtc_channel_protocol_violations !== 0
    ) {
      throw new Error(
        `server overloaded or rejected a supported channel under steady load: ${JSON.stringify(statusDuring)}`,
      );
    }
    const pressureDuring = assertRtcPressureClean(statusDuring, "capacity steady load");
    await evaluateInChrome(
      page,
      `(() => {
        for (const peer of window.__zubspaceCapacityPeers || []) peer.close();
        window.__zubspaceCapacityPeers = [];
        return true;
      })()`,
    );
    const statusAfter = await waitUntil(
      "RTC disconnect cleanup",
      async (remainingMs) => {
        const status = await fetchJsonWithDeadline(statusUrl, {
          fetchImpl: fetchGameStatusResponse,
          timeoutMs: Math.min(signalingTimeoutMs, remainingMs),
          label: "cleanup status request",
        });
        if (
          status.rtc_sessions !== 0 ||
          status.rtc_ingress_execution?.in_flight !== 0 ||
          status.rtc_ingress_execution?.waiting !== 0 ||
          status.clients !== 0 ||
          status.rtc_outbound_queue_packets !== 0 ||
          status.rtc_outbound_queue_bytes !== 0 ||
          status.rtc_outbound_reliable_overflows !== 0 ||
          status.rtc_channel_protocol_violations !== 0 ||
          status.rtc_udp_ports_degraded !== 0 ||
          status.rtc_effective_capacity !== statusBefore.rtc_effective_capacity
        ) {
          return null;
        }
        return assertRtcUdpPortOwnership(status, 0, "after cleanup");
      },
      20000,
    );
    const pressureAfter = assertRtcPressureClean(statusAfter, "capacity after cleanup", {requireIngressDrained: true});
    const serverLogAudit = assertRtcServerLogClean(logs, "capacity final");
    const after = server ? processSample(server.pid) : null;
    const elapsedSeconds = Math.max(
      ((during?.at || Date.now()) - (before?.at || measuredStartedAt)) / 1000,
      0.001,
    );
    let cpuPercent = null;
    if (before && during) {
      const ticksPerSecond = Number(
        spawnSync("getconf", ["CLK_TCK"], { encoding: "utf8" }).stdout.trim() || 100,
      );
      cpuPercent =
        (((during.cpuTicks - before.cpuTicks) / ticksPerSecond) / elapsedSeconds) * 100;
    }
    result.candidatePairBytes.approximateServerEgressBitsPerSecond =
      (result.candidatePairBytes.received * 8) / elapsedSeconds;
    result.candidatePairBytes.approximateServerIngressBitsPerSecond =
      (result.candidatePairBytes.sent * 8) / elapsedSeconds;
    console.log(
      JSON.stringify(
        {
          ok: true,
          measuredAt: new Date().toISOString(),
          host: {
            platform: `${os.platform()} ${os.release()} ${os.arch()}`,
            logicalCpus: os.cpus().length,
            memoryGiB: os.totalmem() / 1024 ** 3,
            chrome: chrome.binary,
          },
          server: {
            mode: remoteMode ? "external" : "local",
            binary: remoteMode ? null : serverBin,
            offerUrl,
            statusUrl,
            browserUrl,
            clientCount,
            holdSeconds,
            offerBatchSize,
            udpRange,
            ice: {
              urls: iceUrls,
              policy: icePolicy,
              credentialsConfigured: Boolean(iceUsername && iceCredential),
              gatheringTimeoutMs: iceGatheringTimeoutMs,
              expectedCandidatePaths,
            },
            cpuPercent,
            before,
            during,
            after,
          },
          transport: result,
          pressureAudit: {
            before: pressureBefore,
            during: pressureDuring,
            after: pressureAfter,
          },
          serverLogAudit,
          statusBefore,
          statusDuring,
          statusAfter,
        },
        null,
        2,
      ),
    );
  }, async () => {
    await stopChild(chrome?.child);
    if (chrome?.userDataDir) fs.rmSync(chrome.userDataDir, { force: true, recursive: true });
    await stopChild(server);
    if (server?.exitCode && server.exitCode !== 0) process.stderr.write(logs.join(""));
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(error.stack || error);
    process.exitCode = 1;
  });
}
