#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import { lifecycleIsReady } from "./lifecycle_readiness.mjs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  compactMetrics,
  openChromeClient,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import {
  artifactFileManifest,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { writeQaTraceArtifact, writeQaTraceFailureResult } from "./qa_trace_artifacts.mjs";
import {
  createSession,
  deleteSession,
  execute,
  firefoxKeyDown,
  firefoxKeyUp,
  firefoxArtifactClient,
  freePort,
  openClient,
  pageState,
  repoRoot,
  screenshot,
  sleep,
  startGeckodriver,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForGeckodriver,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";

const browser = process.env.ZUBSPACE_E2E_BROWSER || "chrome";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
const cycles = Number(process.env.ZUBSPACE_E2E_CYCLES || "10");
const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/startup_lifecycle_matrix.mjs";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const defaultArtifactDirectory = path.join(
  "/tmp",
  `zubspace-startup-lifecycle-${browser}-${arena}-${Date.now()}-${process.pid}`,
);
const artifactTarget = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR || defaultArtifactDirectory;

if (!new Set(["chrome", "firefox"]).has(browser)) {
  throw new Error("ZUBSPACE_E2E_BROWSER must be chrome or firefox");
}
if (!new Set(["chaos-zone", "extreme"]).has(arena)) {
  throw new Error("startup lifecycle matrix is scoped to chaos-zone and extreme");
}
if (!Number.isInteger(cycles) || cycles < 1) {
  throw new Error("ZUBSPACE_E2E_CYCLES must be a positive integer");
}
if (process.env.ZUBSPACE_SMOKE_BOT_COUNT !== "0") {
  throw new Error("startup lifecycle matrix requires ZUBSPACE_SMOKE_BOT_COUNT=0");
}

function traceRecords(server, marker) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes(marker))
    .map((line) => {
      const record = {};
      for (const match of line.matchAll(/(\w+)=("[^"]*"|\S+)/g)) {
        const raw = match[2];
        if (raw === "true" || raw === "false") record[match[1]] = raw === "true";
        else if (raw !== "" && Number.isFinite(Number(raw))) record[match[1]] = Number(raw);
        else record[match[1]] = raw.replace(/^"|"$/g, "");
      }
      return record;
    });
}

function compactLifecycleState(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return {
    metrics: compactMetrics(metrics),
    lifecycle: {
      zoneFilesReady: local.zoneFilesReady,
      zoneMapReady: local.zoneMapReady,
      networkPrizeBaselinePending: local.networkPrizeBaselinePending,
      lastSelfServerPrizeLifeSeq: local.lastSelfServerPrizeLifeSeq,
      lastSelfServerPrizeRevision: local.lastSelfServerPrizeRevision,
      lastSelfServerEnergy: local.lastSelfServerEnergy,
      pendingShipResetPosition: local.pendingShipResetPosition,
      pendingShipResetDeliveryId: local.pendingShipResetDeliveryId,
      pendingShipResetLifeSeq: local.pendingShipResetLifeSeq,
      pendingShipResetSequence: local.pendingShipResetSequence,
      lifecycleBarrierElapsed: local.lifecycleBarrierElapsed,
      lifecycleBaselineReceiptsSeen: local.lifecycleBaselineReceiptsSeen,
      lifecycleBaselineReceiptsMatched: local.lifecycleBaselineReceiptsMatched,
      lifecycleBaselineReceiptsUnmatched: local.lifecycleBaselineReceiptsUnmatched,
      authorityPhase: local.authorityPhase,
      authorityTransitionKind: local.authorityTransitionKind,
      authoritySegment: local.authoritySegment,
      authorityScopeServerSessionId: local.authorityScopeServerSessionId,
      authorityScopeArenaStreamId: local.authorityScopeArenaStreamId,
      authorityScopePlayerId: local.authorityScopePlayerId,
      authorityScopeLifeSeq: local.authorityScopeLifeSeq,
      authorityScopeAlive: local.authorityScopeAlive,
      authorityReadinessJoined: local.authorityReadinessJoined,
      authorityReadinessArenaEpoch: local.authorityReadinessArenaEpoch,
      authorityReadinessSettings: local.authorityReadinessSettings,
      authorityReadinessSecurity: local.authorityReadinessSecurity,
      authorityReadinessMap: local.authorityReadinessMap,
      authorityReadinessFiles: local.authorityReadinessFiles,
      authorityInitialBaselineReadyToSubmit: local.authorityInitialBaselineReadyToSubmit,
      reliableQueuedPackets: local.reliableQueuedPackets,
    },
    networkLog: state?.networkLog || [],
    networkStats: state?.networkStats || {},
    inputTrace: state?.inputTrace || [],
    packetLogTail: (state?.packetLog || []).slice(-32),
  };
}

function snapshotAdvanced(before, after) {
  const beforeMetrics = before?.uiMetrics || {};
  const afterMetrics = after?.uiMetrics || {};
  return Number.isInteger(beforeMetrics.latestSnapshotId) &&
    Number.isInteger(afterMetrics.latestSnapshotId) &&
    beforeMetrics.latestSnapshotId !== afterMetrics.latestSnapshotId &&
    Number.isInteger(afterMetrics.lastAckedSnapshotId) &&
    afterMetrics.lastAckedSnapshotId !== beforeMetrics.lastAckedSnapshotId;
}

function assertNoLifecycleFailure(server, state) {
  const logs = server.logs.join("");
  const forbidden = [
    "noncanonical_ship_life_reset",
    "ship change acknowledgement timed out",
    "lifecycle baseline rejected",
  ].filter((needle) => logs.toLowerCase().includes(needle.toLowerCase()));
  const local = state?.uiMetrics?.localGameplayState || {};
  if (forbidden.length > 0 || !lifecycleIsReady(state)) {
    throw new Error(`initial lifecycle did not remain authoritative: ${JSON.stringify({
      forbidden,
      state: compactLifecycleState(state),
    })}`);
  }
}

async function fetchStatus(port) {
  const response = await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`);
  if (!response.ok) throw new Error(`server status failed with HTTP ${response.status}`);
  return response.json();
}

async function waitForCleanup(port) {
  let status = null;
  await waitUntil("fresh startup session cleans up", async () => {
    status = await fetchStatus(port);
    return status.clients === 0 && status.joinedPlayers === 0 && status.rtc_sessions === 0 &&
      status.rtc_outbound_queue_packets === 0 && status.rtc_outbound_queue_bytes === 0 &&
      Object.values(status.rtc_inbound_queue_packets || {}).every((count) => count === 0);
  }, 8_000);
  return status;
}

function writeFailureEvidence({ cycle, server, state, screenshotBytes, browserLogs, error, provenance }) {
  const directory = `${artifactTarget}-failure-cycle-${cycle}`;
  fs.mkdirSync(directory, { recursive: false });
  fs.writeFileSync(path.join(directory, "server.log"), server?.logs.join("") || "");
  fs.writeFileSync(
    path.join(directory, "client-state.json"),
    `${JSON.stringify(state ? compactLifecycleState(state) : null, null, 2)}\n`,
  );
  fs.writeFileSync(path.join(directory, "browser.log"), (browserLogs || []).join(""));
  fs.writeFileSync(path.join(directory, "error.txt"), `${error.stack || error.message}\n`);
  if (screenshotBytes) fs.writeFileSync(path.join(directory, "screenshot.png"), screenshotBytes);
  writeQaTraceFailureResult({ directory, provenance,
    captures: [{ label: "browser", state, serverLogs: server?.logs }] });
  return directory;
}

async function runChromeCycle({ cycle, staticServer, chrome, provenance }) {
  const serverPort = await freePort();
  const server = startRealServer(serverPort, { zone: arena });
  let client = null;
  let lastState = null;
  let screenshotBytes = null;
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1&name=startC${cycle}`;
    client = await openChromeClient(chrome, url);
    await waitUntil(`Chrome ${arena} cycle ${cycle} initial authority`, async () => {
      lastState = await client.metricsState();
      return lifecycleIsReady(lastState);
    }, 60_000);
    const before = await client.pageState();
    const playerId = before.uiMetrics.selfPlayerId;
    const startX = before.uiMetrics.playerX;
    const startY = before.uiMetrics.playerY;
    await client.hold("ArrowUp", 450);
    await waitUntil(`Chrome ${arena} cycle ${cycle} server movement`, async () => {
      const records = traceRecords(server, "human authority input trace");
      return records.some((record) => record.player_id === playerId && (record.input_bits & 1) !== 0);
    }, 8_000);
    await sleep(300);
    const after = await client.pageState();
    const displacement = Math.hypot(
      (after.uiMetrics.playerX ?? startX) - startX,
      (after.uiMetrics.playerY ?? startY) - startY,
    );
    if (displacement <= 0.5 || !snapshotAdvanced(before, after)) {
      throw new Error(`real Chrome movement/snapshot ACK did not advance: ${JSON.stringify({
        displacement,
        before: compactLifecycleState(before),
        after: compactLifecycleState(after),
      })}`);
    }
    assertNoLifecycleFailure(server, after);
    const securityAudit = assertSecurityRejectAudit(server);
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [chrome.logs],
      disconnect: async () => {
        await client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true");
      },
    });
    screenshotBytes = Buffer.from(await client.screenshot(), "base64");
    await sleep(750);
    lastState = await client.pageState();
    const sentDisconnect = (lastState.packetLog || []).some((entry) =>
      entry.direction === "sent" && JSON.stringify(entry.bytes) === "[0,7]"
    );
    if (!sentDisconnect) {
      throw new Error("Chrome disconnect request was not emitted through the normal WASM transport");
    }
    const cleanupStatus = await waitForCleanup(serverPort);
    const artifactState = await captureBrowserArtifactState(client);
    assertCompleteQaTraceCaptures([artifactState.qaTraceCapture]);
    await client.close();
    client = null;
    return {
      cycle,
      playerId,
      displacement,
      before: compactLifecycleState(before),
      after: compactLifecycleState(after),
      securityAudit,
      deliveryAudit,
      cleanupStatus,
      serverLog: server.logs.join(""),
      screenshotBytes,
      artifactState,
    };
  } catch (error) {
    if (client) {
      lastState = await captureBrowserArtifactState(client);
      screenshotBytes = Buffer.from(await client.screenshot().catch(() => ""), "base64");
    }
    const directory = writeFailureEvidence({
      cycle,
      server,
      state: lastState,
      screenshotBytes: screenshotBytes?.length > 0 ? screenshotBytes : null,
      browserLogs: chrome.logs,
      error,
      provenance,
    });
    error.message = `${error.message}\nretained startup failure: ${directory}`;
    throw error;
  } finally {
    await client?.close().catch(() => {});
    await stopProcess(server.child).catch(() => {});
  }
}

async function runFirefoxCycle({ cycle, staticServer, driver, provenance }) {
  const serverPort = await freePort();
  const server = startRealServer(serverPort, { zone: arena });
  let lastState = null;
  let screenshotBytes = null;
  const actor = firefoxArtifactClient(driver);
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await createSession(driver);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1&name=startF${cycle}`;
    await openClient(driver, url);
    await waitUntil(`Firefox ${arena} cycle ${cycle} initial authority`, async () => {
      lastState = await pageState(driver);
      return lifecycleIsReady(lastState);
    }, 60_000);
    const before = await pageState(driver);
    const playerId = before.uiMetrics.selfPlayerId;
    const startX = before.uiMetrics.playerX;
    const startY = before.uiMetrics.playerY;
    // Keep the real WebDriver key source held until authority observes it.
    // A compound down/pause/up action can complete inside Marionette without
    // exposing the intermediate key state to a slow debug-WASM render. Split
    // the genuine actions and make the server sample—not elapsed time—the
    // release boundary.
    const thrustStartedAt = Date.now();
    await firefoxKeyDown(driver, "ArrowUp");
    try {
      await waitUntil(`Firefox ${arena} cycle ${cycle} server movement`, async () => {
        const records = traceRecords(server, "human authority input trace");
        return records.some((record) =>
          record.player_id === playerId && (record.input_bits & 1) !== 0
        );
      }, 8_000);
      const remainingHoldMs = 450 - (Date.now() - thrustStartedAt);
      if (remainingHoldMs > 0) await sleep(remainingHoldMs);
    } finally {
      await firefoxKeyUp(driver, "ArrowUp").catch(() => {});
    }
    await sleep(300);
    const after = await pageState(driver);
    const displacement = Math.hypot(
      (after.uiMetrics.playerX ?? startX) - startX,
      (after.uiMetrics.playerY ?? startY) - startY,
    );
    if (displacement <= 0.5 || !snapshotAdvanced(before, after)) {
      throw new Error(`real Firefox movement/snapshot ACK did not advance: ${JSON.stringify({
        displacement,
        before: compactLifecycleState(before),
        after: compactLifecycleState(after),
      })}`);
    }
    assertNoLifecycleFailure(server, after);
    const securityAudit = assertSecurityRejectAudit(server);
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [driver.logs],
      disconnect: async () => {
        await execute(driver, "window.__zubspaceSendDisconnectForTest?.(); return true;");
      },
    });
    screenshotBytes = Buffer.from(await screenshot(driver), "base64");
    await sleep(750);
    lastState = await pageState(driver);
    const sentDisconnect = (lastState.packetLog || []).some((entry) =>
      entry.direction === "sent" && JSON.stringify(entry.bytes) === "[0,7]"
    );
    if (!sentDisconnect) {
      throw new Error("Firefox disconnect request was not emitted through the normal WASM transport");
    }
    const cleanupStatus = await waitForCleanup(serverPort);
    const artifactState = await captureBrowserArtifactState(actor);
    assertCompleteQaTraceCaptures([artifactState.qaTraceCapture]);
    await deleteSession(driver);
    return {
      cycle,
      playerId,
      displacement,
      before: compactLifecycleState(before),
      after: compactLifecycleState(after),
      securityAudit,
      deliveryAudit,
      cleanupStatus,
      serverLog: server.logs.join(""),
      screenshotBytes,
      artifactState,
    };
  } catch (error) {
    lastState = await captureBrowserArtifactState(driver.sessionId ? actor : null);
    const encoded = driver.sessionId ? await screenshot(driver).catch(() => "") : "";
    screenshotBytes = encoded ? Buffer.from(encoded, "base64") : null;
    const directory = writeFailureEvidence({
      cycle,
      server,
      state: lastState,
      screenshotBytes,
      browserLogs: driver.logs,
      error,
      provenance,
    });
    error.message = `${error.message}\nretained startup failure: ${directory}`;
    throw error;
  } finally {
    await deleteSession(driver).catch(() => {});
    await stopProcess(server.child).catch(() => {});
  }
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const artifactDirectory = createAtomicArtifactDirectory(artifactTarget);
  const staticServer = await startStaticServer();
  const summaries = [];
  const qaTraceCaptureAudit = [];
  let chrome = null;
  let driver = null;
  try {
    if (browser === "chrome") {
      chrome = startChrome(await freePort(), `startup-lifecycle-${arena}`);
      await waitForChrome(chrome);
    } else {
      driver = startGeckodriver(await freePort(), `startup-lifecycle-${arena}-firefox`);
      await waitForGeckodriver(driver);
    }

    for (let cycle = 1; cycle <= cycles; cycle += 1) {
      const evidence = browser === "chrome"
        ? await runChromeCycle({ cycle, staticServer, chrome, provenance: initialProvenance })
        : await runFirefoxCycle({ cycle, staticServer, driver, provenance: initialProvenance });
      const prefix = `cycle-${String(cycle).padStart(2, "0")}`;
      fs.writeFileSync(path.join(artifactDirectory.temporary, `${prefix}.server.log`), evidence.serverLog);
      fs.writeFileSync(path.join(artifactDirectory.temporary, `${prefix}.screenshot.png`), evidence.screenshotBytes);
      qaTraceCaptureAudit.push(writeQaTraceArtifact({ directory: artifactDirectory.temporary,
        label: prefix, state: evidence.artifactState, serverLogs: [evidence.serverLog],
        provenance: initialProvenance }));
      const summary = { ...evidence };
      delete summary.serverLog;
      delete summary.screenshotBytes;
      delete summary.artifactState;
      summaries.push(summary);
      fs.writeFileSync(
        path.join(artifactDirectory.temporary, `${prefix}.json`),
        `${JSON.stringify(summary, null, 2)}\n`,
      );
      console.log(JSON.stringify({ ok: true, browser, arena, cycle, displacement: evidence.displacement }));
    }

    const finalProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    fs.writeFileSync(
      path.join(artifactDirectory.temporary, "provenance.json"),
      `${JSON.stringify(finalProvenance, null, 2)}\n`,
    );
    const result = {
      ok: true,
      transport: "rtc",
      browser,
      arena,
      freshSessions: cycles,
      noUserRequestedShipChange: true,
      genuineKeyboardMovement: true,
      cycles: summaries,
      qaTraceCaptureAudit,
      provenancePath: "provenance.json",
      provenance: finalProvenance,
      artifactFiles: artifactFileManifest(artifactDirectory.temporary),
    };
    const resultPath = path.join(artifactDirectory.temporary, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    artifactDirectory.publish();
    console.log(JSON.stringify({
      ok: true,
      browser,
      arena,
      freshSessions: cycles,
      artifact: path.join(artifactTarget, "result.json"),
      provenance: finalProvenance.workingTreeDiffSha256,
    }, null, 2));
  } catch (error) {
    artifactDirectory.cleanup();
    throw error;
  } finally {
    await deleteSession(driver).catch(() => {});
    await Promise.all([
      stopProcess(chrome?.child),
      stopProcess(driver?.child),
    ]).catch(() => {});
    if (chrome?.userDataDir) fs.rmSync(chrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exit(1);
  });
}

export { main, lifecycleIsReady };
