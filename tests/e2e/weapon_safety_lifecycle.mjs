#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  waitForCombatViewWeaponReady, driveClientToWorldTarget, brakeClientVelocity, orientClientForProtocolDirection,
  fireUntilDamage, waitForRespawnCycle, parseServerDamageEvents, mergedPacketLog,
  assertSingleBrowserDamageTrace, assertServerDamageAccounting, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { retainedProjectileAfterDamageDeath } from "../../tools/dynamic_zone_weapon_schedule.mjs";
import { sameDecoyEntity } from "../../tools/realtime_event_evidence.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const origin = { x: 8200, y: 8200 };
const safeTarget = { x: 8200, y: 8120 };


function cancellations(state, ownerId) {
  const unique = new Map();
  for (const entry of mergedPacketLog(state)) {
    const bytes = Buffer.from(entry.bytes || []);
    if (entry.direction !== "received" || bytes.length !== 52 || bytes[0] !== 0x7d || bytes[1] !== 2 || bytes[2] !== 5) continue;
    if (bytes.readUInt16LE(27) !== ownerId) continue;
    const event = { eventId: bytes.readUInt32LE(19), tick: bytes.readUInt32LE(23),
      session: bytes.readBigUInt64LE(29).toString(), through: bytes.readBigUInt64LE(37).toString(),
      life: bytes.readUInt32LE(45), sequence: bytes[49] & 1 ? bytes.readUInt16LE(50) : null };
    unique.set(event.eventId, event);
  }
  return [...unique.values()];
}

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/weapon_safety_lifecycle.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 0 } }), port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json" } });
  const browsers = [startChrome(await freePort(), "safety-owner"), startChrome(await freePort(), "safety-witness")];
  const urls = browsers.map((_, i) => `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=safety${i}`);
  const clients = [], stages = [], screenshots = {};
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "witness" : "owner", client })), extraScreenshots: screenshots,
    extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      "owner-browser": browsers[0].logs, "witness-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(client => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  const metrics = async client => (await client.metricsState()).uiMetrics;
  async function ready(client, label) {
    await waitUntil(`${label}: cooldown and authority`, async () => {
      const m = await metrics(client);
      return m.playerAlive && m.serverPlayerAlive && m.localGameplayState.authorityPhase === "Live" && m.bombCooldown <= 0;
    }, 10_000);
    await waitForCombatViewWeaponReady(client, label);
  }
  async function mine(client, label) {
    await ready(client, label);
    await client.keyDown("Shift");
    try { await client.press("Tab", 60); } finally { await client.keyUp("Shift"); }
  }
  async function decoy(client, label) { await ready(client, label); await client.press("3", 60); }
  async function navigate(client, target, label) {
    const result = await driveClientToWorldTarget(client, target, 15_000, true, 5, null, 160);
    stages.push({ label, target, result });
  }
  async function select(client, key) { await client.press("Escape"); await client.press(key); }
  async function prepareDeathLine(owner, witness, label) {
    // Finish spatial preparation before starting the Decoy's authored15s
    // lifetime. A radius2 witness could sit32px off the gun line and wait for
    // the slowly coasting owner to drift into range, expiring it naturally.
    await navigate(owner, { x: origin.x, y: origin.y - 32 }, label);
    assert.ok((await brakeClientVelocity(owner)).braked);
    assert.ok((await orientClientForProtocolDirection(witness, 0, -1, 4000)).oriented);
    await ready(witness, `${label}: firing witness`);
    const [a, b] = await Promise.all([metrics(owner), metrics(witness)]);
    assert.ok(Math.abs(a.playerX - b.playerX) <= 11 &&
      a.playerY - b.playerY >= -48 && a.playerY - b.playerY <= -16,
      "death fixture must put the live owner ahead of the genuine gun muzzle");
    stages.push({ label: `${label}: prepared gun line`,
      owner: { x: a.playerX, y: a.playerY }, witness: { x: b.playerX, y: b.playerY, facing: b.playerOrientation } });
  }
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) { clients.push(await openChromeClient(browsers[i], urls[i])); await clients[i].enterShip(); }
    const [owner, witness] = clients;
    await waitForPlayersReady(owner, witness, server);
    const initial = await metrics(owner), ownerId = initial.selfPlayerId, witnessId = (await metrics(witness)).selfPlayerId;
    assert.equal(initial.inSafeZone, false);
    assert.ok(initial.localGameplayState.decoys >= 2, "maxed loadout needs two Decoys");
    await select(witness, "s");
    await waitUntil("witness spectator admission", async () => (await metrics(witness)).localGameplayState.observerReady, 10_000);
    await mine(owner, "first mine");
    let firstMine;
    await waitUntil("observer sees first authoritative mine", async () => {
      firstMine = (await metrics(witness)).remoteProjectileSources.find(p => p.playerId === ownerId && p.kind === "mine");
      return firstMine?.authorityProjectileId != null;
    }, 5000);
    await decoy(owner, "first Decoy");
    await waitUntil("observer sees first Decoy", async () => (await metrics(witness)).remoteDecoys.some(p => p.playerId === ownerId), 5000);
    screenshots["before-safe"] = await witness.screenshot();
    await navigate(owner, safeTarget, "first safe entry");
    await waitUntil("safety silently clears both clients", async () => {
      const [a, b] = await Promise.all([metrics(owner), metrics(witness)]);
      return a.inSafeZone && a.localGameplayState.mines === 0 && a.localGameplayState.decoysActive === 0 &&
        !b.remoteProjectileSources.some(p => p.playerId === ownerId) && !b.remoteDecoys.some(p => p.playerId === ownerId);
    }, 5000);
    const firstCuts = cancellations(await witness.pageState(), ownerId);
    assert.ok(firstCuts.some(c => c.session === firstMine.sessionId && BigInt(c.through) >= BigInt(firstMine.shotGroupId)));
    stages.push({ label: "first safety cancellation", firstMine, cuts: firstCuts });
    screenshots["after-safe"] = await witness.screenshot();
    await navigate(owner, origin, "leave safety");
    await mine(owner, "post-egress mine");
    let newerMine;
    await waitUntil("newer launch survives prior cutoff", async () => {
      newerMine = (await metrics(witness)).remoteProjectileSources.find(p => p.playerId === ownerId && p.kind === "mine");
      return newerMine?.authorityProjectileId != null && BigInt(newerMine.shotGroupId) > BigInt(firstCuts.at(-1).through);
    }, 5000);
    stages.push({ label: "post-egress launch", mine: newerMine });
    // Enter safety again to clean the test's own mine before returning the
    // observer to a combat hull at spawn. This is ordinary game navigation.
    await navigate(owner, safeTarget, "clear post-egress mine");
    await waitUntil("second safe entry clears mine", async () => !(await metrics(witness)).remoteProjectileSources.some(p => p.playerId === ownerId), 5000);
    await ready(owner, "full ship reset");
    await waitUntil("full energy for reset", async () => { const m = await metrics(owner); return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy; }, 10000);
    await select(owner, "s");
    await waitUntil("owner reset spectator admission", async () => (await metrics(owner)).localGameplayState.observerReady, 10_000);
    await select(owner, "1");
    await select(witness, "1");
    await waitForPlayersReady(owner, witness, server);
    await navigate(owner, { x: 8440, y: origin.y }, "stage predecessor mine away from spawn");
    await mine(owner, "predecessor mine");
    let oldMine;
    await waitUntil("observer sees predecessor mine", async () => {
      oldMine = (await metrics(witness)).remoteProjectileSources.find(p => p.playerId === ownerId && p.kind === "mine");
      return oldMine?.authorityProjectileId != null;
    }, 5000);
    await prepareDeathLine(owner, witness, "return to combat death line");
    await decoy(owner, "death Decoy");
    let beforeDecoy;
    await waitUntil("death Decoy is visible", async () => {
      beforeDecoy = (await metrics(witness)).remoteDecoys.find(p => p.playerId === ownerId);
      return beforeDecoy;
    }, 5000);
    const beforeDeath = await metrics(owner), previousRespawn = beforeDeath.playerRespawnSeq;
    stages.push({ label: "death-lifetime setup", oldMine, beforeDecoy,
      ownerLifeSeq: beforeDeath.playerLifeSeq, previousRespawn });
    let deadWitness;
    const kill = await fireUntilDamage({ label: "Decoy/mine owner death", shooter: witness, victim: owner,
      server, shooterId: witnessId, victimId: ownerId, sinceCount: parseServerDamageEvents(server.logs).length,
      requireLethal: true, singlePressMs: 60,
      onServerDamageEvent: async () => {
        await waitUntil("Decoy and predecessor mine survive confirmed owner death", async () => {
          const [a, b] = await Promise.all([metrics(owner), metrics(witness)]);
          deadWitness = { ownerAlive: a.playerAlive, serverAlive: a.serverPlayerAlive, decoys: a.localGameplayState.decoysActive,
            remoteDecoy: b.remoteDecoys.find(p => sameDecoyEntity(p, beforeDecoy)),
            mine: b.remoteProjectileSources.find(p => p.authorityProjectileId === oldMine.authorityProjectileId) };
          return !deadWitness.ownerAlive && !deadWitness.serverAlive && deadWitness.decoys >= 1 && deadWitness.remoteDecoy && deadWitness.mine;
        }, 1500);
      } });
    await waitForRespawnCycle(owner, previousRespawn);
    const afterRespawn = await metrics(owner), observerRespawn = await metrics(witness);
    assert.ok(afterRespawn.playerLifeSeq > oldMine.lifeSeq);
    const retainedOldMine = retainedProjectileAfterDamageDeath(oldMine, afterRespawn.playerLifeSeq, observerRespawn.remoteProjectileSources);
    assert.ok(retainedOldMine && retainedOldMine.authorityProjectileId === oldMine.authorityProjectileId,
      "death preserves every immutable source field and exact mine identity");
    const afterDecoy = observerRespawn.remoteDecoys.find(p => sameDecoyEntity(p, beforeDecoy));
    assert.ok(afterDecoy && afterDecoy.ttl < beforeDecoy.ttl, "Decoy must retain its original age through death/respawn");
    stages.push({ label: "death and respawn preserve source lifetime", oldMine, retainedOldMine, beforeDecoy, kill: kill.event, deadWitness, afterDecoy, successorLife: afterRespawn.playerLifeSeq });
    screenshots["after-respawn"] = await witness.screenshot();
    await navigate(owner, safeTarget, "successor life enters safety");
    await waitUntil("successor safety clears older-life mine and Decoy", async () => {
      const m = await metrics(witness);
      return !m.remoteProjectileSources.some(p => p.playerId === ownerId) && !m.remoteDecoys.some(p => p.playerId === ownerId);
    }, 5000);
    const cuts = cancellations(await witness.pageState(), ownerId);
    assert.ok(cuts.some(c => c.life === afterRespawn.playerLifeSeq && c.session === oldMine.sessionId && BigInt(c.through) >= BigInt(oldMine.shotGroupId)));
    stages.push({ label: "successor safety cancellation", cuts });

    // A separate fresh mine preserves the prior cancellation proof above.
    // This time leave it active through death, place the successor on it, and
    // have the hostile witness trigger it with ordinary movement.
    await waitUntil("full energy before self-splash setup", async () => {
      const m = await metrics(owner);
      return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
    }, 10_000);
    await select(owner, "s");
    await waitUntil("self-splash setup observer admission", async () => (await metrics(owner)).localGameplayState.observerReady, 10_000);
    await select(owner, "1");
    await waitForPlayersReady(owner, witness, server);
    await navigate(owner, { x: 8440, y: origin.y }, "stage self-splash mine");
    await mine(owner, "self-splash predecessor mine");
    let splashMine;
    await waitUntil("self-splash mine has authoritative identity", async () => {
      splashMine = (await metrics(witness)).remoteProjectileSources.find(p => p.playerId === ownerId && p.kind === "mine");
      return splashMine?.authorityProjectileId != null;
    }, 5000);
    await prepareDeathLine(owner, witness, "return for second damage death");
    const secondPreviousRespawn = (await metrics(owner)).playerRespawnSeq;
    const secondKill = await fireUntilDamage({ label: "old-mine source damage death", shooter: witness, victim: owner,
      server, shooterId: witnessId, victimId: ownerId, sinceCount: parseServerDamageEvents(server.logs).length,
      requireLethal: true, singlePressMs: 60 });
    await waitForRespawnCycle(owner, secondPreviousRespawn);
    const splashSuccessor = await metrics(owner);
    assert.ok(splashSuccessor.playerLifeSeq > splashMine.lifeSeq);
    assert.ok((await metrics(witness)).remoteProjectileSources.some(p => p.authorityProjectileId === splashMine.authorityProjectileId && p.lifeSeq === splashMine.lifeSeq));
    await navigate(owner, { x: splashMine.x, y: splashMine.y }, "successor approaches own predecessor mine");
    await brakeClientVelocity(owner);
    const beforeSplash = await metrics(owner);
    assert.equal(beforeSplash.playerLifeSeq, splashSuccessor.playerLifeSeq);
    const damageOffset = parseServerDamageEvents(server.logs).length;
    const selfSplash = () => parseServerDamageEvents(server.logs).slice(damageOffset).find(event =>
      event.projectileId === splashMine.authorityProjectileId && event.shooterId === ownerId && event.victimId === ownerId &&
      event.shooterLifeSeq === splashMine.lifeSeq && event.victimLifeSeq === splashSuccessor.playerLifeSeq);
    const triggerRoute = await driveClientToWorldTarget(witness, { x: splashMine.x, y: splashMine.y },
      15_000, false, 5, selfSplash, 120);
    await waitUntil("old owned mine splashes the exact successor life", () => selfSplash(), 5000);
    const splash = selfSplash();
    assertServerDamageAccounting(splash, "old-owner successor self-splash");
    assert.ok(splash.damage > 0 && splash.energyAfter < splash.energyBefore);
    assert.equal(splash.lethal, false, "reference own-bomb survival floor must remain intact");
    assert.ok(splash.energyAfter >= 1);
    await Promise.all([
      assertSingleBrowserDamageTrace(owner, splash, "successor receives old-own splash"),
      assertSingleBrowserDamageTrace(witness, splash, "witness receives successor splash"),
    ]);
    await waitUntil("both clients retire the exploded old mine", async () => {
      const [a, b] = await Promise.all([metrics(owner), metrics(witness)]);
      return !a.remoteProjectileSources.some(p => p.authorityProjectileId === splashMine.authorityProjectileId) &&
        !b.remoteProjectileSources.some(p => p.authorityProjectileId === splashMine.authorityProjectileId);
    }, 5000);
    stages.push({ label: "older own mine damages exact successor", splashMine, sourceDeath: secondKill.event,
      successorLife: splashSuccessor.playerLifeSeq, beforeSplash: { x: beforeSplash.playerX, y: beforeSplash.playerY, energy: beforeSplash.playerEnergy },
      triggerRoute, splash });
    screenshots["after-old-owner-splash"] = await owner.screenshot();
    const result = await evidence.succeed({ ok: true, ownerId, witnessId, fixture, stages,
      coverage: "genuine keys: mine/Decoy safety cut, newer post-egress launch, mine and Decoy across confirmed death/respawn, same-session successor-life safety cut, old-owned mine splash against exact successor with survival floor and both-client receipts; no altered combat settings" }, capture());
    console.log(JSON.stringify({ ok: result.ok, stages: stages.map(s => s.label), artifacts: process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR }));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    const footer = browserE2eFailureFooter(artifacts);
    error.message += `\n${footer}`;
    if (error.stack) error.stack += `\n${footer}`;
    throw error;
  } finally {
    await Promise.all(clients.map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    await new Promise(resolve => staticServer.server.close(resolve));
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
