#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitForPlayersReady,
  runKillRespawnDamageCycle, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";

// Only map geometry is a declared fixture. Extreme energy, weapon, recharge,
// bounty, spawn and death/respawn settings remain the shipped authoritative profile.
async function main() {
  const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/maxed_respawn.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const fixture = createWeaponLifecycleFixture({ zoneId: "extreme" });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zone: "extreme", zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "true" } });
  const browsers = [];
  const clients = [], cycles = [], screenshots = {};
  const urls = ["SpawnAce", "SpawnHyde"].map(name =>
    `http://127.0.0.1:${staticServer.port}/?arena=extreme&rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&password=secret&trace=1&packetlog=1&name=${name}`);
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "victim" : "shooter", client })), extraScreenshots: screenshots,
    extraLogs: { fixture: JSON.stringify(fixture, null, 2), cycles: JSON.stringify(cycles, null, 2),
      "shooter-browser": browsers[0]?.logs ?? [], "victim-browser": browsers[1]?.logs ?? [] },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  try {
    await waitForServerListening(port, server.child, server.logs);
    // Retain every started handle and enter cleanup even if browser preflight
    // refuses the first or second launch. Otherwise the already-running local
    // server/static listener keeps this failed target alive indefinitely.
    browsers.push(startChrome(await freePort(), "respawn-shooter"));
    browsers.push(startChrome(await freePort(), "respawn-victim"));
    await Promise.all(browsers.map(waitForChrome));
    clients.push(...await Promise.all(browsers.map((browser, i) => openChromeClient(browser, urls[i]))));
    await Promise.all(clients.map(c => c.enterShip()));
    const [shooter, victim] = clients;
    await waitForPlayersReady(shooter, victim, server);
    const [a, b] = await Promise.all(clients.map(async c => (await c.metricsState()).uiMetrics));
    for (const m of [a, b]) {
      assert.equal(m.playerMaxEnergy, 2800, "Extreme Warbird authored maximum");
      assert.equal(m.inSafeZone, false, "declared clear combat patch");
      assert.equal(m.localGameplayState.authorityPhase, "Live");
    }
    let sinceCount = 0;
    for (let cycle = 1; cycle <= 2; cycle++) {
      const result = await runKillRespawnDamageCycle({ cycle, shooter, victim, server,
        shooterId: a.selfPlayerId, victimId: b.selfPlayerId, sinceCount });
      cycles.push(result); sinceCount = result.nextSinceCount;
      const m = (await victim.metricsState()).uiMetrics;
      assert.equal(m.playerMaxEnergy, 2800);
      assert.equal(m.localGameplayState.authorityPhase, "Live");
      assert.equal(m.localGameplayState.networkPrizeBaselinePending, false);
      assert.ok(result.postRespawnDamage.victimLifeSeq > result.kill.victimLifeSeq);
      console.log(JSON.stringify({ cycle, killLife: result.kill.victimLifeSeq,
        nextLife: result.postRespawnDamage.victimLifeSeq, maxEnergy: m.playerMaxEnergy }));
    }
    screenshots.respawn = await victim.screenshot();
    const result = await evidence.succeed({ ok: true, arena: "extreme", fixture, cycles,
      scope: "two genuine-key lethal cycles, dead-input suppression, exact death/score and remote visibility, maxed respawn and post-respawn damage; map-only clear patch with unchanged Extreme settings" }, capture());
    console.log(JSON.stringify({ ok: result.ok, cycles: cycles.length }));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
