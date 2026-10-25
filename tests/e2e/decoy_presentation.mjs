#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitForPlayersReady, waitForLivePlayerScopes, waitUntil, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { sameDecoyEntity } from "../../tools/realtime_event_evidence.mjs";

const angleDistance = (a, b) => Math.abs(((a-b+1.5)%1)-0.5);
const mirrored = (initial, owner) => ((2*initial-owner)%1+1)%1;
const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");

// Unmodified Chaos, no bots or weapon grants. Genuine deployment, turning
// and thrust establish the reference behavior before any Repel mutation.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/decoy_presentation.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0 });
  const browsers = [startChrome(await freePort(), "decoy-owner"), startChrome(await freePort(), "decoy-observer")];
  const base = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=decoypilot`, `${base}&name=decoywitness`];
  const clients = [], screenshots = {}, observations = [];
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "observer" : "owner", client })),
    extraScreenshots: screenshots,
    extraLogs: { observations: JSON.stringify(observations, null, 2), "owner-browser": browsers[0].logs, "observer-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const metrics = async client => (await client.metricsState()).uiMetrics;
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) {
      clients.push(await openChromeClient(browsers[i], urls[i]));
      await clients[i].enterShip();
    }
    const [owner, observer] = clients;
    await waitForLivePlayerScopes(owner, observer);
    // Preserve the true stationary deployment: only the witness flies.
    // A genuine turn marks the waiting owner active without adding velocity.
    await owner.press("ArrowRight", 70);
    const initial = await metrics(owner), ownerId = initial.selfPlayerId;
    const observerApproach = await navigateAuthoredTarget(observer,
      { x: initial.playerX, y: initial.playerY }, server, { arrivalRadiusPx: 48 });
    observations.push({ label: "stationary-owner-approach", ...observerApproach });
    await waitForPlayersReady(owner, observer, server);
    const life = initial.playerLifeSeq;
    assert.ok(initial.localGameplayState.decoys >= 2, "shipped starting prizes must supply two ordinary deployments");
    const observedDecoys = m => (m.remoteDecoys || []).filter(d => d.playerId === ownerId);
    // Each viewer has its own arena stream. Within that admitted stream, the
    // recovered local copy must retain the same exact source and birth.
    const recoveredOwnCopy = (m, source) => observedDecoys(m).find(d => sameDecoyEntity(d,
      { ...source, arenaStreamId: m.localGameplayState.authorityScopeArenaStreamId }));
    const remoteOwner = m => {
      const actor = (m.remotePlayers || []).find(p => p.id === ownerId);
      if (actor) assert.ok(Number.isFinite(actor.orientation) && Number.isFinite(actor.displayOrientation),
        "source and displayed headings must be observed separately");
      return actor;
    };
    async function deploy(label) {
      await waitUntil(`${label}: live weapon readiness`, async () => {
        const m = await metrics(owner), l = m.localGameplayState;
        return l.authorityPhase === "Live" && l.gameplayStepReady && l.bombCooldown <= 0;
      }, 10_000);
      const before = await metrics(owner);
      const observerBefore = await metrics(observer);
      const ownerShip = observerBefore.remoteActorPresentation.find(p => p.id === ownerId)?.ship;
      assert.ok(Number.isInteger(ownerShip), "observer has the owner's authoritative ship");
      const known = observedDecoys(observerBefore);
      await owner.press("3", 70);
      let decoy;
      await waitUntil(`${label}: authoritative remote decoy and ordinary presentation`, async () => {
        const [a, b] = await Promise.all(clients.map(metrics));
        assert.equal(a.playerLifeSeq, life);
        assert.equal(a.localGameplayState.decoyNameLabelPresentationCount, 0, "our own decoys have no duplicate name labels");
        decoy = observedDecoys(b).find(d => !known.some(old => sameDecoyEntity(d, old)));
        return decoy && a.semanticInputCounts.decoy === before.semanticInputCounts.decoy+1
          && a.localGameplayState.decoys === before.localGameplayState.decoys-1
          && a.localGameplayState.decoyOrdinaryShipPresentationCount >= 1
          && a.localGameplayState.decoyRadarPresentationCount >= 1
          && b.localGameplayState.decoyOrdinaryShipPresentationCount >= 1
          && b.localGameplayState.decoyNameLabelPresentationCount >= 1
          && b.localGameplayState.decoyRadarPresentationCount >= 1;
      }, 10_000);
      assert.equal(decoy.ownerName, "decoypilot");
      assert.equal(decoy.ship, ownerShip);
      observations.push({ label, before, decoy });
      screenshots[`${label}-owner`] = await owner.screenshot();
      screenshots[`${label}-observer`] = await observer.screenshot();
      return decoy;
    }
    const stationary = await deploy("stationary");
    assert.ok(Math.hypot(stationary.fireVelocityX, stationary.fireVelocityY) < 0.01);
    assert.ok(Math.hypot(stationary.x-stationary.fireX, stationary.y-stationary.fireY) < 0.01,
      "stationary decoy starts on the ship, without an invented push-off");
    assert.ok(Math.hypot(stationary.fireX-initial.playerX, stationary.fireY-initial.playerY) <= 2);
    await owner.press("ArrowRight", 450);
    let ownMirrored;
    await waitUntil("stationary decoy mirrors the owner's turn", async () => {
      const [a, m] = await Promise.all(clients.map(metrics));
      const d = observedDecoys(m).find(d => sameDecoyEntity(d, stationary)), p = remoteOwner(m);
      if (!d || !p || angleDistance(p.displayOrientation, stationary.initialOrientation) < 0.04) return false;
      assert.ok(Math.hypot(d.x-stationary.fireX, d.y-stationary.fireY) < 0.01);
      const own = recoveredOwnCopy(a, stationary);
      if (!own || angleDistance(own.orientation, mirrored(own.initialOrientation, a.playerOrientation)) >= 0.005) return false;
      assert.equal(a.localGameplayState.decoyNameLabelPresentationCount, 0);
      ownMirrored = { decoy: own, ownerOrientation: a.playerOrientation };
      return angleDistance(d.orientation, mirrored(d.initialOrientation, p.orientation)) < 0.005;
    }, 5000);
    observations.push({ label: "recovered-own-stationary-mirrored", ...ownMirrored });
    screenshots["stationary-turned"] = await observer.screenshot();
    await owner.press("ArrowUp", 700);
    const moving = await deploy("moving");
    assert.ok(Math.hypot(moving.fireVelocityX, moving.fireVelocityY) > 20, "genuine thrust establishes a moving launch");
    assert.ok(Math.hypot(moving.velocityX-moving.fireVelocityX, moving.velocityY-moving.fireVelocityY) < 0.01,
      "decoy inherits ship velocity without additional launch speed");
    await owner.press("ArrowRight", 450);
    await owner.press("ArrowUp", 700);
    let separated;
    await waitUntil("fresh steering separates the owner from the inherited decoy path", async () => {
      const m = await metrics(observer), d = observedDecoys(m).find(d => sameDecoyEntity(d, moving)), p = remoteOwner(m);
      if (!d || !p) return false;
      const separation = Math.hypot(d.x-p.positionX, d.y-p.positionY);
      if (separation <= 15 || angleDistance(d.orientation, mirrored(d.initialOrientation, p.orientation)) >= 0.005) return false;
      separated = { decoy: d, owner: p, separation };
      return true;
    }, 5000);
    observations.push({ label: "separated-and-mirrored", ...separated });
    screenshots["moving-separated"] = await observer.screenshot();
    for (const client of clients) {
      const state = await client.pageState();
      assert.equal(state.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!state.networkLog.some(e => e.level === "error"));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, arena: "chaos-zone",
      stationaryOverlap: true, inheritedVelocity: true, mirroredTurns: true,
      localNameHidden: true, remoteNameAndRadarVisible: true, ordinaryShipSprites: true,
      separatedDistance: separated.separation,
    }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.stack += `\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]);
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error.message); process.exit(1); });
