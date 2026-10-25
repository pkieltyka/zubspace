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
import { assertArtifactProvenanceMatches } from "./artifact_provenance.mjs";
import { assertRetainedArtifactDirectoryAvailable } from "./artifact_paths.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { logLines } from "../../tools/log_chunks.mjs";
import {
  freePort,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";

const e2eScript = "tests/e2e/initial_file_request_retry.mjs";
const runStartedAt = new Date().toISOString();
const runStartedAtMs = Date.now();
const suppressedRequestBudget = 6;
const artifactDir = path.resolve(
  process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
    path.join(os.homedir(), ".local/state/zubspace/e2e", `initial-file-request-retry-${Date.now()}`),
);
assertRetainedArtifactDirectoryAvailable(artifactDir);

function serverFileRequests(logs, marker, ordinalField = null) {
  const rows = [];
  for (const line of logLines(logs)) {
    if (!line.includes(marker)) continue;
    const playerId = line.match(/\bplayer_id=(\d+)/)?.[1];
    const sessionId = line.match(/\bsession=PlayerSessionId\((\d+)\)/)?.[1];
    const fileIndex = line.match(/\bfile_index=(\d+)/)?.[1];
    const ordinal = ordinalField == null ? null : line.match(new RegExp(`\\b${ordinalField}=(\\d+)`))?.[1];
    if (playerId == null || sessionId == null || fileIndex == null ||
        (ordinalField != null && ordinal == null)) {
      throw new Error(`malformed server file-request evidence: ${line}`);
    }
    rows.push({
      playerId: Number(playerId),
      sessionId: Number(sessionId),
      fileIndex: Number(fileIndex),
      ordinal: ordinal == null ? null : Number(ordinal),
      line,
    });
  }
  return rows;
}

function compact(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return {
    at: Date.now(),
    playerId: metrics.selfPlayerId,
    playerAlive: metrics.playerAlive,
    serverPlayerAlive: metrics.serverPlayerAlive,
    playerX: metrics.playerX,
    playerY: metrics.playerY,
    authorityPhase: local.authorityPhase,
    authorityTransitionKind: local.authorityTransitionKind,
    authorityScopeAlive: local.authorityScopeAlive,
    authorityReadinessMap: local.authorityReadinessMap,
    authorityReadinessFiles: local.authorityReadinessFiles,
    zoneMapReady: local.zoneMapReady,
    zoneFilesReady: local.zoneFilesReady,
    pendingFileRequests: local.pendingFileRequests,
    pendingFileFrontIndex: local.pendingFileFrontIndex,
    fileRequestInFlight: local.fileRequestInFlight,
    fileRequestReissues: local.fileRequestReissues,
    fileRequestFramesSinceSend: local.fileRequestFramesSinceSend,
    pendingZoneDownloads: local.pendingZoneDownloads,
    networkNotice: local.networkNotice,
  };
}

async function main() {
  if (process.env.ZUBSPACE_E2E_HEADLESS !== "0") {
    throw new Error("the initial file-request retry proof must run with headed Chrome");
  }

  process.env.ZUBSPACE_SERVER_TEST_DROP_INITIAL_MAP_FILE_REQUESTS = String(suppressedRequestBudget);
  process.env.ZUBSPACE_SERVER_BOT_COUNT = "0";
  process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
  const provenanceOptions = { runStartedAt, e2eScript };
  const initialProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
  const serverPort = await freePort();
  const chromePort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort, { botCount: 0 });
  const chrome = startChrome(chromePort, "initial-file-request-retry");
  let client = null;
  let pageUrl = null;
  let successArtifacts = null;

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await waitForChrome(chrome);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    pageUrl = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}` +
      "&password=secret&arena=chaos-zone&name=fileretry&packetlog=1&trace=1";
    client = await openChromeClient(chrome, pageUrl, { waitForReveal: false,
      initScript: "window.__zubspaceZoneCacheReadDisabledForTest = true;" });

    const suppressed = () => serverFileRequests(
      server.logs,
      "server test initial map file request suppressed",
      "request_ordinal",
    );
    const accepted = () => serverFileRequests(
      server.logs,
      "server zone_file_request_accepted",
    );
    const retrySamples = [];
    let lastSampleAt = 0;
    await waitUntil("server receives the original initial map request", async () => {
      const drops = suppressed();
      if (drops.length === 0) return false;
      const sample = compact(await client.metricsState());
      if (sample.zoneFilesReady !== false || sample.pendingFileFrontIndex !== 0) {
        throw new Error(`initial map request arrived outside a pending zone install: ${JSON.stringify(sample)}`);
      }
      retrySamples.push({ elapsedMs: Date.now() - runStartedAtMs, suppressedRequests: drops.length, ...sample });
      return true;
    }, 30_000);

    await waitUntil("four bounded retries and the re-armed request reach the server", async () => {
      const drops = suppressed();
      if (drops.length > suppressedRequestBudget) {
        throw new Error(`server suppressed more requests than configured: ${JSON.stringify(drops)}`);
      }
      if (drops.length === suppressedRequestBudget) {
        const sample = compact(await client.metricsState());
        retrySamples.push({ elapsedMs: Date.now() - runStartedAtMs, suppressedRequests: drops.length, ...sample });
        return true;
      }
      if (Date.now() - lastSampleAt >= 300) {
        lastSampleAt = Date.now();
        const sample = compact(await client.metricsState());
        retrySamples.push({ elapsedMs: Date.now() - runStartedAtMs, suppressedRequests: drops.length, ...sample });
      }
      return false;
    }, 25_000);

    const suppressedRequests = suppressed();
    const firstDrop = suppressedRequests[0];
    if (suppressedRequests.some((request, index) =>
      request.ordinal !== index + 1 ||
      request.playerId !== firstDrop.playerId ||
      request.sessionId !== firstDrop.sessionId ||
      request.fileIndex !== 0
    )) {
      throw new Error(`initial file retries changed identity, index, or order: ${JSON.stringify(suppressedRequests)}`);
    }
    const maxObservedReissues = Math.max(
      ...retrySamples.map((sample) => Number(sample.fileRequestReissues) || 0),
    );
    if (maxObservedReissues !== 4) {
      throw new Error(`the client did not exercise all four bounded file-request reissues: ${JSON.stringify({
        maxObservedReissues, retrySamples,
      })}`);
    }

    await waitUntil("initial join exposes its long-wait status while files are still pending", async () => {
      const sample = compact(await client.metricsState());
      if (sample.networkNotice?.includes("Still waiting for the initial server baseline")) {
        retrySamples.push({ elapsedMs: Date.now() - runStartedAtMs, suppressedRequests: suppressed().length, ...sample });
        return sample.zoneFilesReady === false &&
          sample.authorityTransitionKind === "InitialJoin" &&
          sample.authorityPhase !== "Live";
      }
      return false;
    }, 25_000);
    const waitingState = compact(await client.metricsState());
    if (
      waitingState.authorityTransitionKind !== "InitialJoin" ||
      waitingState.authorityPhase === "Live" ||
      waitingState.zoneFilesReady !== false
    ) {
      throw new Error(`the re-request wait was not an initial-join file stall: ${JSON.stringify(waitingState)}`);
    }

    await waitUntil("the re-armed same-session map request is accepted", async () => {
      return accepted().some((request) =>
        request.playerId === firstDrop.playerId &&
        request.sessionId === firstDrop.sessionId &&
        request.fileIndex === 0
      );
    }, 20_000);
    const acceptedMapRequests = accepted().filter((request) =>
      request.playerId === firstDrop.playerId &&
      request.sessionId === firstDrop.sessionId &&
      request.fileIndex === 0
    );
    if (acceptedMapRequests.length !== 1) {
      throw new Error(`expected one eventual map response on the original session: ${JSON.stringify(acceptedMapRequests)}`);
    }

    await waitUntil("the re-requested map and zone files install", async () => {
      const state = await client.metricsState();
      return state.uiMetrics?.localGameplayState?.zoneMapReady === true &&
        state.uiMetrics?.localGameplayState?.zoneFilesReady === true;
    }, 60_000);
    await waitUntil("the recovered client enters a live ship", async () => {
      const state = await client.metricsState();
      const local = state.uiMetrics?.localGameplayState || {};
      if (local.authorityScopeAlive === true &&
          local.authorityPhase === "Live" &&
          state.uiMetrics?.playerAlive === true &&
          state.uiMetrics?.serverPlayerAlive === true) return true;
      await client.press("Escape", 60);
      await client.press("2", 50);
      return false;
    }, 30_000);

    await sleep(500);
    const recovered = compact(await client.pageState());
    if (
      recovered.authorityPhase !== "Live" ||
      recovered.authorityScopeAlive !== true ||
      recovered.zoneFilesReady !== true ||
      recovered.pendingFileRequests !== 0 ||
      recovered.fileRequestInFlight != null ||
      recovered.networkNotice?.includes("Still waiting for the initial server baseline")
    ) {
      throw new Error(`initial file re-request did not recover to a clean live scope: ${JSON.stringify(recovered)}`);
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
      clients: [{ label: "initial-file-retry", client }],
      extraLogs: { "initial-file-retry-chrome": chrome.logs },
      provenance: finalProvenance,
    });
    const result = {
      ok: true,
      scenario: "initial join re-requests a withheld map file on the same RTC session",
      suppressedRequestBudget,
      suppressedRequests,
      acceptedMapRequests,
      maxObservedReissues,
      retrySamples,
      waitingState,
      recovered,
      elapsedMs: Date.now() - runStartedAtMs,
      securityAudit,
      dataChannelDeliveryAudit,
      provenance: finalProvenance,
      successArtifacts,
    };
    const resultPath = path.join(artifactDir, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`, { flag: "wx" });
    console.log(JSON.stringify({
      ok: result.ok,
      scenario: result.scenario,
      droppedRequestCount: suppressedRequests.length,
      acceptedMapRequestCount: acceptedMapRequests.length,
      maxObservedReissues,
      waitingAuthority: waitingState.authorityPhase,
      recoveredAuthority: recovered.authorityPhase,
      artifactDir,
    }));
  } catch (error) {
    if (client) {
      try {
        const failureProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
        await saveBrowserE2eArtifacts({
          artifactDir: `${artifactDir}-failure`,
          server,
          staticServer,
          urls: [pageUrl],
          clients: [{ label: "initial-file-retry", client }],
          extraLogs: { failure: error.stack || String(error), "initial-file-retry-chrome": chrome.logs },
          provenance: failureProvenance,
        });
      } catch (artifactError) {
        console.error(`failure artifact capture also failed: ${artifactError.stack || artifactError}`);
      }
    }
    throw error;
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
