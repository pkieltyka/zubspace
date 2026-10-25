#!/usr/bin/env node

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  openChromeClient,
  saveBrowserE2eArtifacts,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
} from "./artifact_provenance.mjs";
import { assertRetainedArtifactDirectoryAvailable } from "./artifact_paths.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { auditBaselineWaitGamePackets } from "./baseline_wait_packets.mjs";
import {
  freePort,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";

const e2eScript = "tests/e2e/prize_baseline_delay.mjs";
const runStartedAt = new Date().toISOString();
const runStartedAtMs = Date.now();
const delaySnapshots = Number(process.env.ZUBSPACE_E2E_PRIZE_BASELINE_DELAY_SNAPSHOTS || "720");
const artifactDir = path.resolve(
  process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
    path.join(os.homedir(), ".local/state/zubspace/e2e", `q13-prize-baseline-delay-${Date.now()}`),
);
assertRetainedArtifactDirectoryAvailable(artifactDir);

function compact(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return {
    at: Date.now(),
    playerX: metrics.playerX,
    playerY: metrics.playerY,
    playerVelocityX: metrics.playerVelocityX,
    playerVelocityY: metrics.playerVelocityY,
    playerAlive: metrics.playerAlive,
    serverPlayerAlive: metrics.serverPlayerAlive,
    playerLifeSeq: metrics.playerLifeSeq,
    clientInputSeq: metrics.clientInputSeq,
    positionsSent: metrics.positionsSent,
    networkPrizeBaselinePending: local.networkPrizeBaselinePending,
    gameplayStepReady: local.gameplayStepReady,
    gameplayScopeReady: local.gameplayScopeReady,
    inputBits: local.inputBits,
    inputRingLen: local.inputRingLen,
    authorityPhase: local.authorityPhase,
    authorityTransitionKind: local.authorityTransitionKind,
    authorityInitialBaselineReadyToSubmit: local.authorityInitialBaselineReadyToSubmit,
    stepAuthorityReady: local.stepAuthorityReady,
    preStepNetworkRebaseSuppressed: local.preStepNetworkRebaseSuppressed,
    authorityScopeAlive: local.authorityScopeAlive,
    authorityScopeLifeSeq: local.authorityScopeLifeSeq,
    authorityReadinessJoined: local.authorityReadinessJoined,
    authorityReadinessSecurity: local.authorityReadinessSecurity,
    authorityReadinessMap: local.authorityReadinessMap,
    authorityReadinessFiles: local.authorityReadinessFiles,
    lastSelfServerEnergy: local.lastSelfServerEnergy,
    networkNotice: local.networkNotice,
  };
}

async function readinessProbe(client) {
  return client.evaluate(`(() => {
    const metrics = window.__zubspaceUiMetrics || {};
    const local = metrics.localGameplayState || {};
    return {
      playerLifeSeq: metrics.playerLifeSeq,
      playerAlive: metrics.playerAlive,
      serverPlayerAlive: metrics.serverPlayerAlive,
      latestSnapshotId: metrics.latestSnapshotId,
      zoneFilesReady: local.zoneFilesReady,
      networkPrizeBaselinePending: local.networkPrizeBaselinePending,
      gameplayStepReady: local.gameplayStepReady,
      gameplayScopeReady: local.gameplayScopeReady,
      authorityPhase: local.authorityPhase,
      authorityTransitionKind: local.authorityTransitionKind,
      authorityInitialBaselineReadyToSubmit: local.authorityInitialBaselineReadyToSubmit,
      stepAuthorityReady: local.stepAuthorityReady,
      preStepNetworkRebaseSuppressed: local.preStepNetworkRebaseSuppressed,
      authorityScopeAlive: local.authorityScopeAlive,
      authorityScopeLifeSeq: local.authorityScopeLifeSeq,
      authorityReadinessJoined: local.authorityReadinessJoined,
      authorityReadinessSecurity: local.authorityReadinessSecurity,
      authorityReadinessMap: local.authorityReadinessMap,
      authorityReadinessFiles: local.authorityReadinessFiles,
      networkNotice: local.networkNotice,
    };
  })()`);
}

function distance(a, b) {
  return Math.hypot((a.x ?? 0) - (b.x ?? 0), (a.y ?? 0) - (b.y ?? 0));
}

async function main() {
  if (!Number.isInteger(delaySnapshots) || delaySnapshots < 360 || delaySnapshots > 2_000) {
    throw new Error("baseline delay must be between 360 and 2000 snapshots");
  }
  if (process.env.ZUBSPACE_E2E_HEADLESS !== "0") {
    throw new Error("the Q13 movement-feel replay must run with headed Chrome");
  }

  process.env.ZUBSPACE_SERVER_TEST_PRIZE_BASELINE_DELAY_SNAPSHOTS = String(delaySnapshots);
  process.env.ZUBSPACE_SERVER_BOT_COUNT = "0";
  process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
  const provenanceOptions = { runStartedAt, e2eScript };
  const initialProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
  const serverPort = await freePort();
  const chromePort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort, { botCount: 0 });
  const chrome = startChrome(chromePort, "q13-prize-baseline");
  let client = null;
  let pageUrl = null;
  let successArtifacts = null;

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await waitForChrome(chrome);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    pageUrl = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}` +
      "&password=secret&arena=chaos-zone&name=q13pilot&packetlog=1&trace=1";
    client = await openChromeClient(chrome, pageUrl, { waitForReveal: false });

    await waitUntil("Q13 client publishes the arena map", async () => {
      const state = await client.metricsState();
      return state.uiMetrics?.localGameplayState?.zoneMapReady === true;
    }, 30_000);
    await waitUntil("Q13 client finishes its zone files before ship selection", async () => {
      return (await readinessProbe(client)).zoneFilesReady === true;
    }, 60_000);

    await waitUntil("Q13 client enters a live ship without waiting for prize state", async () => {
      const state = await readinessProbe(client);
      if (state.authorityScopeAlive === true) return true;
      await client.press("Escape", 60);
      await client.press("2", 50);
      return false;
    }, 30_000);

    let lastReadinessSampleAt = 0;
    const readinessTrace = [];
    await waitUntil("initial join awaits its durable baseline after transport and map readiness", async () => {
      const state = await readinessProbe(client);
      if (Date.now() - lastReadinessSampleAt >= 1_000) {
        lastReadinessSampleAt = Date.now();
        const sample = { elapsedMs: Date.now() - Number(runStartedAtMs), ...state };
        readinessTrace.push(sample);
        console.log(`Q13 readiness ${JSON.stringify(sample)}`);
      }
      return state.authorityScopeAlive === true &&
        Number.isInteger(state.authorityScopeLifeSeq) &&
        state.authorityReadinessJoined === true &&
        state.authorityReadinessSecurity === true &&
        state.authorityReadinessMap === true &&
        state.authorityReadinessFiles === true &&
        state.networkPrizeBaselinePending === true;
    }, 40_000);

    const pendingStartState = await client.metricsState();
    const pendingStart = compact(pendingStartState);
    const startPosition = { x: pendingStart.playerX, y: pendingStart.playerY };
    const startInputSeq = pendingStart.clientInputSeq;
    const startPositionsSent = pendingStart.positionsSent;
    const packetSequenceStart = await client.evaluate(
      "window.__zubspacePacketLog?.at(-1)?.seq ?? 0",
    );

    await client.keyDown("w");
    await sleep(1_500);
    const duringState = await client.metricsState();
    const during = compact(duringState);
    const localDistance = distance(startPosition, { x: during.playerX, y: during.playerY });
    const inputSequenceAdvance = (during.clientInputSeq - startInputSeq) & 0xffff;
    const baselineWaitTrafficAudit = await client.evaluate(
      `(${auditBaselineWaitGamePackets.toString()})(window.__zubspacePacketLog || [], ${Number(packetSequenceStart)})`,
    );
    const gameplayPacketsDuringWait = baselineWaitTrafficAudit.gameplayPacketsDuringWait;
    const pendingSplash = await client.evaluate(`({
      visible: !!document.querySelector('#loading-overlay') && !document.querySelector('#loading-overlay').classList.contains('hidden'),
      ready: window.__zubspaceLifecycle?.inGameplay,
    })`);

    if (
      !Number.isFinite(startPosition.x) ||
      !Number.isFinite(startPosition.y) ||
      !Number.isFinite(during.playerX) ||
      !Number.isFinite(during.playerY) ||
      during.networkPrizeBaselinePending !== true ||
      during.authorityScopeAlive !== true ||
      during.authorityPhase !== "AwaitingDurableSubmission" ||
      during.authorityTransitionKind !== "InitialJoin" ||
      during.gameplayScopeReady !== false ||
      during.gameplayStepReady !== false ||
      localDistance > 0.5 ||
      inputSequenceAdvance !== 0 ||
      during.positionsSent !== startPositionsSent ||
      baselineWaitTrafficAudit.windowComplete !== true ||
      baselineWaitTrafficAudit.qualityEchoPacketsDuringWait < 1 ||
      gameplayPacketsDuringWait !== 0
      || pendingSplash.visible !== true || pendingSplash.ready !== false
    ) {
      throw new Error(`delayed baseline movement/outbound contract failed: ${JSON.stringify({
        pendingStart,
        during,
        localDistance,
        inputSequenceAdvance,
        startPositionsSent,
        gameplayPacketsDuringWait,
        baselineWaitTrafficAudit,
        pendingSplash,
      })}`);
    }

    await waitUntil("long-wait notice appears while requests continue", async () => {
      const state = await readinessProbe(client);
      return String(state.networkNotice || "").includes(
        "Still waiting for the initial server baseline",
      );
    }, 20_000);
    const longWaitState = await client.pageState();
    const splashLongWait = await client.evaluate(`document.querySelector('#loading-identity-text')?.textContent`);
    if (!String(splashLongWait).startsWith('Still connecting…')) {
      throw new Error(`pending splash lacks its long-wait status: ${JSON.stringify(splashLongWait)}`);
    }
    const pendingScreenshot = await client.screenshot();

    await waitUntil("server prize baseline installs a live gameplay scope", async () => {
      const state = await readinessProbe(client);
      const metrics = (await client.metricsState()).uiMetrics || {};
      const local = metrics.localGameplayState || {};
      return state.networkPrizeBaselinePending === false &&
        local.lastSelfServerEnergy != null &&
        state.authorityScopeAlive === true &&
        state.authorityPhase === "Live" &&
        state.gameplayScopeReady === true &&
        state.gameplayStepReady === true;
    }, 40_000);
    await sleep(1_000);
    const postBaselineMovementState = await client.metricsState();
    const postBaselineMovement = compact(postBaselineMovementState);
    const postBaselineDistance = distance(startPosition, {
      x: postBaselineMovement.playerX,
      y: postBaselineMovement.playerY,
    });
    const postBaselineInputAdvance =
      (postBaselineMovement.clientInputSeq - startInputSeq) & 0xffff;
    if (
      !Number.isFinite(postBaselineMovement.playerX) ||
      !Number.isFinite(postBaselineMovement.playerY) ||
      postBaselineMovement.networkPrizeBaselinePending !== false ||
      postBaselineMovement.authorityPhase !== "Live" ||
      postBaselineMovement.gameplayScopeReady !== true ||
      postBaselineMovement.gameplayStepReady !== true ||
      postBaselineDistance <= 1 ||
      postBaselineInputAdvance === 0
    ) {
      throw new Error(`movement did not resume after the durable baseline: ${JSON.stringify({
        start: pendingStart,
        during,
        postBaselineMovement,
        postBaselineDistance,
        postBaselineInputAdvance,
      })}`);
    }
    await client.keyUp("w");
    await waitUntil("post-baseline position reports resume", async () => {
      const metrics = (await client.metricsState()).uiMetrics || {};
      return metrics.positionsSent >= startPositionsSent + 2;
    }, 8_000);
    const recoveredState = await client.pageState();
    const recovered = compact(recoveredState);
    if (
      recovered.networkPrizeBaselinePending !== false ||
      recovered.authorityScopeAlive !== true ||
      recovered.lastSelfServerEnergy == null ||
      String(recovered.networkNotice || "").includes("Still waiting for the initial server baseline")
    ) {
      throw new Error(`baseline did not clear the long-wait state: ${JSON.stringify(recovered)}`);
    }

    const securityAudit = assertSecurityRejectAudit(server);
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [chrome.logs],
      disconnect: async () => {
        await client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {});
      },
    });
    const finalProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    successArtifacts = await saveBrowserE2eArtifacts({
      artifactDir,
      server,
      staticServer,
      urls: [pageUrl],
      clients: [{ label: "q13-pilot", client }],
      extraLogs: { "q13-chrome": chrome.logs },
      provenance: finalProvenance,
    });
    fs.writeFileSync(
      path.join(artifactDir, "during-baseline.png"),
      Buffer.from(pendingScreenshot, "base64"),
      { flag: "wx" },
    );
    fs.writeFileSync(
      path.join(artifactDir, "during-baseline-state.json"),
      `${JSON.stringify(longWaitState, null, 2)}\n`,
      { flag: "wx" },
    );
    const result = {
      ok: true,
      scenario: "initial authority wait during delayed private prize baseline",
      delaySnapshots,
      pendingStart,
      during,
      localDistance,
      inputSequenceAdvance,
      postBaselineMovement,
      postBaselineDistance,
      postBaselineInputAdvance,
      duringGameplayStepReady: during.gameplayStepReady,
      duringGameplayScopeReady: during.gameplayScopeReady,
      duringAuthorityPhase: during.authorityPhase,
      duringAuthorityTransitionKind: during.authorityTransitionKind,
      gameplayPacketsDuringWait,
      baselineWaitTrafficAudit,
      readinessTrace,
      longWaitStatus: compact(longWaitState),
      recovered,
      postBaselinePosition: { x: recovered.playerX, y: recovered.playerY },
      securityAudit,
      dataChannelDeliveryAudit,
      provenance: finalProvenance,
      successArtifacts,
    };
    const resultPath = path.join(artifactDir, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`, { flag: "wx" });
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    let failureArtifacts = null;
    if (client) {
      const failureDirectory = `${artifactDir}-failure`;
      try {
        const failureProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
        failureArtifacts = await saveBrowserE2eArtifacts({
          artifactDir: failureDirectory,
          server,
          staticServer,
          urls: [pageUrl],
          clients: [{ label: "q13-pilot", client }],
          extraLogs: { "q13-chrome": chrome.logs, failure: error.stack || String(error) },
          provenance: failureProvenance,
        });
      } catch (artifactError) {
        console.error(`failure artifact capture also failed: ${artifactError.stack || artifactError}`);
      }
    }
    const suffix = failureArtifacts?.absoluteArtifactDir
      ? `\nFailure evidence: ${failureArtifacts.absoluteArtifactDir}`
      : "";
    throw new Error(`${error.stack || error.message}${suffix}`);
  } finally {
    await client?.close().catch(() => {});
    await stopProcess(chrome.child).catch(() => {});
    await stopProcess(server.child).catch(() => {});
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
