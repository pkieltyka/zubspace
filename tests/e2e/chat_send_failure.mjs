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
  waitForLivePlayerScopes,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const e2eScript = "tests/e2e/chat_send_failure.mjs";

function advancedU16(after, before) {
  if (!Number.isInteger(after) || !Number.isInteger(before)) return false;
  const delta = (after - before) & 0xffff;
  return delta > 0 && delta < 0x8000;
}

function advancedTick(after, before) {
  if (!Number.isInteger(after) || !Number.isInteger(before)) return false;
  const delta = (after - before) & 0x7fffffff;
  return delta > 0 && delta < 0x40000000;
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const evidence = createFocusedBrowserEvidence({ e2eScript });

  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaPort, "chat-failure-alpha");
  const betaChrome = startChrome(betaPort, "chat-failure-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs },
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. Each client
    // is guarded so one already-dead session cannot make cleanup throw.
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
      `&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    alphaUrl = `${pageBase}&name=chatfailalpha&chat_send_failure=armed`;
    betaUrl = `${pageBase}&name=chatfailbeta`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    // Reliable chat readiness is independent of mutual AOI visibility.
    await waitForLivePlayerScopes(alpha, beta);

    const before = await alpha.pageState();
    const senderName = summarizeClient(before).rosters.find(
      entry => entry.id === before.uiMetrics.selfPlayerId,
    )?.name;
    if (!senderName) throw new Error("chat sender missing from the admitted roster");
    if (before.uiMetrics?.browserNetworkOwned !== true) {
      throw new Error(`Game did not own browser network before injection: ${JSON.stringify(before.uiMetrics)}`);
    }
    const armed = await alpha.evaluate("window.__zubspaceFailNextChatSendForTest?.() === true");
    if (!armed) {
      throw new Error("chat-send failure injection seam was not configured");
    }

    await alpha.press("Enter", 40);
    await alpha.typeChatText("roundtwelve");
    await alpha.press("Enter", 40);

    let afterFailure = null;
    await waitUntil("one chat send failure with retained Game ownership", async () => {
      const state = await alpha.pageState();
      const failureState = await alpha.evaluate(
        "window.__zubspaceChatSendFailureStateForTest?.() || null",
      );
      if (
        failureState?.injected === 1 &&
        failureState.remaining === 0 &&
        state.networkStats?.chatSendFailuresInjected === 1 &&
        state.uiMetrics?.browserNetworkOwned === true
      ) {
        afterFailure = { state, failureState };
        return true;
      }
      return false;
    }, 8_000);

    const failedPacket = afterFailure.state.packetLog.find(
      (entry) =>
        entry.direction === "failed" &&
        entry.detail?.reason === "injected-chat-send-failure",
    );
    if (!failedPacket) {
      throw new Error("injected chat-send failure was not captured in the browser packet log");
    }

    const recoveryBaseline = afterFailure.state;
    const recoverySummary = summarizeClient(recoveryBaseline);
    await alpha.hold("ArrowUp", 280);
    await sleep(120);

    let recovered = null;
    await waitUntil("network polling and gameplay sends after chat failure", async () => {
      const state = await alpha.pageState();
      const summary = summarizeClient(state);
      const inputAdvanced = advancedU16(
        state.uiMetrics?.clientInputSeq,
        recoveryBaseline.uiMetrics?.clientInputSeq,
      );
      const snapshotAdvanced = advancedTick(
        state.uiMetrics?.latestSnapshotServerTick,
        recoveryBaseline.uiMetrics?.latestSnapshotServerTick,
      );
      const positionSent = summary.sentPositionCount > recoverySummary.sentPositionCount;
      if (
        state.uiMetrics?.browserNetworkOwned === true &&
        state.networkStats?.chatSendFailuresInjected === 1 &&
        inputAdvanced &&
        snapshotAdvanced &&
        positionSent
      ) {
        recovered = { state, summary, inputAdvanced, snapshotAdvanced, positionSent };
        return true;
      }
      return false;
    }, 8_000);

    // The reliable sender retries the failed frame. Both that message and a
    // later ordinary send must appear once on both peers, even when delivery
    // resumes in one batch. Do not wait for the retry before sending again:
    // that would miss the recovery-burst regression.
    await alpha.press("Enter", 40);
    await alpha.typeChatText("afterfailure");
    await alpha.press("Enter", 40);
    await waitUntil("normal named chat delivery after send failure", async () => {
      const states = await Promise.all([alpha.pageState(), beta.pageState()]);
      return states.every(state => {
        const messages = state.uiMetrics.localGameplayState.chatMessages;
        return messages.filter(text => text === `${senderName}: roundtwelve`).length === 1
          && messages.filter(text => text === `${senderName}: afterfailure`).length === 1
          && !messages.some(text => text.startsWith("You:"));
      });
    }, 8_000);

    const seriousEvents = [...alpha.events, ...beta.events].filter(
      (event) => event.type === "error" || event.type === "exception",
    );
    if (seriousEvents.length > 0) {
      throw new Error(`browser console errors: ${JSON.stringify(seriousEvents)}`);
    }

    const result = await evidence.succeed({
      ok: true,
      transport: "rtc",
      arena,
      injectedChatSendFailures: recovered.state.networkStats.chatSendFailuresInjected,
      recoveredMessageEchoedOnceToBothPeers: true,
      subsequentNamedMessageDeliveredOnceToBothPeers: true,
      browserNetworkOwnedAfterFailure: afterFailure.state.uiMetrics.browserNetworkOwned,
      browserNetworkOwnedAfterRecovery: recovered.state.uiMetrics.browserNetworkOwned,
      clientInputSeq: {
        before: recoveryBaseline.uiMetrics.clientInputSeq,
        after: recovered.state.uiMetrics.clientInputSeq,
      },
      latestSnapshotServerTick: {
        before: recoveryBaseline.uiMetrics.latestSnapshotServerTick,
        after: recovered.state.uiMetrics.latestSnapshotServerTick,
      },
      sentPositionCount: {
        before: recoverySummary.sentPositionCount,
        after: recovered.summary.sentPositionCount,
      },
      failedPacket: {
        seq: failedPacket.seq,
        channel: failedPacket.detail?.channel || null,
        reason: failedPacket.detail?.reason || null,
      },
      alpha: compactSummary(recovered.summary),
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
