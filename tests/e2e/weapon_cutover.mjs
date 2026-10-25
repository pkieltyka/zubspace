#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  waitForCombatViewWeaponReady, driveClientToWorldTarget, mergedPacketLog,
  browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { parseRealtimeOwnerWeaponCancellation, parseReliableOwnerWeaponCancellation } from "../../tools/realtime_event_evidence.mjs";
import { collectArtifactProvenance } from "./artifact_provenance.mjs";
import { assertQaServerBinaryProvenance, assertQaWasmProvenance } from "./qa_trace_artifacts.mjs";
import { readCutoverWireEvidence } from "./cutover_wire_observation.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
function cuts(state, id) {
  return mergedPacketLog(state).flatMap(entry => {
    const packet = Buffer.from(entry.bytes || []);
    if (entry.direction !== "received") return [];
    const fact = parseReliableOwnerWeaponCancellation(packet) || parseRealtimeOwnerWeaponCancellation(packet);
    if (!fact || fact.source !== id) return [];
    return [{ event: fact.eventId, session: fact.sourceSessionId, through: fact.throughShotGroupId,
      life: fact.ownerLifeSeq, reliableId: fact.reliableId ?? null }];
  });
}
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/weapon_cutover.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 2 } }), port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json" } });
  const browsers = [startChrome(await freePort(), "cutover-owner"), startChrome(await freePort(), "cutover-peer")];
  const urls = browsers.map((_, i) => `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=cutover${i}`);
  const clients = [], stages = [], screenshots = {};
  let preDeparture = null;
  let peerAfterDeparture = null;
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "peer" : "owner", client })), extraScreenshots: screenshots,
    extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      "owner-before-leave": JSON.stringify(preDeparture),
      "peer-after-leave": JSON.stringify(peerAfterDeparture),
      "owner-browser": browsers[0].logs, "peer-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  async function full(c) {
    await waitUntil("full live ship energy", async () => { const m = await metrics(c); return m.playerAlive && m.serverPlayerAlive && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy && m.localGameplayState.authorityPhase === "Live"; }, 12_000);
  }
  async function weaponReady(c) {
    const m = await metrics(c);
    assert.equal(m.localGameplayState.menuVisible, false, "weapon input requires the gameplay keyboard layer");
    assert.equal(m.chatActive, false, "weapon input must not be typed into chat");
    await waitUntil("weapon cooldown", async () => (await metrics(c)).bombCooldown <= 0, 8000);
    await waitForCombatViewWeaponReady(c, "cutover launch");
  }
  async function mine(c) {
    await weaponReady(c); await c.keyDown("Shift");
    try { await c.press("Tab", 60); } finally { await c.keyUp("Shift"); }
  }
  async function select(c, key) { await c.press("Escape"); await c.press(key); }
  const ownMines = (m, id) => m.remoteProjectileSources.filter(p => p.playerId === id && p.kind === "mine");
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) { clients.push(await openChromeClient(browsers[i], urls[i])); await clients[i].enterShip(); }
    const [owner, peer] = clients;
    await waitForPlayersReady(owner, peer, server);
    const ownerId = (await metrics(owner)).selfPlayerId, peerId = (await metrics(peer)).selfPlayerId;
    const route = await driveClientToWorldTarget(peer, { x: 8440, y: 8200 }, 15_000, true, 5, null, 160);
    assert.equal(route.reached, true);
    await mine(peer);
    let foreign;
    await waitUntil("foreign mine has exact identity", async () => { foreign = ownMines(await metrics(owner), peerId)[0]; return foreign?.authorityProjectileId != null; }, 5000);
    for (const [label, key] of [["ship change", "2"], ["spectator change", "s"]]) {
      const before = await metrics(owner);
      await mine(owner);
      let oldMine;
      await waitUntil(`${label}: own mine reaches peer`, async () => { oldMine = ownMines(await metrics(peer), ownerId)[0]; return oldMine?.authorityProjectileId != null; }, 5000);
      await weaponReady(owner); await owner.press("3", 60);
      let oldDecoy;
      await waitUntil(`${label}: own Decoy reaches peer`, async () => { oldDecoy = (await metrics(peer)).remoteDecoys.find(p => p.playerId === ownerId); return oldDecoy; }, 5000);
      await full(owner);
      screenshots[`before-${key}`] = await peer.screenshot();
      await select(owner, key);
      let successor;
      await waitUntil(`${label}: authoritative transition and silent cleanup`, async () => {
        const [a, b] = await Promise.all([metrics(owner), metrics(peer)]);
        successor = a;
        return a.playerLifeSeq > before.playerLifeSeq && (key === "s" ? a.localGameplayState.observerReady : a.localGameplayState.authorityPhase === "Live") &&
          ownMines(b, ownerId).length === 0 && !b.remoteDecoys.some(p => p.playerId === ownerId) &&
          a.localGameplayState.mines === 0 && a.localGameplayState.decoysActive === 0 &&
          ownMines(a, peerId).some(p => p.authorityProjectileId === foreign.authorityProjectileId);
      }, 8000);
      const cancellation = cuts(await readCutoverWireEvidence(peer), ownerId).find(c => c.life === successor.playerLifeSeq && c.session === oldMine.sessionId && BigInt(c.through) >= BigInt(oldMine.shotGroupId));
      assert.ok(cancellation, "committed transition must publish an exact source cutoff");
      assert.ok((await metrics(peer)).localGameplayState.mines >= 1, "peer's unrelated owned mine must survive");
      stages.push({ label, oldMine, oldDecoy, previousLife: before.playerLifeSeq, successorLife: successor.playerLifeSeq, cancellation, preservedForeignMine: foreign });
      screenshots[`after-${key}`] = await peer.screenshot();
      if (key === "s") {
        await select(owner, "1");
        await waitForPlayersReady(owner, peer, server);
      }
    }
    await mine(owner);
    let newMine;
    await waitUntil("a valid post-cutover launch reaches the peer", async () => { newMine = ownMines(await metrics(peer), ownerId)[0]; return newMine?.authorityProjectileId != null; }, 5000);
    assert.ok(BigInt(newMine.shotGroupId) > BigInt(stages.at(-1).cancellation.through));
    assert.ok(newMine.lifeSeq > stages.at(-1).oldMine.lifeSeq);
    assert.ok(ownMines(await metrics(owner), peerId).some(p => p.authorityProjectileId === foreign.authorityProjectileId));
    stages.push({ label: "new valid launch", newMine });
    await weaponReady(owner); await owner.press("3", 60);
    let leavingDecoy;
    await waitUntil("departure Decoy reaches the observer", async () => {
      leavingDecoy = (await metrics(peer)).remoteDecoys.find(p => p.playerId === ownerId);
      return leavingDecoy;
    }, 5000);
    const storedIdentity = await owner.evaluate("JSON.parse(localStorage.getItem('zubspace.identity.v1')).playerId");
    preDeparture = { state: await owner.artifactState(), qaBundle: await owner.freezeQaTrace(),
      provenance: collectArtifactProvenance({ e2eScript: "tests/e2e/weapon_cutover.mjs",
        ...(process.env.ZUBSPACE_E2E_STATIC_ROOT ? { wasm: path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) } : {}),
        clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) }) };
    assertQaServerBinaryProvenance(preDeparture.qaBundle, preDeparture.provenance);
    assertQaWasmProvenance(preDeparture.qaBundle, preDeparture.provenance);
    await select(owner, "q");
    await waitUntil("genuine Esc-Q returns to the Join form", async () => owner.evaluate(
      "document.querySelector('#player-name-form')?.classList.contains('visible')"), 15_000);
    let departureCut, departureLeft;
    await waitUntil("reliable departure cutoff precedes roster removal and clears exact flight", async () => {
      const state = await readCutoverWireEvidence(peer), m = await metrics(peer);
      departureCut = cuts(state, ownerId).find(c => c.reliableId != null && c.session === newMine.sessionId &&
        BigInt(c.through) >= BigInt(newMine.shotGroupId));
      departureLeft = mergedPacketLog(state).flatMap(entry => {
        const b = Buffer.from(entry.bytes || []);
        return entry.direction === "received" && b.length === 9 && b[0] === 0 && b[1] === 3 &&
          b[6] === 4 && b.readUInt16LE(7) === ownerId ? [{ reliableId: b.readUInt32LE(2) }] : [];
      }).find(left => departureCut && ((left.reliableId - departureCut.reliableId) >>> 0) > 0 &&
        ((left.reliableId - departureCut.reliableId) >>> 0) < 0x80000000);
      return departureCut && departureLeft && !m.remoteActorPresentation.some(p => p.id === ownerId) &&
        !m.remoteProjectileSources.some(p => p.sessionId === newMine.sessionId) &&
        !m.remoteDecoys.some(p => p.playerId === ownerId) && m.localGameplayState.mines >= 1;
    }, 10_000);
    stages.push({ label: "genuine leave cancels old source", mine: newMine, leavingDecoy,
      departureCut, departureLeft, preDepartureBundleId: preDeparture.qaBundle.bundleId });
    peerAfterDeparture = await peer.artifactState();
    const removalTick = peerAfterDeparture.uiMetrics.latestSnapshotServerTick;
    const elapsed = source => ((removalTick - source.serverTick) >>> 0) / 100;
    assert.ok(elapsed(leavingDecoy) < leavingDecoy.ttl - 1,
      "departure removal must precede natural Decoy expiry with a one-second margin");
    assert.ok(elapsed(newMine) < newMine.ttl - 1,
      "departure removal must precede natural mine expiry");
    screenshots["after-leave"] = await peer.screenshot();
    // Rejoin through the normal remembered-name form; no new RTC session or
    // gameplay state is synthesized by the test driver.
    await owner.evaluate("document.querySelector('#player-name-submit').focus(); true");
    await owner.press("Space", 60, false);
    await waitUntil("manual rejoin loads the ordinary client", async () => {
      const m = (await owner.metricsState()).uiMetrics;
      return m?.browserNetworkOwned && m.localGameplayState.zoneMapReady;
    }, 30_000);
    await owner.enterShip(); await waitForPlayersReady(owner, peer, server);
    assert.equal(await owner.evaluate("JSON.parse(localStorage.getItem('zubspace.identity.v1')).playerId"), storedIdentity);
    const rejoinedId = (await metrics(owner)).selfPlayerId;
    assert.ok(ownMines(await metrics(owner), peerId).some(p => p.authorityProjectileId === foreign.authorityProjectileId));
    await mine(owner);
    let rejoinedMine;
    await waitUntil("new session's valid launch survives the old departure cutoff", async () => {
      rejoinedMine = ownMines(await metrics(peer), rejoinedId).find(p => p.sessionId !== newMine.sessionId);
      return rejoinedMine?.authorityProjectileId != null;
    }, 5000);
    assert.notEqual(rejoinedMine.sessionId, newMine.sessionId);
    assert.ok(BigInt(rejoinedMine.shotGroupId) > BigInt(departureCut.through));
    assert.ok(!(await metrics(peer)).remoteProjectileSources.some(p => p.sessionId === newMine.sessionId));
    stages.push({ label: "manual rejoin accepts a fresh-session launch", rejoinedId, rejoinedMine });
    const result = await evidence.succeed({ ok: true, ownerId, peerId, fixture, stages,
      coverage: "real-key ship/spectator cutovers and Esc-Q departure clear exact mine/Decoy sources, preserve foreign flight, order reliable cleanup before player-left and accept a fresh-session launch after manual rejoin" }, capture());
    console.log(JSON.stringify({ ok: result.ok, stages: stages.map(s => s.label), artifact: process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR }));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); error.message += `\n${browserE2eFailureFooter(artifacts)}`; throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    await new Promise(resolve => staticServer.server.close(resolve));
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
