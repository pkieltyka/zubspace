#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, sleep, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  driveClientToWorldTarget, brakeClientVelocity, orientClientForProtocolDirection,
  waitForCombatViewWeaponReady, parseServerDamageEvents, assertSingleBrowserDamageTrace,
  assertServerDamageAccounting, browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseServerProjectileEvents } from "../../tools/server_events.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { brickWeaponStage } from "./brick_weapon_stage.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/brick_weapons.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 514, radius: 2 } }), port = await freePort();
  const { geometry } = loadRendezvousMap("chaos-zone", [fixture.root]);
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json" } });
  const browsers = [], clients = [], urls = [], stages = [], screenshots = {};
  let ownerId, peerId;
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const spawns = () => parseServerProjectileEvents(server.logs).filter(e => e.type === "server_projectile_spawned");
  const damage = () => parseServerDamageEvents(server.logs);
  async function projectileStates(c) {
    const packets = await c.evaluate(`[...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
      .filter(e => e.direction === 'received' && e.bytes?.[0] === 125 && e.bytes[1] === 2 && e.bytes[2] === 4).map(e => e.bytes)`);
    const unique = new Map();
    for (const bytes of packets) {
      const p = Buffer.from(bytes);
      assert.ok(p.length === 122 || p.length === 124, "current projectile-state wire length");
      assert.equal(p.length, 122 + (p[105] & 8 ? 2 : 0));
      const state = { eventId: p.readUInt32LE(19), tick: p.readUInt32LE(23), ownerId: p.readUInt16LE(27),
        session: p.readBigUInt64LE(29).toString(), life: p.readUInt32LE(37), group: p.readBigUInt64LE(41).toString(),
        projectileId: p.readUInt32LE(74), childIndex: p[82], weapon: p.readUInt16LE(83),
        x: p.readFloatLE(85), y: p.readFloatLE(89), vx: p.readFloatLE(93), vy: p.readFloatLE(97), terminal: Boolean(p[105] & 4) };
      unique.set(state.eventId, state);
    }
    return [...unique.values()];
  }
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: ["owner", "peer", "observer"][i], client })),
    extraScreenshots: screenshots, extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  async function join(name) {
    const b = startChrome(await freePort(), `brick-weapon-${name}`); browsers.push(b); await waitForChrome(b);
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
    urls.push(url); const c = await openChromeClient(b, url); clients.push(c); await c.enterShip(); return c;
  }
  async function navigate(c, target, label) {
    const route = await driveClientToWorldTarget(c, target, 20_000, true, 3, null, 160);
    assert.ok(route.reached, `${label}: genuine flight must reach the staging point`);
    assert.ok((await brakeClientVelocity(c, 6000)).braked);
    stages.push({ label, target, route });
  }
  async function aim(c, direction) {
    const vector = direction === 0 ? [0, -1] : [0, 1];
    assert.ok((await orientClientForProtocolDirection(c, ...vector, 6000)).oriented);
    assert.equal((await metrics(c)).playerProtocolDirection, direction);
  }
  async function ready(c, label) {
    await waitUntil(`${label}: full energy and weapon cooldown`, async () => {
      const m = await metrics(c), l = m.localGameplayState;
      return m.playerAlive && m.gameplayStepReady && m.serverPlayerEnergy >= m.playerMaxEnergy
        && m.playerEnergy >= m.playerMaxEnergy && l.bombCooldown <= 0 && l.bulletCooldown <= 0;
    }, 20_000);
    await waitForCombatViewWeaponReady(c, label);
  }
  async function oneHit(shooter, victim, key, kind, label) {
    const a = await metrics(shooter), b = await metrics(victim), offset = damage().length;
    assert.equal(a.inSafeZone, false, `${label}: firing pilot must be outside safety`);
    assert.equal(b.inSafeZone, false, `${label}: victim must be outside safety`);
    await ready(shooter, label); await shooter.press(key, 45);
    let hit;
    await waitUntil(`${label}: exact authoritative hit`, () => {
      hit = damage().slice(offset).find(e => e.shooterId === a.selfPlayerId && e.victimId === b.selfPlayerId && (e.weapon & 31) === kind);
      return hit;
    }, 8000);
    assertServerDamageAccounting(hit, label);
    await Promise.all(clients.map((c, i) => assertSingleBrowserDamageTrace(c, hit, `${label} client ${i}`)));
    assert.equal((await metrics(victim)).playerLifeSeq, b.playerLifeSeq);
    stages.push({ label, hit }); return hit;
  }
  try {
    await waitForServerListening(port, server.child, server.logs);
    const owner = await join("brickgunner"), peer = await join("brickhostile"), observer = await join("brickwitness");
    await waitForPlayersReady(owner, peer, server);
    ownerId = (await metrics(owner)).selfPlayerId;
    peerId = (await metrics(peer)).selfPlayerId;
    await ready(observer, "observer initial baseline before spectator selection");
    await observer.press("Escape"); await observer.press("s");
    await waitUntil("observer admitted", async () => (await metrics(observer)).localGameplayState.observerReady, 8000);
    await ready(owner, "Terrier selection"); await owner.press("Escape"); await owner.press("5");
    await waitUntil("Terrier maxed life is ready", async () => {
      const m = await metrics(owner); return m.localGameplayState.gameplayStepReady && m.localGameplayState.gunLevel === 3
        && m.playerLifeSeq > 1
        && (await metrics(observer)).remoteActorPresentation.some(p => p.id === ownerId && p.ship === 4);
    }, 12_000);
    await navigate(peer, { x: 8200, y: 8320 }, "hostile hull below planned wall");
    await owner.press("4", 60);
    await waitUntil("all clients install ten-tile friendly/hostile wall", async () => {
      const m = await Promise.all(clients.map(metrics));
      return m.every(x => x.localGameplayState.brickTiles.length === 10)
        && m[0].localGameplayState.brickTiles.every(b => b.friendly)
        && m[1].localGameplayState.brickTiles.every(b => !b.friendly);
    }, 6000);
    const wall = (await metrics(owner)).localGameplayState.brickTiles;
    assert.ok(wall.every(b => b.y === wall[0].y), "neutral north-facing drop must be horizontal");
    const stage = brickWeaponStage(geometry, wall);
    const { wallTop, wallBottom, wallLeft, wallRight } = stage;
    stages.push({ label: "authored firing geometry", stage });
    await navigate(peer, stage.hostile, "hostile hull below actual wall");
    await navigate(owner, stage.gunner, "friendly gunner above wall");
    await aim(peer, 0);
    await ready(peer, "hostile bullet");
    const beforeHostile = spawns().length, damageBefore = damage().length;
    await peer.press("Control", 45);
    let launch, reflected;
    await waitUntil("hostile bouncing bullet reflects at brick in both observing views", async () => {
      launch = spawns().slice(beforeHostile).find(e => e.ownerId === (peerId) && (e.weapon & 31) === 2);
      if (!launch) return false;
      const views = await Promise.all([owner, observer].map(metrics));
      reflected = views.map(m => m.remoteProjectileSources.find(p => p.playerId === peerId && p.kind === "bullet"
        && p.shotGroupId === String(launch.shotGroupId) && p.velocityY > 0 && p.y >= wallBottom));
      return reflected.every(Boolean);
    }, 6000);
    await sleep(500);
    assert.ok(!damage().slice(damageBefore).some(e => e.shooterId === peerId && e.victimId === ownerId), "hostile brick must prevent the otherwise aligned hit");
    stages.push({ label: "hostile bullet reflected", launch, reflected });
    console.log(JSON.stringify({ stage: "hostile bullet reflected", projectile: launch.projectileId }));
    screenshots["hostile-wall"] = await observer.screenshot();
    await aim(owner, 20);
    const beforeFriendly = spawns().length;
    const friendlyHit = await oneHit(owner, peer, "Control", 2, "friendly Terrier bullets cross wall");
    const hitSpawn = spawns().find(e => e.projectileId === friendlyHit.projectileId);
    assert.ok(hitSpawn, "the damage must bind to a real accepted projectile");
    const barrels = spawns().slice(beforeFriendly).filter(e => e.ownerId === ownerId && e.shotGroupId === hitSpawn.shotGroupId);
    assert.equal(barrels.length, 2, "one Terrier linked activation has two normal barrels");
    assert.equal(barrels[0].y, barrels[1].y);
    assert.equal(Math.abs(barrels[0].x - barrels[1].x), 20, "cardinal double-barrel offsets truncate to +/-10 pixels");
    stages.push({ label: "Terrier double barrel", barrels });
    console.log(JSON.stringify({ stage: "friendly double barrel", damage: friendlyHit.damage }));
    // Keep the target beyond Thor's proximity sensor on the near side: a
    // blast through the wall alone would not prove projectile penetration.
    await navigate(owner, stage.thorTarget, "Thor target beyond far-side sensor margin");
    await aim(peer, 0);
    const thorHit = await oneHit(peer, owner, "5", 8, "hostile Thor penetrates brick");
    const thorContact = parseServerProjectileEvents(server.logs).find(e => e.type === "server_projectile_hit"
      && e.projectileId === thorHit.projectileId && e.victimId === ownerId);
    assert.ok(thorContact?.y < wallTop, "Thor itself must cross the wall before the far-side collision");
    stages.push({ label: "Thor far-side collision", thorContact });
    console.log(JSON.stringify({ stage: "Thor far-side collision", thorContact }));
    // Leave the brick owner's hull outside blast range. The real hostile bomb
    // must explode at the brick, independently of a direct enemy hull contact.
    await navigate(owner, { x: 8000, y: 8160 }, "owner leaves wall explosion footprint");
    await ready(peer, "hostile bomb");
    const beforeBomb = spawns().length;
    await peer.press("Tab", 45);
    let bomb, fans;
    await waitUntil("hostile wall bomb generates bound authoritative fragments in all clients", async () => {
      bomb = spawns().slice(beforeBomb).find(e => e.ownerId === peerId && [3,4].includes(e.weapon & 31));
      if (!bomb) return false;
      const views = await Promise.all(clients.map(metrics));
      fans = views.map(m => m.remoteProjectileSources.filter(p => p.playerId === peerId && p.kind === "shrapnel"
        && p.shotGroupId === String(bomb.shotGroupId) && p.authorityProjectileId != null));
      return fans.every(fan => fan.length > 0);
    }, 6000);
    for (const fan of fans) {
      assert.equal(new Set(fan.map(p => p.authorityProjectileId)).size, fan.length, "fragment identities are unique");
    }
    const states = (await projectileStates(observer)).filter(p => p.group === String(bomb.shotGroupId) && p.ownerId === peerId);
    const terminal = states.find(p => p.terminal && p.projectileId === bomb.projectileId);
    assert.ok(terminal, "a server terminal must bind the parent wall explosion");
    assert.ok(terminal.y >= wallBottom && terminal.y < wallBottom + 16 && terminal.x >= wallLeft && terminal.x < wallRight,
      "the parent must explode immediately outside the hostile brick face");
    const children = states.filter(p => !p.terminal && (p.weapon & 31) === 15);
    assert.ok(children.length > 0 && children.length <= 8);
    assert.equal(new Set(children.map(p => p.projectileId)).size, children.length);
    assert.ok(children.every(p => p.x === terminal.x && p.y === terminal.y
      && ((p.weapon >> 5) & 3) === ((bomb.weapon >> 5) & 3)), "children inherit explosion position and owner-requested parent power");
    for (const fan of fans) assert.ok(fan.every(p => children.some(s => s.projectileId === p.authorityProjectileId
      && s.childIndex === p.sourceProjectileIndex && s.life === p.lifeSeq && s.session === p.sessionId)));
    stages.push({ label: "hostile wall explosion fan", bomb, terminal, children, fans });
    screenshots["wall-fragments"] = await observer.screenshot();
    console.log(JSON.stringify(await evidence.succeed({ ok: true, fixture,
      scope: "hostile bullet reflection/no hit, friendly linked Terrier hit and barrel spacing, Thor penetration, server-bound wall-explosion fragments; no fragment damage or full cross-browser claim",
      friendlyHit, barrelSpacing: 20, hostileLaunch: launch, thorPenetrates: true, wallBomb: bomb, fragmentCounts: fans.map(f => f.length) }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
// IDs are assigned by the real roster rather than encoded into gameplay input.
main().catch(error => { console.error(error.stack || error); process.exit(1); });
