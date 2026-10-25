#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

import {
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  browserE2eFailureFooter,
  compactSummary,
  openChromeClient,
  startChrome,
  summarizeClient,
  waitForChrome,
  waitForPlayersReady, rendezvousPlayers,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// Terminal-transport recovery acceptance. A client whose RTCPeerConnection reaches failed/closed
// must notice, re-offer on its own, re-enter gameplay scope, and become render-visible to its peer
// again within a bounded window. The forced close uses the same lifecycle path a real ICE failure
// takes (`rtcLifecycle.closeActive` -> `peer.close()`), and the artifact carries the page's own
// rejoin count plus the peer connection attempt count so the recovery is counted rather than
// inferred from a peer cycling.

const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const e2eScript = "tests/e2e/transport_recovery.mjs";
// Generous but bounded: the fresh offer, reliable-map re-transfer, and re-scope all have to land.
const RECOVERY_BUDGET_MS = 45_000;

function localGameplay(metrics) {
  return metrics?.localGameplayState || {};
}

function transportOpen(metrics) {
  return localGameplay(metrics).rtcTransportStatus === 2;
}

function rejoinCount(metrics) {
  return localGameplay(metrics).rtcRejoinCount ?? 0;
}

function gameplayScopeReady(metrics) {
  return localGameplay(metrics).gameplayScopeReady === true;
}

function gameplayStepReady(metrics) {
  return localGameplay(metrics).gameplayStepReady === true;
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const evidence = createFocusedBrowserEvidence({ e2eScript });
  const indicatorScreenshots = {};

  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaPort, "transport-recovery-alpha");
  const betaChrome = startChrome(betaPort, "transport-recovery-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server,
    serverPort,
    staticServer,
    urls: [alphaUrl, betaUrl],
    clients: [
      { label: "alpha", client: alpha },
      { label: "beta", client: beta },
    ],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs },
    extraScreenshots: indicatorScreenshots,
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase =
      `http://127.0.0.1:${staticServer.port}/index.html` +
      `?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(arena)}&trace=1`;
    alphaUrl = `${pageBase}&name=transportalpha`;
    betaUrl = `${pageBase}&name=transportbeta`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);

    const baseline = await alpha.metricsState();
    const betaBaseline = await beta.metricsState();
    if (!transportOpen(baseline.uiMetrics)) {
      throw new Error(`alpha transport was not open before the forced close: ${JSON.stringify(localGameplay(baseline.uiMetrics))}`);
    }
    if (rejoinCount(baseline.uiMetrics) !== 0) {
      throw new Error(`alpha had already rejoined before the scenario: ${rejoinCount(baseline.uiMetrics)}`);
    }
    if ((betaBaseline.uiMetrics?.remoteVisiblePlayers ?? 0) < 1) {
      throw new Error("beta did not see alpha before the forced close");
    }

    if (localGameplay(baseline.uiMetrics).networkIndicator !== null || localGameplay(betaBaseline.uiMetrics).networkIndicator !== null) {
      throw new Error("healthy clients must not display a network badge");
    }
    indicatorScreenshots.connected = await alpha.screenshot();
    // Delay only the next signaling exchange, leaving real browser RTC and all
    // gameplay messages untouched, so the recovery indicator can be observed.
    await alpha.evaluate(`(() => {
      const original = window.fetch;
      window.__networkIndicatorRestoreFetch = () => { window.fetch = original; delete window.__networkIndicatorRestoreFetch; };
      window.fetch = async (...args) => {
        if (String(args[0]?.url ?? args[0]).includes("/v1/rtc/offer")) await new Promise(resolve => setTimeout(resolve, 1500));
        return original.apply(window, args);
      };
      return true;
    })()`);

    // Force the terminal transition through the shipped lifecycle path. The
    // page's own hook is the same call an ICE `failed` event makes.
    const forced = await alpha.evaluate(
      'window.__zubspaceRtcCloseForTest?.(4, "qa-transport-recovery") === true',
    );
    if (!forced) {
      throw new Error("forced terminal close hook was not configured");
    }
    const forcedAt = Date.now();

    let disconnected = null;
    await waitUntil("red disconnected indicator during transport recovery", async () => {
      const state = await alpha.metricsState();
      if (localGameplay(state.uiMetrics).networkIndicator !== "[ Disconnected ]") return false;
      disconnected = localGameplay(state.uiMetrics);
      return true;
    }, 5_000);
    indicatorScreenshots.disconnected = await alpha.screenshot();

    // Random rejoin positions are not a promise of peer AOI overlap. Keep
    // the original45s total recovery/visibility budget, including real travel.
    await waitUntil("alpha re-enters its exact live transport scope", async () => {
      const m = (await alpha.metricsState()).uiMetrics;
      return rejoinCount(m) >= 1 && transportOpen(m) && gameplayScopeReady(m) && gameplayStepReady(m);
    }, RECOVERY_BUDGET_MS - (Date.now() - forcedAt));
    const recoveryRendezvous = await rendezvousPlayers(alpha, beta, server,
      { setupBudgetMs: RECOVERY_BUDGET_MS - (Date.now() - forcedAt) });
    let recovered = null;
    await waitUntil("alpha rejoins and is render-visible to beta again", async () => {
      const [alphaState, betaState] = await Promise.all([
        alpha.metricsState(),
        beta.metricsState(),
      ]);
      const rejoined = rejoinCount(alphaState.uiMetrics) >= 1;
      const alphaReady =
        transportOpen(alphaState.uiMetrics) &&
        gameplayScopeReady(alphaState.uiMetrics) &&
        gameplayStepReady(alphaState.uiMetrics);
      const betaSeesAlpha = (betaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1;
      if (rejoined && alphaReady && betaSeesAlpha) {
        recovered = { alphaState, betaState, elapsedMs: Date.now() - forcedAt };
        return true;
      }
      return false;
    }, Math.max(1, RECOVERY_BUDGET_MS - (Date.now() - forcedAt)));
    if (recovered.elapsedMs > RECOVERY_BUDGET_MS) throw new Error("transport recovery exceeded its original45s visibility budget");

    if (localGameplay(recovered.alphaState.uiMetrics).networkIndicator !== null) throw new Error("recovered client still displays the disconnected indicator");
    await alpha.evaluate("window.__networkIndicatorRestoreFetch?.(); true");
    indicatorScreenshots.recovered = await alpha.screenshot();

    const attempts = await alpha.evaluate(
      "window.__zubspaceRtcPeersForTest?.length ?? null",
    );
    const serverLog = server.logs.join("");
    const serverCalls = [...serverLog.matchAll(/server rtc player disconnected/g)].length;

    const result = await evidence.succeed({
      ok: true,
      transport: "rtc",
      arena,
      connectionIndicator: { healthy: null, disconnected: disconnected.networkIndicator, recovered: null, signalingDelayMs: 1500 },
      rejoin: {
        count: rejoinCount(recovered.alphaState.uiMetrics),
        elapsedMs: recovered.elapsedMs,
        rendezvous: recoveryRendezvous,
        attempts,
        forcedTerminalStatus: 4,
        serverPlayerDisconnects: serverCalls,
      },
      alpha: compactSummary(summarizeClient(recovered.alphaState)),
      beta: compactSummary(summarizeClient(recovered.betaState)),
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.message = `${error.message}\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(alphaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
