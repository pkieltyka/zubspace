#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, sleep, startRealServer, startStaticServer, stopProcess,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil,
  browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// The real 120-second server policy, genuine keyboard input and ordinary RTC.
// Native tests separately cover repeated idle deaths and exact loading pauses.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/spawn_idle.mjs" });
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, env: {
    ZUBSPACE_SERVER_LOG_FORMAT: "json", ZUBSPACE_SERVER_QA_TRACE_UPLOAD: "1",
  } });
  const browsers = [], clients = [], urls = [], checkpoints = [], screenshots = {};
  const local = async client => (await client.metricsState()).uiMetrics;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: ["active", "idle"][i], client })),
    extraScreenshots: screenshots,
    extraLogs: { checkpoints: JSON.stringify(checkpoints, null, 2),
      ...Object.fromEntries(browsers.map((browser, i) => [`browser-${i}`, browser.logs])) },
    disconnect: async () => { await Promise.all(clients.map(client =>
      client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  async function join(name) {
    const browser = startChrome(await freePort(), name); browsers.push(browser);
    await waitForChrome(browser);
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
    urls.push(url);
    const client = await openChromeClient(browser, url); clients.push(client);
    await client.enterShip();
    await waitUntil(`${name} controllable`, async () => {
      const m = await local(client);
      return m.gameplayStepReady && m.playerAlive && m.localGameplayState.authorityPhase === "Live";
    }, 20_000);
    return client;
  }
  try {
    await waitForServerListening(port, server.child, server.logs);
    const active = await join("ActiveGrace");
    const activeBefore = await local(active);
    await active.press("ArrowUp", 400);
    await waitUntil("active control has genuinely moved", async () => {
      const m = await local(active);
      return Math.hypot(m.playerX - activeBefore.playerX, m.playerY - activeBefore.playerY) > 3;
    }, 3000);
    const activeLife = (await local(active)).playerLifeSeq;
    // Leave this participant alone for the entire timeout window. The other
    // participant receives no gameplay action until its spectator return.
    const idle = await join("IdleGrace");
    const initial = await local(idle), started = Date.now();
    checkpoints.push({ stage: "idle-ready", at: started, initial, active: await local(active) });
    let forced = null, nextCheckpoint = started + 15_000;
    while (Date.now() - started < 135_000) {
      const [idleState, activeState] = await Promise.all([local(idle), local(active)]);
      assert.equal(activeState.playerLifeSeq, activeLife);
      assert.equal(activeState.localGameplayState.authorityPhase, "Live", "an already active life has no general AFK timeout");
      assert.equal(activeState.localGameplayState.connectionQualityHoldRemainingMs, null);
      if (idleState.localGameplayState.authorityPhase === "Spectating" && idleState.localGameplayState.observerReady) {
        forced = { at: Date.now(), state: idleState, active: activeState };
        break;
      }
      if (Date.now() >= nextCheckpoint) {
        const elapsedMs = Date.now() - started;
        checkpoints.push({ stage: "waiting", elapsedMs, idle: idleState, active: activeState });
        console.log(JSON.stringify({ stage: "spawn-idle-wait", elapsedMs }));
        nextCheckpoint += 15_000;
      }
      await sleep(500);
    }
    assert.ok(forced, "inactive controllable fresh spawn must become spectator after two minutes");
    assert.ok(forced.at - started >= 118_000, "the actual two-minute grace must be served");
    assert.equal(forced.state.playerLifeSeq, (initial.playerLifeSeq + 1) >>> 0 || 1);
    assert.equal(forced.state.selfPlayerId, initial.selfPlayerId);
    assert.equal(forced.state.localGameplayState.connectionQualityHoldRemainingMs, null);
    assert.equal(forced.state.notificationMessages.filter(message =>
      message === "Moved to spectator: no movement or firing after spawning.").length, 1);
    checkpoints.push({ stage: "forced", ...forced });
    screenshots["idle-spectator"] = await idle.screenshot();
    if (!(await local(idle)).localGameplayState.menuVisible) await idle.press("Escape");
    await idle.press("1");
    await waitUntil("idle spectator returns immediately without a quality hold", async () => {
      const m = await local(idle);
      assert.equal(m.selfPlayerId, initial.selfPlayerId);
      return m.playerAlive && m.gameplayStepReady && m.localGameplayState.authorityPhase === "Live";
    }, 15_000);
    const beforeMove = await local(idle);
    await idle.press("ArrowUp", 500);
    const afterMove = await local(idle);
    assert.ok(Math.hypot(afterMove.playerX - beforeMove.playerX, afterMove.playerY - beforeMove.playerY) > 3);
    assert.equal(afterMove.localGameplayState.connectionQualityHoldRemainingMs, null);
    checkpoints.push({ stage: "returned-and-moved", state: afterMove });
    const decisions = server.logs.join("").split("\n").filter(Boolean)
      .map(line => { try { return JSON.parse(line); } catch { return null; } })
      .filter(line => line?.fields?.event === "spawn_idle_spectator");
    assert.equal(decisions.length, 1, "only the inactive life is forced, exactly once");
    assert.equal(decisions[0].fields.player_id, initial.selfPlayerId);
    console.log(JSON.stringify(await evidence.succeed({ ok: true, forcedAfterMs: forced.at - started,
      decisions, coverage: "real two-minute inactive initial life, active-life exemption, exact one-life forced spectator, clear reason, no quality hold, immediate menu return and genuine thrust; repeated death/loading boundaries are native coverage" }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
