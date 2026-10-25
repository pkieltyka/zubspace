#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  driveClientToWorldTarget, brakeClientVelocity, waitForCombatViewWeaponReady,
  parseServerDamageEvents, assertSingleBrowserDamageTrace, assertServerDamageAccounting,
  browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseRealtimeWeaponEvent } from "../../tools/realtime_event_evidence.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { referenceFragmentDirections } from "./shrapnel_reference.mjs";
import { collectArtifactProvenance } from "./artifact_provenance.mjs";
import { assertQaServerBinaryProvenance, assertQaWasmProvenance } from "./qa_trace_artifacts.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
async function main() {
  const identity = { e2eScript: "tests/e2e/shrapnel_damage.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) };
  const evidence = createFocusedBrowserEvidence(identity), built = collectArtifactProvenance(identity);
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 2 }, clearBounds: { minX: 488, maxX: 544, minY: 488, maxY: 544 } });
  const port = await freePort(), staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json",
    ZUBSPACE_SERVER_QA_TRACE_UPLOAD: "1", ZUBSPACE_SERVER_SIGNALING_ALLOWED_ORIGINS: `http://127.0.0.1:${staticServer.port}` } });
  const browsers = [], clients = [], urls = [], stages = [], phaseTraces = [], screenshots = {};
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const damage = () => parseServerDamageEvents(server.logs);
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: ["owner", "trigger", "target"][i], client })),
    extraScreenshots: screenshots, extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      "phase-qa-traces": JSON.stringify(phaseTraces), ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  async function join(name) {
    const b = startChrome(await freePort(), `fragment-${name}`); browsers.push(b); await waitForChrome(b);
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${name}`;
    urls.push(url); const c = await openChromeClient(b, url); clients.push(c); await c.enterShip(); return c;
  }
  async function ready(c, label) {
    await waitUntil(`${label}: full live energy and current private loadout`, async () => {
      const m = await metrics(c), l = m.localGameplayState;
      return m.playerAlive && m.gameplayStepReady && !l.networkPrizeBaselinePending && l.prizeAuthorityLifeSeq === m.playerLifeSeq
        && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy && l.bombCooldown <= 0;
    }, 20_000);
    await waitForCombatViewWeaponReady(c, label);
  }
  async function navigate(c, point, label) {
    const route = await driveClientToWorldTarget(c, point, 20_000, true, 3, null, 160);
    assert.ok(route.reached, `${label}: real navigation reached staging point`);
    assert.ok((await brakeClientVelocity(c, 6000)).braked);
    stages.push({ label, point, route });
  }
  async function reset(c, key) {
    await ready(c, "reset selection"); await c.press("Escape"); await c.press("s");
    await waitUntil("reset spectator admitted", async () => (await metrics(c)).localGameplayState.observerReady, 8000);
    await c.press("Escape"); await c.press(key);
    await ready(c, "new player life");
  }
  async function weapons(c) {
    const packets = await c.evaluate(`[...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
      .filter(e => e.direction === 'received' && e.bytes?.[0] === 125 && e.bytes[2] === 1).map(e => e.bytes)`);
    const facts = new Map();
    for (const p of packets) { const fact = parseRealtimeWeaponEvent(Buffer.from(p)); if (fact) facts.set(fact.eventId, fact); }
    return [...facts.values()];
  }
  try {
    await waitForServerListening(port, server.child, server.logs);
    const owner = await join("fragowner"), trigger = await join("fragtrigger"), target = await join("fragtarget");
    await waitForPlayersReady(owner, target, server);
    const ownerId = (await metrics(owner)).selfPlayerId, targetId = (await metrics(target)).selfPlayerId;
    for (const [shipKey, level, name] of [["1", 2, "Warbird"], ["4", 3, "Leviathan"]]) {
      if (shipKey !== "1") await Promise.all([reset(owner, shipKey), reset(trigger, "1"), reset(target, "1")]);
      await Promise.all(clients.map((c, i) => ready(c, `${name} pilot ${i}`)));
      await navigate(owner, { x: 8200, y: 8320 }, `${name} mine site`);
      await ready(owner, `${name} mine energy`);
      const known = new Set((await weapons(target)).map(w => w.eventId));
      await owner.keyDown("Shift"); try { await owner.press("Tab", 60); } finally { await owner.keyUp("Shift"); }
      let parent, mine;
      await waitUntil(`${name} authoritative stationary mine and immutable launch`, async () => {
        parent = (await weapons(target)).find(w => w.source === ownerId && !known.has(w.eventId) && (w.weapon & 0x8000));
        if (!parent) return false;
        mine = (await metrics(target)).remoteProjectileSources.find(p => p.playerId === ownerId && p.kind === "mine" && p.shotGroupId === parent.shotGroupId);
        return mine?.authorityProjectileId != null;
      }, 6000);
      assert.equal((parent.weapon >> 5 & 3) + 1, level);
      const count = parent.weapon >> 10 & 31; assert.equal(count, 8);
      const directions = referenceFragmentDirections({ x: mine.x, y: mine.y, count, level, frequency: parent.frequency });
      const direction = directions.find(d => Math.abs(d.x) > 0.8);
      assert.ok(direction, `${name}: reference fan has a horizontal staging ray in the clear patch`);
      const distance = level*80 + 60;
      const point = { x: mine.x + direction.x*distance, y: mine.y + direction.y*distance };
      // The owner leaves before the target is staged, so its remaining drift
      // cannot spoil the narrow fragment interception during a long departure.
      await navigate(owner, { x: 8520, y: 8320 }, `${name} owner outside blast`);
      await navigate(target, { x: mine.x + Math.sign(direction.x)*distance, y: 8200 }, `${name} target avoids the live mine sensor`);
      await navigate(target, point, `${name} target beyond blast on reference fragment ray`);
      await waitForCombatViewWeaponReady(target, `${name} target receipt`);
      const targetBefore = await metrics(target), damageOffset = damage().length;
      assert.ok(Math.hypot(targetBefore.playerX-mine.x,targetBefore.playerY-mine.y) > level*80 + 30);
      // Genuine motion trips the normal proximity sensor; there is no damage,
      // explosion, weapon or teleport injection.
      const route = await driveClientToWorldTarget(trigger, { x: mine.x, y: mine.y-40 }, 12_000, true, 4, null, 160);
      assert.ok(route.reached, `${name}: real trigger pilot reaches mine proximity`);
      let hit;
      await waitUntil(`${name} real active-fragment damage beyond the blast`, () => {
        hit = damage().slice(damageOffset).find(e => e.shooterId === ownerId && e.victimId === targetId && (e.weapon & 31) === 15);
        return hit;
      }, 7000);
      const joined = await target.freezeQaTrace({ upload: true }); phaseTraces.push({ name, joined });
      assert.equal(joined.schema, "zubspace-qa-trace-joined-v1");
      assertQaServerBinaryProvenance(joined.browser, built); assertQaWasmProvenance(joined.browser, built);
      const explosion = joined.server.events.find(e => e.type === "server_projectile_explosion_committed"
        && e.parentProjectileId === mine.authorityProjectileId && String(e.shotGroupId) === parent.shotGroupId);
      assert.ok(explosion, `${name}: correlated server QA proves the parent committed its fan`);
      assert.equal(explosion.fragmentCount, 8);
      const packets = await target.evaluate(`[...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
        .filter(e => e.direction === 'received' && e.bytes?.[0] === 125 && e.bytes[1] === 2 && e.bytes[2] === 4).map(e => e.bytes)`);
      const child = packets.map(p => Buffer.from(p)).find(p => [122,124].includes(p.length)
        && !(p[105] & 4) && p.readUInt32LE(74) === hit.projectileId);
      assert.ok(child, `${name}: damaging child has an authenticated server state`);
      assert.equal(child.readBigUInt64LE(41).toString(), parent.shotGroupId);
      assert.equal(child.readBigUInt64LE(29).toString(), parent.sourceSessionId);
      assert.equal(child.readUInt32LE(37), parent.sourceLifeSeq);
      assert.equal(child.readUInt16LE(27), ownerId);
      const reference = directions[child[82]];
      assert.ok(reference && Math.abs(child.readFloatLE(93)-reference.x*300) <= 0.11
        && Math.abs(child.readFloatLE(97)-reference.y*300) <= 0.11,
        "damaging child's actual velocity matches its reference random fan index");
      assert.equal(child.readUInt32LE(23), explosion.commitTick);
      assert.equal(hit.weapon >> 5 & 3, level-1);
      assert.equal(hit.damage, 200 + 100*(level-1));
      assert.ok(hit.serverTick > explosion.commitTick + 25, "active damage must occur beyond the reference inactive window");
      assert.ok(!damage().slice(damageOffset).some(e => e.victimId === targetId && [3,4,8].includes(e.weapon & 31)), "distant victim must take no parent blast damage");
      assertServerDamageAccounting(hit, name);
      await Promise.all(clients.map((c, i) => assertSingleBrowserDamageTrace(c, hit, `${name} client ${i}`)));
      assert.equal((await metrics(target)).playerLifeSeq, targetBefore.playerLifeSeq);
      stages.push({ label: `${name} fragment hit`, parent, mine, directions, chosenDirection: direction,
        point, targetBefore, route, hit, explosion, childStatePacket: [...child] });
      screenshots[name.toLowerCase()] = await target.screenshot();
      console.log(JSON.stringify({ stage: name, fragmentDamage: hit.damage, ageTicks: hit.serverTick-explosion.commitTick, fragmentCount: explosion.fragmentCount }));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, fixture,
      scope: "genuine maxed Warbird/Leviathan mines create eight authoritative fragments and deal level-two/three active-fragment damage to a real player beyond parent blast radius, exact damage receipts in three views; not all artwork loops or full lifecycle/browser coverage",
      fragmentHits: stages.filter(s => s.label.endsWith("fragment hit")).map(s => s.hit), phaseQaCaptures: phaseTraces.length }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
