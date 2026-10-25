#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening,
  startGeckodriver, waitForGeckodriver, createSession, openClient, deleteSession,
  metricsState, firefoxArtifactClient, holdFirefoxKeys, releaseFirefoxKeys, screenshot } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { waitUntil, browserE2eFailureFooter } from "./combat_replay.mjs";
import { stageChaosSafeZonePilot } from "./keyboard_chord_matrix.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { authoredSafetyMargin } from "./authored_map_geometry.mjs";
import { serverObserverProfileForNetworkProfile } from "./server_observer_profile.mjs";

// Narrow diagnostic: run the actual safety coordinator in real Firefox at the
// retained broad/thin pockets. Only twelve authored Spawn fields change; the
// complete shipped map, balance and16px clearance requirement are preserved.
async function main() {
  const pocket = process.env.ZUBSPACE_E2E_SAFETY_POCKET || "broad";
  assert(["broad", "thin"].includes(pocket));
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/chaos_safety_staging.mjs" });
  const spawnRegion = pocket === "thin" ? { x: 485, y: 593, radius: 0 } : { x: 630, y: 434, radius: 0 };
  const fixture = createSpawnRegionFixture({ zoneId: "chaos-zone", spawnRegion });
  const geometry = loadRendezvousMap("chaos-zone", [fixture.root]).geometry;
  const safety = authoredSafetyMargin((x, y) => geometry.tileAt(x, y) === 171);
  const hull = authoredSafetyMargin((x, y) => geometry.authoredHullIsTraversable(x, y));
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root,
    observerNetworkProfile: serverObserverProfileForNetworkProfile("mild"),
    env: { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
  const driver = startGeckodriver(await freePort(), `safety-${pocket}`);
  const url = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=SafetyProbe&password=secret&trace=1&packetlog=1&network_profile=mild&network_profile_seed=271828`;
  let actor, stage, before, after;
  const capture = () => ({ server, serverPort: port, staticServer, urls: [url],
    clients: actor ? [{ label: "firefox", client: actor }] : [],
    extraLogs: { fixture: JSON.stringify(fixture), stages: JSON.stringify({ pocket, before, stage, after }), browser: driver.logs },
    disconnect: async () => { await actor?.evaluate("window.__zubspaceSendDisconnectForTest?.()"); } });
  try {
    await waitForServerListening(port, server.child, server.logs);
    await waitForGeckodriver(driver); await createSession(driver); await openClient(driver, url);
    actor = { ...firefoxArtifactClient(driver), metricsState: () => metricsState(driver),
      screenshot: () => screenshot(driver),
      hold: (key, ms) => holdFirefoxKeys(driver, [key], ms),
      press: (key, ms) => holdFirefoxKeys(driver, [key], ms), releaseKeys: () => releaseFirefoxKeys(driver) };
    await waitUntil("real Firefox ship and private baseline ready", async () => {
      const m = (await actor.metricsState()).uiMetrics;
      return m?.playerAlive && m.gameplayStepReady && m.localGameplayState?.authorityPhase === "Live";
    }, 30_000);
    before = (await actor.metricsState()).uiMetrics;
    assert(safety(before.playerX, before.playerY) < 16, "the declared start requires genuine centring");
    stage = await stageChaosSafeZonePilot(actor);
    after = (await actor.metricsState()).uiMetrics;
    assert.equal(after.playerLifeSeq, before.playerLifeSeq);
    assert.equal(after.selfCorrectionCount, 0);
    assert(after.inSafeZone && safety(after.playerX, after.playerY) >= 16 && hull(after.playerX, after.playerY) >= 16);
    assert(Math.hypot(after.playerVelocityX, after.playerVelocityY) <= 3.5);
    await actor.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest?.()");
    let networkProfile;
    await waitUntil("staging fragment impairment timers drain", async () => {
      networkProfile = await actor.evaluate("window.__zubspaceNetworkProfileStateForTest?.()");
      return networkProfile?.measurementEnded && ["s2c", "c2s"].every(k =>
        networkProfile[k].pending === 0 && networkProfile[k].drainPending === 0);
    }, 10_000);
    console.log(JSON.stringify(await evidence.succeed({ ok: true, diagnosticOnly: true, pocket, before, stage, after,
      networkProfile,
      safetyPx: safety(after.playerX, after.playerY), hullPx: hull(after.playerX, after.playerY),
      scope: "genuine Firefox key steering through the actual safety coordinator at a declared authored pocket; not the complete keyboard/impairment matrix" }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await deleteSession(driver).catch(() => {});
    await Promise.all([stopProcess(driver.child), stopProcess(server.child)]).catch(() => {});
    staticServer.server.close(); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
