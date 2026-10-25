#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import { createSession, deleteSession, execute, firefoxArtifactClient, freePort,
  holdFirefoxKeys, metricsState, openClient, screenshot, sleep, startGeckodriver,
  startRealServer, startStaticServer, stopProcess, waitForGeckodriver,
  waitForServerListening, webdriver } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil,
  browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { hasPlayerShip } from "./player_ship_identity.mjs";
import { tabReturnVisibilityEvidence } from "./tab_return_visibility.mjs";

const browser = process.env.ZUBSPACE_E2E_BROWSER || "firefox";
assert.ok(["chrome", "firefox"].includes(browser));

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/tab_return_recovery.mjs" });
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: "0", env: { ZUBSPACE_SERVER_LOG_FORMAT: "json" } });
  const driver = browser === "chrome"
    ? startChrome(await freePort(), "tab-return", { allowBackgroundThrottling: true })
    : startGeckodriver(await freePort(), "tab-return");
  const url = `http://127.0.0.1:${staticServer.port}/?trace=1&packetlog=1&arena=chaos-zone&name=tabreturn&password=secret`;
  const checkpoints = [], visibility = [], screenshots = {};
  let actor = null, cdp = null;
  const capture = () => ({ server, serverPort: port, staticServer, urls: [url],
    clients: actor ? [{ label: browser, client: actor }] : [], extraScreenshots: screenshots,
    extraLogs: { checkpoints: JSON.stringify(checkpoints, null, 2),
      visibility: JSON.stringify(visibility, null, 2), browser: driver.logs },
    disconnect: async () => { await actor?.evaluate("(window.__zubspaceSendDisconnectForTest?.(), true)").catch(() => {}); },
  });
  const read = async () => (await actor.metricsState()).uiMetrics;
  const checkpoint = async label => {
    const state = await read();
    const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    const record = { label, at: Date.now(), state, status };
    checkpoints.push(record);
    console.log(JSON.stringify({ stage: label, quality: status.connection_quality,
      authority: state?.localGameplayState?.authorityPhase }));
    return record;
  };
  const switchAway = async () => {
    if (browser === "chrome") {
      const original = (await cdp("Target.getTargetInfo")).targetInfo.targetId;
      const away = (await cdp("Target.createTarget", { url: "about:blank" })).targetId;
      await cdp("Target.activateTarget", { targetId: away });
      return async () => {
        await cdp("Target.activateTarget", { targetId: original });
        await cdp("Target.closeTarget", { targetId: away });
      };
    }
    const original = await webdriver(driver, "GET", `/session/${driver.sessionId}/window`);
    const away = await webdriver(driver, "POST", `/session/${driver.sessionId}/window/new`, { type: "tab" });
    await webdriver(driver, "POST", `/session/${driver.sessionId}/window`, { handle: away.handle });
    return async () => {
      await webdriver(driver, "DELETE", `/session/${driver.sessionId}/window`);
      await webdriver(driver, "POST", `/session/${driver.sessionId}/window`, { handle: original });
    };
  };
  try {
    await waitForServerListening(port, server.child, server.logs);
    if (browser === "chrome") {
      await waitForChrome(driver);
      actor = await openChromeClient(driver, url, { beforeReady: async ({ send }) => { cdp = send; } });
    } else {
      await waitForGeckodriver(driver);
      await createSession(driver);
      await openClient(driver, url);
      actor = { ...firefoxArtifactClient(driver),
        metricsState: () => metricsState(driver),
        press: (key, ms = 100) => holdFirefoxKeys(driver, [key], ms),
        screenshot: async () => Buffer.from(await screenshot(driver), "base64") };
    }
    await waitUntil("initial playable server-owned life", async () =>
      (await read())?.localGameplayState?.authorityPhase === "Live", 30_000);
    // Establish genuine play. This is not an untouched-spawn idle-timeout test.
    await actor.press("ArrowUp", 500);
    await checkpoint("established-play");
    for (const hiddenMs of [3_000, 125_000]) {
      const beforeHidden = await read();
      await actor.evaluate(`(() => {
        window.__zubspaceTabReturnWitness = [{ at: Date.now(), hidden: document.hidden }];
        if (!window.__zubspaceTabReturnListener) {
          window.__zubspaceTabReturnListener = () => window.__zubspaceTabReturnWitness.push({ at: Date.now(), hidden: document.hidden });
          document.addEventListener("visibilitychange", window.__zubspaceTabReturnListener);
        }
        return true;
      })()`);
      const restore = await switchAway();
      try {
        // Do not poll the hidden page; host-side server reads cannot wake its renderer.
        const until = Date.now() + hiddenMs;
        while (Date.now() < until) {
          await sleep(Math.min(5_000, until - Date.now()));
          const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
          checkpoints.push({ label: "hidden-server-only", hiddenMs, at: Date.now(), status });
        }
      } finally { await restore(); }
      const events = await actor.evaluate("window.__zubspaceTabReturnWitness");
      visibility.push({ requestedHiddenMs: hiddenMs, events,
        ...tabReturnVisibilityEvidence(events, hiddenMs) });
      await actor.evaluate("(document.querySelector('#glcanvas')?.focus(), true)");
      await waitUntil("visible tab resumes its protocol loop", async () =>
        (await actor.evaluate("document.hidden")) === false, 10_000);
      await checkpoint(`visible-return-${hiddenMs}`);
      // A forced hold is permitted; it stays server-owned and keeps its real deadline.
      await waitUntil("original server cooldown expires", async () =>
        ((await read())?.localGameplayState?.connectionQualityHoldRemainingMs ?? 0) <= 0, 65_000);
      if ((await read())?.localGameplayState?.authorityPhase !== "Live") {
        if (!(await read()).localGameplayState.menuVisible) await actor.press("Escape");
        await actor.press("1");
        // One user request can recover the connection, but must not silently
        // spawn a ship after a long reconnect. The next selection is explicit.
        await waitUntil("connection recovered or direct healthy return admitted", async () => {
          const local = (await read())?.localGameplayState;
          return local?.authorityPhase === "Live" || local?.connectionQualityRecovery === "ready";
        }, 85_000);
        if ((await read()).localGameplayState.connectionQualityRecovery === "ready") {
          await waitUntil("fresh connection has healthy mature measurements", async () => {
            const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
            const quality = status.connection_quality;
            return quality.measured_sessions >= 1 && quality.max_session_average_rtt_ms < 400
              && quality.max_session_round_trip_probe_loss < 0.1;
          }, 40_000);
          const until = Date.now() + 11_000;
          while (Date.now() < until) {
            assert.equal((await read()).localGameplayState.authorityPhase, "Spectating");
            await sleep(250);
          }
          if (!(await read()).localGameplayState.menuVisible) await actor.press("Escape");
          await actor.press("1");
        }
      }
      await waitUntil("explicit selection restores the selected live ship", async () => {
        const state = await read();
        assert.ok(state.localGameplayState.rtcRejoinCount <= beforeHidden.localGameplayState.rtcRejoinCount + 1,
          "tab return cannot become a reconnect loop");
        return state.playerAlive && state.localGameplayState.authorityPhase === "Live"
          && state.localGameplayState.connectionQualityHoldRemainingMs === null && hasPlayerShip(state, 0);
      }, 85_000);
      const settled = await read();
      await actor.press("ArrowUp", 800);
      const moved = await read();
      assert.ok(Math.hypot(moved.playerX - settled.playerX, moved.playerY - settled.playerY) > 3,
        "visible return must resume genuine movement");
      const bridge = await actor.evaluate("window.__zubspaceGameKeyStateForTest?.()");
      assert.deepEqual(bridge?.trackedCodes, [], "released controls cannot remain held after tab return");
      await checkpoint(`live-and-moving-${hiddenMs}`);
    }
    screenshots.final = await actor.screenshot();
    const result = await evidence.succeed({ ok: true, browser, visibility,
      backgroundThrottlingDisabled: false,
      coverage: "real short and >2-minute hidden tabs, established play, server-owned cooldown, spectator recovery, explicit return and renewed motion" }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    const footer = browserE2eFailureFooter(artifacts);
    error.stack = `${error.stack || error.message}\n${footer}`;
    throw error;
  } finally {
    if (browser === "firefox") await deleteSession(driver).catch(() => {});
    else await actor?.close().catch(() => {});
    await Promise.all([stopProcess(driver.child), stopProcess(server.child)]).catch(() => {});
    if (driver.userDataDir) fs.rmSync(driver.userDataDir, { recursive: true, force: true });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
