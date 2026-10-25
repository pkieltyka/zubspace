#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitForPlayersReady,
  waitUntil, driveClientToWorldTarget, brakeClientVelocity, fireUntilDamage,
  waitForRespawnCycle, parseServerDamageEvents, browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseServerDeathEvents } from "../../tools/server_events.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/arena_kill_notifications.mjs" });
  // This notification case requires stationary firing at the same origin
  // through both lives. Shipping randomized spawns are unchanged.
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 0 } });
  const port = await freePort();
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root,
    env: { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json" } });
  const browsers = [], clients = [], urls = [], observations = [], screenshots = {};
  let staticServer;
  const names = ["NoticeAce", "NoticeVictim", "NoticeWatch"];
  const metrics = async client => (await client.metricsState()).uiMetrics;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: names[i], client })),
    extraScreenshots: screenshots,
    extraLogs: { observations: JSON.stringify(observations, null, 2), fixture: JSON.stringify(fixture),
      ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  try {
    await waitForServerListening(port, server.child, server.logs);
    staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
    for (const name of names) {
      const browser = startChrome(await freePort(), name); browsers.push(browser);
      await waitForChrome(browser);
      const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
      urls.push(url);
      const client = await openChromeClient(browser, url); clients.push(client);
      await client.enterShip();
    }
    const [shooter, victim, observer] = clients;
    await waitForPlayersReady(shooter, victim, server);
    await waitForPlayersReady(shooter, observer, server);
    // Keep the uninvolved player out of the genuine firing line.
    const observerRoute = await driveClientToWorldTarget(observer, { x: 8400, y: 8200 }, 15000, true, 5);
    assert.ok(observerRoute.reached, `uninvolved observer setup failed: ${JSON.stringify(observerRoute)}`);
    const observerBrake = await brakeClientVelocity(observer);
    assert.ok(observerBrake.braked, "uninvolved observer must settle outside the firing line");
    const shooterId = (await metrics(shooter)).selfPlayerId;
    const victimId = (await metrics(victim)).selfPlayerId;
    for (const role of ["player", "spectator"]) {
      if (role === "spectator") {
        await waitUntil("observer full energy", async () => {
          const m = await metrics(observer);
          return m.playerAlive && m.serverPlayerAlive && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
        }, 10000);
        await observer.press("Escape"); await observer.press("s");
        await waitUntil("observer becomes spectator", async () => (await metrics(observer)).localGameplayState.observerReady, 10000);
      }
      const before = await metrics(observer), victimBefore = await metrics(victim);
      const shooterBefore = await metrics(shooter);
      assert.ok(Math.hypot(victimBefore.playerX - shooterBefore.playerX,
        victimBefore.playerY - shooterBefore.playerY) <= 4,
      `${role} notice requires the declared collocated stationary origin`);
      const beforeDeaths = parseServerDeathEvents(server.logs).length;
      const hit = await fireUntilDamage({ label: `${role} sees arena kill`, shooter, victim,
        server, shooterId, victimId, sinceCount: parseServerDamageEvents(server.logs).length,
        requireLethal: true, singlePressMs: 60 });
      await waitUntil("exact committed death", async () => parseServerDeathEvents(server.logs).length > beforeDeaths, 3000);
      const deaths = parseServerDeathEvents(server.logs).slice(beforeDeaths);
      assert.equal(deaths.length, 1);
      const death = deaths[0]; assert.equal(death.damageSeq, hit.event.damageSeq);
      const message = `${names[1]}(${death.killedBounty}) killed by: ${names[0]}`;
      let observed;
      await waitUntil(`${role}: one arena notification`, async () => {
        observed = await metrics(observer);
        return observed.combatKillNotificationCount === before.combatKillNotificationCount + 1 &&
          observed.notificationMessages.filter(s => s === message).length === 1;
      }, 5000);
      assert.equal(observed.selfWins, before.selfWins);
      assert.equal(observed.selfLosses, before.selfLosses);
      await waitForRespawnCycle(victim, victimBefore.playerRespawnSeq);
      const after = await metrics(observer);
      assert.equal(after.combatKillNotificationCount, before.combatKillNotificationCount + 1,
        "snapshots/respawn cannot replay a global notification");
      observations.push({ role, death, message, countBefore: before.combatKillNotificationCount,
        countAfter: after.combatKillNotificationCount, observerId: observed.selfPlayerId });
      screenshots[role] = await observer.screenshot();
    }
    const result = await evidence.succeed({ ok: true, observations,
      scope: "genuine-key human kills reach an uninvolved active player and then spectator exactly once; private score stays unchanged" }, capture());
    console.log(JSON.stringify({ ok: result.ok, observations }));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    if (error.stack) error.stack += `\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    if (staticServer) await new Promise(resolve => staticServer.server.close(resolve));
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
