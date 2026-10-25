#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening, sleep } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// Genuine keyboard input, real RTC server and the real 60-second policy.
// The only test control changes DataChannel delivery, never gameplay state.
async function main() {
  const profile = process.env.ZUBSPACE_E2E_QUALITY_PROFILE || "degraded-latency";
  assert.ok(["degraded-latency", "degraded-loss"].includes(profile));
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/connection_quality_enforcement.mjs" });
  const port = await freePort(), metricsPort = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { env: {
    ZUBSPACE_SERVER_LOG_FORMAT: "json", ZUBSPACE_SERVER_METRICS_LISTEN: `127.0.0.1:${metricsPort}`,
  } });
  const browsers = [startChrome(await freePort(), "quality-held"), startChrome(await freePort(), "quality-healthy")];
  const root = `http://127.0.0.1:${staticServer.port}/`;
  const base = `${root}?trace=1&packetlog=1&packetlog_bytes=256&arena=chaos-zone&password=secret`;
  const urls = [`${base}&name=qualityheld&network_profile=${profile}&network_profile_seed=271828&network_profile_armed=1`, `${base}&name=qualityhealthy`];
  let alpha = null, beta = null;
  const checkpoints = [], screenshots = {};
  let metricsText = "";
  const quality = async () => (await (await fetchGameStatusResponse(`${root}v1/status`)).json()).connection_quality;
  const local = async client => {
    const m = (await client.metricsState()).uiMetrics;
    const remaining = m.localGameplayState.connectionQualityHoldRemainingMs;
    const seconds = Math.ceil((remaining ?? 0) / 1000);
    assert.equal(m.localGameplayState.connectionQualityCountdown, seconds > 0
      ? `Connection cooldown: ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`
      : null, "HUD countdown follows the remaining hold and disappears at zero");
    return m;
  };
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: [{ label: "held", client: alpha }, { label: "healthy", client: beta }],
    extraScreenshots: screenshots,
    extraLogs: { checkpoints: JSON.stringify(checkpoints, null, 2), prometheus: metricsText,
      "held-browser": browsers[0].logs, "healthy-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all([alpha, beta].filter(Boolean).map(client => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const checkpoint = async label => {
    const data = { label, at: Date.now(), quality: await quality(), alpha: await local(alpha), beta: await local(beta) };
    checkpoints.push(data);
    console.log(JSON.stringify({ stage: label, at: data.at, quality: data.quality }));
    return data;
  };
  const expectHealthyOther = async () => {
    const m = await local(beta);
    assert.equal(m.localGameplayState.authorityPhase, "Live");
    assert.equal(m.localGameplayState.connectionQualityHoldRemainingMs, null);
  };
  const requestShip = async () => {
    if (!(await local(alpha)).localGameplayState.menuVisible) await alpha.press("Escape");
    await alpha.press("1");
  };
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    [alpha, beta] = await Promise.all(browsers.map((browser, i) => openChromeClient(browser, urls[i])));
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await Promise.all([alpha.press("ArrowUp", 500), beta.press("ArrowUp", 500)]);
    await waitUntil("healthy baseline windows", async () => {
      const q = await quality();
      return q.measured_sessions === 2 && q.samples >= 85;
    }, 35_000);
    assert.equal(await alpha.evaluate("window.__zubspaceBeginNetworkProfileMeasurementForTest()"), true);
    await alpha.press("ArrowUp", 3000);
    assert.equal(await alpha.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest()"), true);
    await checkpoint("brief-spike-ended");
    const recoveryDeadline = Date.now() + 35_000;
    while (Date.now() < recoveryDeadline) {
      assert.equal((await quality()).forced_total, 0, "brief spike must not force a spectator");
      assert.equal((await local(alpha)).localGameplayState.authorityPhase, "Live");
      await expectHealthyOther();
      await sleep(1000);
    }
    await checkpoint("brief-spike-recovered");
    assert.equal(await alpha.evaluate("window.__zubspaceBeginNetworkProfileMeasurementForTest()"), true);
    await alpha.press("Control", 500);
    let forced;
    await waitUntil("sustained degradation forces the exact peer into spectator", async () => {
      await expectHealthyOther();
      const m = await local(alpha), q = await quality();
      if (q.forced_total !== 1 || m.localGameplayState.authorityPhase !== "Spectating" || !m.localGameplayState.observerReady) return false;
      forced = { at: Date.now(), state: m, quality: q };
      return true;
    }, 85_000);
    assert.ok(forced.state.localGameplayState.connectionQualityHoldRemainingMs > 55_000 &&
      forced.state.localGameplayState.connectionQualityHoldRemainingMs <= 60_000,
      "newly forced observer serves the one-minute hold, allowing the original five-second observation lag");
    assert.equal(forced.quality.retained_holds, 1);
    screenshots["forced-spectator"] = await alpha.screenshot();
    const realized = await alpha.evaluate("window.__zubspaceNetworkStats");
    if (profile === "degraded-latency") {
      assert.ok(forced.quality.max_session_average_rtt_ms >= 600);
      assert.ok(realized.networkProfileS2cDelaySamples > 20 && realized.networkProfileC2sDelaySamples > 20);
      assert.ok(realized.networkProfileS2cDelayMaxMs >= 330 && realized.networkProfileC2sDelayMaxMs >= 330);
    } else {
      assert.ok(forced.quality.max_session_round_trip_probe_loss >= 0.15);
      assert.ok(realized.networkProfileS2cQualityDropped + realized.networkProfileC2sQualityDropped >= 5);
      assert.ok(realized.networkProfileS2cSnapshotDropped > 0 && realized.networkProfileC2sInputDropped > 0);
    }
    await requestShip();
    await waitUntil("early return denied without leaving spectator", async () => {
      const m = await local(alpha);
      return (await quality()).return_denied_total >= 1 && m.localGameplayState.authorityPhase === "Spectating" && m.localGameplayState.spectatorMode;
    }, 10_000);
    assert.ok(!(await local(alpha)).notificationMessages.includes("Playing."), "a rejected ship request cannot announce success");
    const beforePan = await local(alpha);
    await alpha.press("ArrowRight", 500);
    const afterPan = await local(alpha);
    assert.ok(afterPan.playerX - beforePan.playerX > 10, "held spectator retains free camera");
    await alpha.press("Control");
    const target = (await local(alpha)).localGameplayState.spectatingPlayerId;
    assert.equal(target, (await local(beta)).selfPlayerId);
    await sleep(2300);
    assert.equal((await local(alpha)).localGameplayState.spectatingPlayerId, target, "periodic hold status must preserve follow");
    await checkpoint("held-denial-and-camera");
    // Real new transport, same persisted browser identity, and no impairment.
    const abruptCloseStarted = Date.now();
    await alpha.close();
    alpha = null;
    // The delayed unload frame cannot outlive the tab. RTC's normal ICE
    // contract allows 5s to Disconnected plus 25s to Failed; only Failed/Closed
    // retires the server session, so a 20s deadline would reject valid cleanup.
    await waitUntil("old transport removed after the normal ICE failure window", async () => (await (await fetchGameStatusResponse(`${root}v1/status`)).json()).joinedPlayers === 1, 45_000);
    checkpoints.push({ label: "abrupt-transport-cleanup", at: Date.now(), elapsedMs: Date.now() - abruptCloseStarted });
    const reconnectUrl = `${base}&name=qualityheld` + (profile === "degraded-loss"
      ? `&network_profile=${profile}&network_profile_seed=271828&network_profile_armed=1` : "");
    urls.push(reconnectUrl);
    alpha = await openChromeClient(browsers[0], reconnectUrl);
    await waitUntil("reconnected identity remains held observer", async () => {
      const m = await local(alpha);
      return m.localGameplayState.observerReady && m.localGameplayState.spectatorMode && m.localGameplayState.connectionQualityHoldRemainingMs > 0;
    }, 20_000);
    const reconnected = await checkpoint("reconnected-held");
    assert.ok(reconnected.alpha.localGameplayState.connectionQualityHoldRemainingMs < forced.state.localGameplayState.connectionQualityHoldRemainingMs);
    const deniedBefore = reconnected.quality.return_denied_total;
    await requestShip();
    await waitUntil("reconnect cannot bypass cooldown", async () => (await quality()).return_denied_total > deniedBefore, 10_000);
    if (profile === "degraded-loss") {
      assert.equal(await alpha.evaluate("window.__zubspaceBeginNetworkProfileMeasurementForTest()"), true);
    }
    const waitStart = Date.now();
    while ((await local(alpha)).localGameplayState.connectionQualityHoldRemainingMs > 0) {
      assert.ok(Date.now() - waitStart < 65_000, "reconnect must not restart the hold");
      assert.equal((await quality()).forced_total, 1);
      await expectHealthyOther();
      await sleep(1000);
    }
    assert.ok(Date.now() - forced.at >= 58_000, "the real one-minute server hold must be served");
    screenshots["cooldown-expired"] = await alpha.screenshot();
    let beforeReturn;
    if (profile === "degraded-loss") {
      // Expiry alone must not authorize an unhealthy path. Keep real loss
      // running through the deadline. One genuine request owns one fresh
      // spectator connection; returning to play needs a new explicit selection.
      await sleep(1000);
      const expired = await checkpoint("expired-still-unhealthy");
      assert.ok(expired.quality.max_session_round_trip_probe_loss >= 0.1);
      await requestShip();
      await waitUntil("expired but unhealthy return denied", async () => {
        const m = await local(alpha), q = await quality();
        return q.return_denied_total > expired.quality.return_denied_total
          && q.retained_holds === 1 && m.localGameplayState.authorityPhase !== "Live";
      }, 10_000);
      assert.ok(!(await local(alpha)).notificationMessages.includes("Playing."), "an expired but unhealthy return cannot announce success");
      screenshots["expired-unhealthy-denial"] = await alpha.screenshot();
      await waitUntil("one automatic fresh connection preserves the expired server hold", async () => {
        await expectHealthyOther();
        const m = await local(alpha);
        assert.ok(m.localGameplayState.rtcRejoinCount <= expired.alpha.localGameplayState.rtcRejoinCount + 1,
          "expired return cannot become a reconnect loop");
        if (m.localGameplayState.rtcRejoinCount !== expired.alpha.localGameplayState.rtcRejoinCount + 1
          || !m.localGameplayState.observerReady
          || m.localGameplayState.connectionQualityRecovery !== "ready") return false;
        assert.notEqual(m.localGameplayState.authorityScopeArenaStreamId,
          expired.alpha.localGameplayState.authorityScopeArenaStreamId);
        assert.equal(m.localGameplayState.connectionQualityHoldRemainingMs, 0);
        assert.equal(m.localGameplayState.connectionQualityRecovery, "ready");
        assert.equal(m.localGameplayState.authorityPhase, "Spectating");
        assert.equal(m.localGameplayState.connectionQualityRecoveryShip, 0);
        beforeReturn = m;
        return true;
      }, 40_000);
      // New transport alone cannot grant play. Keep actual degradation through
      // the new session's minimum mature window, retaining server authority.
      const unhealthyUntil = Date.now() + 13_000;
      while (Date.now() < unhealthyUntil) {
        const m = await local(alpha);
        assert.notEqual(m.localGameplayState.authorityPhase, "Live");
        assert.equal((await quality()).retained_holds, 1);
        assert.equal(m.localGameplayState.rtcRejoinCount, beforeReturn.localGameplayState.rtcRejoinCount);
        await expectHealthyOther();
        await sleep(250);
      }
      await checkpoint("automatic-recovery-still-held-on-bad-path");
      assert.equal(await alpha.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest()"), true);
    }
    // Both profiles reconnect with a fresh measurement window. Serving the
    // hold alone does not establish the required twenty mature samples plus
    // ten sustained healthy seconds. The latency arm previously requested its
    // only return at22.1s after reconnect, before the earliest22.5s verdict.
    await waitUntil("fresh measured recovery below hysteresis thresholds", async () => {
      await expectHealthyOther();
      const q = await quality();
      return q.measured_sessions === 2 && q.max_session_average_rtt_ms < 400
        && q.max_session_round_trip_probe_loss < 0.1;
    }, 65_000);
    // Allow the sustained-health window, with no input. Recovery must never
    // silently respawn a player who looked away after requesting a reconnect.
    const spectatorUntil = Date.now() + 11_000;
    while (Date.now() < spectatorUntil) {
      assert.equal((await local(alpha)).localGameplayState.authorityPhase, "Spectating");
      await sleep(250);
    }
    await checkpoint("expired-path-recovered");
    beforeReturn = await local(alpha);
    await requestShip();
    await waitUntil("healthy return commits after cooldown", async () => {
      const m = await local(alpha);
      assert.equal(m.selfPlayerId, beforeReturn.selfPlayerId, "return must preserve its admitted transport identity");
      assert.equal(m.localGameplayState.authorityScopeArenaStreamId,
        beforeReturn.localGameplayState.authorityScopeArenaStreamId,
        "healthy return must remain in the held arena stream");
      assert.equal(m.localGameplayState.rtcRejoinCount, beforeReturn.localGameplayState.rtcRejoinCount);
      checkpoints.push({ label: "healthy-return-pending", at: Date.now(),
        selfPlayerId: m.selfPlayerId, state: m.localGameplayState });
      return m.playerAlive && m.localGameplayState.authorityPhase === "Live" && m.localGameplayState.connectionQualityHoldRemainingMs === null;
    }, profile === "degraded-loss" ? 35_000 : 20_000);
    const beforeMotion = await local(alpha);
    await alpha.press("ArrowUp", 800);
    const afterMotion = await local(alpha);
    assert.ok(Math.hypot(afterMotion.playerX - beforeMotion.playerX, afterMotion.playerY - beforeMotion.playerY) > 3);
    await expectHealthyOther();
    const final = await checkpoint("healthy-return-and-motion");
    assert.equal(final.quality.retained_holds, 0);
    assert.equal(final.quality.healthy_return_total, 1);
    metricsText = await (await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text();
    assert.ok(metricsText.includes("zubspace_connection_quality_forced_total 1\n"));
    assert.ok(metricsText.includes("zubspace_connection_quality_healthy_return_total 1\n"));
    const logs = server.logs.join("").split("\n").slice(0, -1).filter(Boolean).map(line => JSON.parse(line));
    const decisions = logs.filter(log => log.fields?.message === "connection quality decision");
    assert.equal(decisions.filter(log => log.fields.decision === "forced_spectator").length, 1);
    assert.equal(decisions.filter(log => log.fields.decision === "return_accepted").length, 1);
    assert.ok(decisions.some(log => log.fields.decision === "return_denied"));
    if (profile === "degraded-loss") {
      assert.ok(decisions.some(log => log.fields.decision === "return_denied" && log.fields.remaining_ms === 0));
    }
    screenshots["healthy-return"] = await alpha.screenshot();
    const result = await evidence.succeed({ ok: true, profile, realized, forced, decisions,
      coverage: "real healthy baseline, brief spike, sustained degradation, exact forced spectator, early denial, free camera/follow, reconnect hold, real one-minute cooldown, healthy return and keyboard motion" }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    const originalStack = error.stack || error.message;
    const footer = browserE2eFailureFooter(artifacts);
    error.message += `\n${footer}`;
    error.stack = `${originalStack}\n${footer}`;
    throw error;
  } finally {
    await Promise.all([alpha, beta].filter(Boolean).map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
