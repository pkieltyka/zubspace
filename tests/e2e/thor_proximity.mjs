#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitForPlayersReady, waitUntil,
  driveClientToWorldTarget, brakeClientVelocity, orientClientForProtocolDirection,
  waitForCombatViewWeaponReady, parseServerDamageEvents, assertSingleBrowserDamageTrace,
  assertServerDamageAccounting, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { parseRealtimeWeaponEvent } from "../../tools/realtime_event_evidence.mjs";

// Unchanged Chaos weapon settings, authored clear terrain, ordinary real-key
// navigation. The miss-distance excludes direct hull contact: only the actual
// Thor proximity sensor can produce the correlated primary damage.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/thor_proximity.mjs" });
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 2 }, clearBounds: { minX: 488, maxX: 544, minY: 488, maxY: 544 } });
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json",
    ZUBSPACE_SERVER_QA_TRACE_UPLOAD: "1",
  } });
  const browsers = [], clients = [], urls = [], stages = [], screenshots = {};
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: ["owner", "target"][i], client })),
    extraScreenshots: screenshots, extraLogs: { fixture: JSON.stringify(fixture), stages: JSON.stringify(stages, null, 2),
      ...Object.fromEntries(browsers.map((browser, i) => [`browser-${i}`, browser.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c =>
      c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  async function join(name) {
    const browser = startChrome(await freePort(), name); browsers.push(browser); await waitForChrome(browser);
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
    urls.push(url); const client = await openChromeClient(browser, url); clients.push(client);
    await client.enterShip(); return client;
  }
  async function navigate(client, point, label) {
    const route = await driveClientToWorldTarget(client, point, 20_000, true, 3, null, 160);
    assert.ok(route.reached, `${label}: real navigation reaches staging point`);
    assert.ok((await brakeClientVelocity(client, 6000)).braked, `${label}: residual drift meets the shared braking bound`);
    stages.push({ label, point, route, metrics: await metrics(client) });
  }
  async function weaponFacts(client) {
    const packets = await client.evaluate(`[...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
      .filter(entry => entry.direction === 'received' && entry.bytes?.[0] === 125 && entry.bytes[1] === 2 && entry.bytes[2] === 1)
      .map(entry => entry.bytes)`);
    return packets.map(bytes => parseRealtimeWeaponEvent(Buffer.from(bytes))).filter(Boolean);
  }
  try {
    await waitForServerListening(port, server.child, server.logs);
    const owner = await join("ThorOwner"), target = await join("ThorTarget");
    await waitForPlayersReady(owner, target, server);
    await navigate(owner, { x: 8200, y: 8500 }, "owner beyond initial sensor reach");
    assert.ok((await orientClientForProtocolDirection(owner, 0, -1, 5000)).oriented);
    // Shared braking permits <=3.5px/s drift. Stage this pilot last, with room
    // inside the 108px square sensor, rather than letting it drift outside
    // while the shooter navigates and turns for several seconds.
    await navigate(target, { x: 8285, y: 8200 }, "target lateral to the Thor flight line");
    await waitUntil("full live loadouts before one Thor edge", async () => {
      const states = await Promise.all(clients.map(metrics));
      return states.every(m => m.playerAlive && m.gameplayStepReady && m.playerEnergy >= m.playerMaxEnergy &&
        m.serverPlayerEnergy >= m.playerMaxEnergy && !m.localGameplayState.networkPrizeBaselinePending);
    }, 15_000);
    await Promise.all(clients.map((c, i) => waitForCombatViewWeaponReady(c, `Thor pilot ${i}`)));
    const before = await metrics(owner), targetBefore = await metrics(target);
    stages.push({ label: "before the single real Thor edge", before, targetBefore });
    const ownerId = before.selfPlayerId, targetId = targetBefore.selfPlayerId;
    const damageOffset = parseServerDamageEvents(server.logs).length;
    const prior = new Set((await weaponFacts(target)).map(fact => fact.eventId));
    await owner.press("5", 60);
    let hit;
    await waitUntil("offset Thor proximity produces real primary damage", () => {
      hit = parseServerDamageEvents(server.logs).slice(damageOffset).find(event =>
        event.shooterId === ownerId && event.victimId === targetId && event.weapon === 8 && event.kind === "primary");
      return hit;
    }, 7000);
    const parent = (await weaponFacts(target)).find(fact => fact.source === ownerId && !prior.has(fact.eventId) && fact.weapon === 8);
    assert.ok(parent, "peer receives immutable stock Thor without upgraded power or fragments");
    assert.equal((await metrics(owner)).localGameplayState.thors, before.localGameplayState.thors - 1);
    assertServerDamageAccounting(hit, "stock offset Thor");
    await Promise.all(clients.map((c, i) => assertSingleBrowserDamageTrace(c, hit, `Thor client ${i}`)));
    const joined = await target.freezeQaTrace({ upload: true });
    const witness = joined.server.events.find(event => event.type === "server_collision_witness" &&
      event.projectile_id === hit.projectileId && event.target_id === targetId && event.shot_group_id === parent.shotGroupId);
    // Preserve the real backend witness before checking its geometry, so a
    // failure cannot discard the raw assertion inputs at the cold checkpoint.
    stages.push({ label: "Thor collision checkpoint", before, targetBefore, parent, hit, witness, joined });
    assert.ok(witness, "correlated server trace retains actual primary geometry");
    assert.ok(["proximity_closest_approach", "proximity_deadline"].includes(witness.contact_reason));
    const dx = Math.abs(Math.trunc(witness.projectile_x) - Math.trunc(witness.collision_anchor_x));
    const dy = Math.abs(Math.trunc(witness.projectile_y) - Math.trunc(witness.collision_anchor_y));
    assert.ok(dx > 50 && dx < 108 && dy < 108,
      `Thor detonates outside direct hull contact within the stock square sensor: dx=${dx}, dy=${dy}`);
    assert.equal(witness.explosion_radius, 320);
    // Match the reference integer peak and f32 radial falloff against the
    // authoritative frozen anchor, rather than a later browser position.
    const expected = Math.trunc(Math.fround(Math.fround(320 - witness.damage_center_distance) * Math.fround(750 / 320)));
    assert.equal(hit.unprotectedDamage, expected);
    assert.equal(witness.unprotected_damage, expected);
    assert.equal(hit.damage, expected, "no unrequested shields or other protection changes the sample");
    assert.equal(hit.empUntilTick, 0, "Thor is not an EMP bomb");
    const explosion = joined.server.events.find(event => event.type === "server_projectile_explosion_committed" &&
      event.parentProjectileId === hit.projectileId && event.shotGroupId === parent.shotGroupId);
    assert.ok(explosion, "the exact Thor completes its authoritative explosion");
    assert.equal(explosion.fragmentCount, 0);
    assert.equal((await metrics(target)).playerLifeSeq, targetBefore.playerLifeSeq);
    stages.push({ label: "stock Thor proximity and damage", before, targetBefore, parent, hit, witness, explosion });
    screenshots["owner-after-Thor"] = await owner.screenshot();
    screenshots["target-after-Thor"] = await target.screenshot();
    console.log(JSON.stringify(await evidence.succeed({ ok: true, parent, hit, witness, explosion,
      scope: "genuine-key stock Chaos Thor hits a laterally offset real human through proximity without direct contact, reference falloff from frozen geometry, exact damage in both clients, one inventory spent, no EMP or fragments; all raw levels and diagonal/outside boundaries have separate native coverage" }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
