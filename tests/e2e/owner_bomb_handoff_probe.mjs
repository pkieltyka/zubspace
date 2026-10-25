#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, sleep,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, exactLiveGameplayScope,
  orientClientForProtocolDirection, browserE2eFailureFooter } from "./combat_replay.mjs";
import { loadRendezvousMap, planAuthoredApproach } from "./player_rendezvous.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { ownerBombHandoffEvidence, ownerBombConvergenceEvidence } from "./owner_bomb_handoff_evidence.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { serverObserverProfileForNetworkProfile } from "./server_observer_profile.mjs";

// Read-only runway selection. Fly there with ordinary controls; no setters,
// map edits, synthetic actors or forged weapon/position packets.
export function ownerBombRunway(geometry, origin) {
  const candidates = [];
  for (let dy = -2048; dy <= 2048; dy += 32) {
    for (let dx = -2048; dx <= 2048; dx += 32) {
      const x = Math.floor((origin.x + dx) / 16) * 16 + 8;
      const y = Math.floor((origin.y + dy) / 16) * 16 + 8;
      candidates.push({ x, y, distance: Math.hypot(x - origin.x, y - origin.y) });
    }
  }
  candidates.sort((a, b) => a.distance - b.distance);
  for (const point of candidates) {
    let clear = true;
    for (let dy = -1000; dy <= 200; dy += 8) {
      // Steering settles inside the upward wire bin, not exactly0 radians.
      // Leave lateral room for that legitimate continuous movement heading.
      if (!geometry.authoredHullIsTraversable(point.x, point.y + dy, 80)
        || geometry.tileAt(point.x, point.y + dy) === 171) { clear = false; break; }
    }
    if (!clear) continue;
    try {
      const plan = planAuthoredApproach(geometry, origin, point);
      return { ...point, upRunwayPx: 1000, downRunwayPx: 200, corridorRadiusPx: 80, plan };
    } catch { /* The next candidate must also have a real conservative route. */ }
  }
  throw new Error("no reachable upward bomb runway with full hull clearance");
}

async function main() {
  const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
  const profile = process.env.ZUBSPACE_E2E_NETWORK_PROFILE || "mild";
  const requireContinuity = process.env.ZUBSPACE_E2E_REQUIRE_OWNER_BOMB_CONTINUITY === "1";
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/owner_bomb_handoff_probe.mjs" });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { zone: arena, botCount: 0,
    observerNetworkProfile: serverObserverProfileForNetworkProfile(profile) });
  const browser = startChrome(await freePort(), "owner-bomb-handoff");
  let actor, report, runway, initialScope;
  const frames = new Map(), admissions = new Map(), shots = [];
  const url = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=${arena}&name=BombHandoff&password=secret&trace=1&packetlog=1&network_profile=${profile}&network_profile_seed=271828`;
  const observation = () => ({ frames: [...frames.values()], admissions: [...admissions.values()] });
  const capture = () => ({ server, serverPort: port, staticServer, urls: [url],
    clients: actor ? [{ label: "pilot", client: actor }] : [],
    extraLogs: { browser: browser.logs, observation: JSON.stringify(observation()),
      report: JSON.stringify({ report, runway, shots, initialScope }),
      navigation: JSON.stringify(actor?.rendezvousTrace || []) },
    disconnect: async () => { await actor?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}); } });
  const read = async () => (await actor.metricsState()).uiMetrics;
  const scope = m => JSON.stringify([m.localGameplayState.authorityScopeServerSessionId,
    m.localGameplayState.authorityScopeArenaStreamId, m.selfPlayerId, m.playerLifeSeq]);
  const collect = async () => {
    const data = await actor.evaluate(`({frames:(window.__zubspaceRemoteSpriteTrace||[]).slice(-180),
      admissions:window.__zubspaceWeaponAdmissionTrace||[]})`);
    for (const frame of data.frames) frames.set(frame.spriteFrameOrdinal, frame);
    for (const row of data.admissions) admissions.set(row.traceOrdinal, row);
    assert.ok(frames.size < 6000 && admissions.size < 1000, "bounded shot observation");
    report = ownerBombHandoffEvidence(observation());
  };
  try {
    await waitForServerListening(port, server.child, server.logs); await waitForChrome(browser);
    const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(status.botPlayers, 0); assert.equal(status.zone, arena);
    assert.equal(status.maxed_spawn_loadout, true);
    actor = await openChromeClient(browser, url); await actor.enterShip();
    await waitUntil("exact live bomb pilot", async () => exactLiveGameplayScope(await read()), 20000);
    const initial = await read(); initialScope = scope(initial);
    assert.equal(initial.playerShip, 0);
    const loaded = loadRendezvousMap(arena, [path.join(repoRoot, "assets/zones")]);
    runway = { ...ownerBombRunway(loaded.geometry, { x: initial.playerX, y: initial.playerY }), map: loaded.provenance };
    for (const requestedSpeed of [0, 100, 200, "maximum"]) {
      await navigateAuthoredTarget(actor, runway, server);
      assert.equal((await orientClientForProtocolDirection(actor, 0, -1)).oriented, true);
      const staged = await read();
      assert.equal(scope(staged), initialScope); assert.equal(staged.inSafeZone, false);
      assert.ok(Math.hypot(staged.playerVelocityX, staged.playerVelocityY) <= 3.5);
      const acceleration = staged.localGameplayState.baseThrustAcceleration;
      const topSpeed = staged.localGameplayState.baseTopSpeed;
      assert.ok(acceleration > 0 && topSpeed > 200);
      await waitUntil("authored bomb cooldown ready", async () => (await read()).bombCooldown <= 0, 10000);
      if (requestedSpeed !== 0) {
        const speed = requestedSpeed === "maximum" ? topSpeed : requestedSpeed;
        await actor.hold("ArrowUp", Math.ceil(1000 * speed / acceleration) + (requestedSpeed === "maximum" ? 120 : 0));
      }
      const before = await read();
      assert.equal(scope(before), initialScope); assert.equal(before.inSafeZone, false);
      const old = new Set([...admissions.values()].filter(row => row.phase === "owner-bomb-handoff").map(row => row.activation));
      await actor.press("Tab", 60); await sleep(150);
      let match;
      await waitUntil("exact predicted and canonical bomb draw pair", async () => {
        await collect();
        match = report.cases.find(row => !old.has(row.activation) && !shots.some(shot => shot.activation === row.activation));
        return match != null;
      }, 5000);
      assert.equal(match.freeFlightComparable, true, "do not attribute a bounce/freeze/Repel to handoff timing");
      const actualSpeed = Math.hypot(before.playerVelocityX, before.playerVelocityY);
      assert.ok(requestedSpeed === "maximum" ? actualSpeed >= topSpeed * 0.9
        : Math.abs(actualSpeed - requestedSpeed) <= (requestedSpeed === 0 ? 3.5 : 40),
      `real speed ${actualSpeed} did not exercise requested ${requestedSpeed}`);
      await sleep(350); await collect();
      const convergence = ownerBombConvergenceEvidence(observation(), match);
      if (requireContinuity) {
        assert.equal(match.backward, false, "exact owner bomb must not step backward at confirmation");
        assert.ok(match.animationAgeDelta >= -0.00001, "animation age must not restart at confirmation");
        assert.equal(convergence.ok, true, JSON.stringify(convergence));
      }
      shots.push({ requestedSpeed, actualSpeed, activation: match.activation, inputSequence: match.inputSequence,
        before, handoff: match, convergence });
      console.log(JSON.stringify({ stage: "owner_bomb_handoff", requestedSpeed, actualSpeed,
        backward: match.backward, alongVelocityPx: match.alongVelocityPx, animationAgeDelta: match.animationAgeDelta }));
    }
    assert.equal(shots.length, 4);
    await actor.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest?.(); true");
    await waitUntil("diagnostic profile timers drain", async () => {
      const state = await actor.evaluate("window.__zubspaceNetworkProfileStateForTest?.()");
      return state?.s2c?.pending === 0 && state?.c2s?.pending === 0;
    }, 10000);
    const network = await actor.evaluate("window.__zubspaceNetworkProfileStateForTest?.()");
    assert.ok(network.s2c.delay.samples > 0 && network.c2s.delay.samples > 0);
    const result = await evidence.succeed({ ok: true, diagnosticOnly: !requireContinuity, arena, network,
      report, shots, requireContinuity, scope: requireContinuity
        ? "four genuine owner-bomb confirmations and complete forward convergence at rest and varied speed under seeded delay; original delivery/security/cleanup; not general combat or production incident attribution"
        : "four genuine upward owner-bomb draw handoffs at rest and varied speed under seeded delay; capture success, not a continuity fix or production incident attribution" }, capture());
    console.log(JSON.stringify({ ok: result.ok, diagnosticOnly: !requireContinuity,
      backwardCases: shots.filter(shot => shot.handoff.backward).length,
      completeConvergences: shots.filter(shot => shot.convergence.ok).length }));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await actor?.close().catch(() => {});
    await Promise.all([stopProcess(browser.child), stopProcess(server.child)]);
    fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    await new Promise(resolve => staticServer.server.close(resolve));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href)
  main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
