#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening, sleep } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitForPlayersReady, rendezvousPlayers, waitForCombatViewWeaponReady,
  waitUntil, summarizeClient, browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseServerDamageEvents } from "../../tools/server_events.mjs";
import { readBulletFeedback } from "./bullet_feedback.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { captureMenuFlight, assertMenuFlightIsolation } from "./menu_flight_evidence.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/held_primary_fire.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0 });
  const browsers = [startChrome(await freePort(), "held-fire-actor"), startChrome(await freePort(), "held-fire-peer")];
  const urls = ["holdactor", "holdpeer"].map(name => `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&trace=1&packetlog=1&password=secret&name=${name}`);
  const clients = [], screenshots = {};
  const metrics = async c => (await c.metricsState()).uiMetrics;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "peer" : "actor", client })), extraScreenshots: screenshots,
    extraLogs: { "actor-browser": browsers[0].logs, "peer-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    clients.push(...await Promise.all(browsers.map((browser, i) => openChromeClient(browser, urls[i]))));
    await Promise.all(clients.map(c => c.enterShip()));
    const [actor, peer] = clients;
    await rendezvousPlayers(actor, peer, server);
    await waitForPlayersReady(actor, peer, server);
    await Promise.all(clients.map(c => waitForCombatViewWeaponReady(c, "held primary control")));
    const before = await metrics(actor), peerBefore = await metrics(peer);
    const actorId = before.selfPlayerId, peerId = peerBefore.selfPlayerId;
    assert.equal(before.localGameplayState.menuVisible, false);
    assert.equal(before.chatActive, false);
    const damageStart = parseServerDamageEvents(server.logs).length;
    let heldFlightEvidence = null;
    await actor.keyDown("Space");
    try {
      await waitUntil("held Space list opens", async () => (await metrics(actor)).scoreboardVisible, 3000);
      const heldBefore = await captureMenuFlight(actor);
      await actor.keyDown("Control"); await actor.keyDown("ArrowUp");
      await sleep(450);
      const held = await metrics(actor);
      assert.equal(held.localGameplayState.inputBits, 0);
      assert.equal(held.playerLifeSeq, before.playerLifeSeq);
      heldFlightEvidence = assertMenuFlightIsolation(heldBefore,
        await captureMenuFlight(actor, heldBefore.frame.renderOrdinal), "list");
      assert.deepEqual(held.semanticInputCounts, before.semanticInputCounts);
      assert.equal(parseServerDamageEvents(server.logs).length, damageStart);
      screenshots.held = await actor.screenshot();
      await actor.keyUp("ArrowUp");
      // Ctrl stays physically down across the layer boundary; this is normal
      // continuous fire, not another primary-fire press edge or key repeat.
      await actor.keyUp("Space");
      await sleep(650);
    } finally {
      for (const key of ["Control", "ArrowUp", "Space"]) await actor.keyUp(key).catch(() => {});
    }
    let hits;
    await waitUntil("continuous Ctrl deals repeated authoritative damage", async () => {
      hits = parseServerDamageEvents(server.logs).slice(damageStart)
        .filter(e => e.shooterId === actorId && e.victimId === peerId && e.damage > 0);
      return hits.length >= 2;
    }, 5000);
    assert.equal(new Set(hits.map(e => e.projectileId)).size, hits.length);
    assert.ok(hits.every(e => e.shooterLifeSeq === before.playerLifeSeq && e.victimLifeSeq === peerBefore.playerLifeSeq));
    const last = hits.at(-1);
    await waitUntil("both clients present the final held-fire damage", async () => {
      const [a, b] = await Promise.all(clients.map(readBulletFeedback));
      return a.metricsTrace.some(e => e.lastNetworkDamageServerSeq === last.damageSeq && e.lastNetworkDamagePlayerId === peerId)
        && b.metricsTrace.some(e => e.lastSelfDamageServerSeq === last.damageSeq && e.lastSelfDamageVictimLifeSeq === last.victimLifeSeq);
    }, 5000);
    const sentWeapons = async () => summarizeClient(await actor.artifactState()).sentPositions.filter(e => e.weapon !== 0).length;
    const releasedCount = await sentWeapons();
    assert.ok(releasedCount >= 2, "held Ctrl must emit multiple genuine weapon records");
    await sleep(650);
    assert.equal(await sentWeapons(), releasedCount, "releasing Ctrl must stop weapon emission");
    const released = await metrics(actor);
    assert.equal(released.scoreboardVisible, false);
    assert.equal(released.localGameplayState.inputBits, 0);
    screenshots.released = await peer.screenshot();
    console.log(JSON.stringify(await evidence.succeed({ ok: true, actorId, peerId,
      heldListBlocksFlightAndWeapons: true, releaseSpaceKeepsCtrlHeld: true,
      heldFlightEvidence,
      repeatedDamage: hits, weaponRecords: releasedCount, releaseStopsFire: true,
      scope: "genuine held-Space/Up/Ctrl isolation, continued physical Ctrl across Space release, repeated damage in both clients and Ctrl-release stop" }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
