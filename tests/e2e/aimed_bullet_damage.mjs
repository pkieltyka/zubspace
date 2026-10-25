#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";

import {
  assertServerDamageAccounting,
  assertSingleBrowserDamageTrace,
  browserE2eFailureFooter,
  compactMetrics,
  damageTraceMatches,
  driveClientToWorldTarget,
  openChromeClient,
  parseServerDamageEvents,
  parseServerProjectileEvents,
  startChrome,
  waitForChrome,
  waitForPlayersReady,
  rendezvousPlayers,
  waitUntil,
  waitForCombatViewWeaponReady,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { startRtcUdpImpairment } from "../../tools/rtc_udp_impairment.mjs";
import { mixedUdpProfiles } from "./udp_path_profiles.mjs";
import assert from "node:assert/strict";
import { heldProjectileEvidence } from "./held_projectile_evidence.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { movingHullPathIsClear } from "./bot_combat_evidence.mjs";
import { waitForSelectedRtcRoute } from "./rtc_route_evidence.mjs";

const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const bulletPressMs = Number(process.env.ZUBSPACE_E2E_AIMED_BULLET_PRESS_MS || "30");
const heldContact = process.env.ZUBSPACE_E2E_AIMED_BULLET_HELD_CONTACT === "1";
const targetSeparation = Number(process.env.ZUBSPACE_E2E_AIMED_BULLET_SEPARATION || (heldContact ? "48" : "140"));
const udpProfile = process.env.ZUBSPACE_E2E_AIMED_BULLET_UDP_PROFILE || "";
// Validate before opening any services. Ordinary aimed-bullet coverage remains
// unchanged; the additional lane owns its loopback-only impaired associations.
const udpProfiles = udpProfile ? mixedUdpProfiles(udpProfile) : null;

function weaponType(weapon) {
  return weapon & 0x001f;
}

function matchingEvents(server, sinceCount, shooterId, victimId) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) => event.shooterId === shooterId && event.victimId === victimId);
}

function bulletSelfDamageEvents(server, sinceCount, playerId) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) =>
      event.shooterId === playerId &&
      event.victimId === playerId &&
      [1, 2, 7, 15].includes(weaponType(event.weapon))
    );
}

function remotePlayerMetric(metrics, playerId) {
  return (metrics?.remotePlayers || []).find((player) => player.id === playerId) || null;
}

function remotePlayerEnergy(metrics, playerId) {
  return remotePlayerMetric(metrics, playerId)?.energy ?? metrics?.remoteEnergy ?? 0;
}

function signedOrientationDelta(from, to) {
  return ((to - from + 0.5) % 1 + 1) % 1 - 0.5;
}

async function rotationDirection(client) {
  const before = await client.metricsState();
  await client.hold("ArrowLeft", 140);
  await sleep(100);
  const after = await client.metricsState();
  const delta = signedOrientationDelta(
    before.uiMetrics?.playerOrientation ?? 0,
    after.uiMetrics?.playerOrientation ?? 0,
  );
  return delta >= 0
    ? { increase: "ArrowLeft", decrease: "ArrowRight" }
    : { increase: "ArrowRight", decrease: "ArrowLeft" };
}

async function rotateToOrientation(client, target, label, tolerance = 0.009) {
  const aimed = (orientation) => Math.abs(signedOrientationDelta(orientation, target)) <= tolerance &&
    Math.floor((((orientation % 1) + 1) % 1) * 40) === Math.round(target * 40) % 40;
  const initial = await client.metricsState();
  if (aimed(initial.uiMetrics?.playerOrientation ?? 0)) {
    return initial.uiMetrics;
  }
  const direction = await rotationDirection(client);
  const nearVisits = new Map();
  let turnThroughWrap = false;
  for (let attempt = 0; attempt < 60; attempt += 1) {
    const state = await client.metricsState();
    const orientation = state.uiMetrics?.playerOrientation ?? 0;
    const delta = signedOrientationDelta(orientation, target);
    if (aimed(orientation)) {
      return state.uiMetrics;
    }
    if (Math.abs(delta) < 0.04) {
      const cell = Math.round(orientation * 100_000);
      const visits = (nearVisits.get(cell) ?? 0) + 1;
      nearVisits.set(cell, visits);
      if (visits >= 3) turnThroughWrap = true;
    }
    // At canonical60Hz the captured Terrier alternates between0.49833319
    // (wrong wire cell) and0.50791651 (outside the unchanged0.007 tolerance).
    // Finer wall-clock taps cannot subdivide a canonical input step. Continue
    // through a full turn when that orbit repeats; wrapping changes which
    // canonical samples fall in the accepted interval. Keep the same sixty
    // attempts and require the original angle AND wire cell before returning.
    const turnDistance = turnThroughWrap ? ((target - orientation + 1) % 1) : Math.abs(delta);
    const key = turnThroughWrap || delta > 0 ? direction.increase : direction.decrease;
    // Release after a real input frame. Eight milliseconds after a fresh
    // frame ends before the next 60 Hz step and delivers no turn. Trace
    // capture stays after release so it cannot lengthen the fine pulse.
    // Readiness also requires the intended wire bin: just below 0.5 is a
    // different projectile direction even inside the angular tolerance.
    const holdMs = turnDistance > 0.12 ? 170 : turnDistance > 0.04 ? 70 : 20;
    await client.pulseOnFreshInputStep(key, holdMs);
    await sleep(65);
  }
  throw new Error(`${label} could not rotate toward ${target}: ${JSON.stringify((await client.metricsState()).uiMetrics)}`);
}

async function waitForVictimEnergyFull(shooter, victim, label) {
  await waitUntil(`${label} victim full on local/server/remote`, async () => {
    const [shooterState, victimState] = await Promise.all([shooter.metricsState(), victim.metricsState()]);
    const victimMetrics = victimState.uiMetrics || {};
    const shooterMetrics = shooterState.uiMetrics || {};
    const max = victimMetrics.playerMaxEnergy ?? 1700;
    const victimId = victimMetrics.selfPlayerId;
    return (
      victimMetrics.playerAlive === true &&
      victimMetrics.serverPlayerAlive === true &&
      (victimMetrics.playerRespawnTimer ?? 0) <= 0.05 &&
      (victimMetrics.playerEnergy ?? 0) >= max - 1 &&
      (victimMetrics.visibleEnergyDigits ?? 0) >= max - 1 &&
      (victimMetrics.serverPlayerEnergy ?? 0) >= max - 1 &&
      remotePlayerEnergy(shooterMetrics, victimId) >= max - 1
    );
  }, 18_000);
}

async function assertShooterDidNotSelfDamage(shooter, beforeState, label) {
  const afterState = await shooter.metricsState();
  const beforeMetrics = beforeState.uiMetrics || {};
  const afterMetrics = afterState.uiMetrics || {};
  const selfId = afterMetrics.selfPlayerId ?? beforeMetrics.selfPlayerId;
  const newBulletSelfDamage = (afterState.damageTrace || [])
    .slice(beforeState.damageTrace?.length ?? 0)
    .filter((entry) =>
      entry.forSelf === true &&
      entry.victimId === selfId &&
      entry.attackerId === selfId &&
      [1, 2, 7, 15].includes(weaponType(entry.weapon))
    );

  if (
    afterMetrics.playerAlive !== true ||
    afterMetrics.serverPlayerAlive !== true ||
    (afterMetrics.playerRespawnTimer ?? 0) > 0 ||
    (afterMetrics.playerRespawnSeq ?? 0) > (beforeMetrics.playerRespawnSeq ?? 0) ||
    (afterMetrics.selfLosses ?? 0) > (beforeMetrics.selfLosses ?? 0) ||
    newBulletSelfDamage.length > 0
  ) {
    throw new Error(`${label} shooter recorded bullet self-damage: ${JSON.stringify({
      before: beforeMetrics,
      after: afterMetrics,
      newBulletSelfDamage,
      damageTrace: (afterState.damageTrace || []).slice(beforeState.damageTrace?.length ?? 0),
    })}`);
  }
}

async function assertDamageVisible(client, event, label, sinceIndex) {
  // Damage identity/result is persistent in the direct WATCH_DAMAGE trace and
  // `last*Damage*` metrics. The displayed energy itself remains live and may
  // already have recharged by the time this poll runs.
  await waitUntil(`${label} visible authoritative damage`, async () => {
    const state = await client.metricsState();
    const matchingTrace = (state.damageTrace || [])
      .slice(sinceIndex)
      .some((entry) => damageTraceMatches(entry, event));
    const metrics = state.uiMetrics || {};
    const isVictim = metrics.selfPlayerId === event.victimId;
    const victimEnergyMatches =
      isVictim &&
      metrics.lastSelfDamageServerSeq === event.damageSeq &&
      metrics.lastSelfDamageServerTick === event.serverTick &&
      Math.abs((metrics.lastSelfDamageEnergy ?? Number.NaN) - event.energyAfter) <= 1 &&
      (metrics.serverPlayerEnergy ?? 0) >= event.energyAfter - 2 &&
      (metrics.playerEnergy ?? Number.NaN) >= event.energyAfter - 2;
    const remoteEnergyMatches =
      !isVictim &&
      (() => {
        const remote = remotePlayerMetric(metrics, event.victimId);
        return (
          remote &&
          (remote.energy ?? Number.NaN) >= event.energyAfter - 2 &&
          (remote.serverEnergy ?? Number.NaN) >= event.energyAfter - 2 &&
          metrics.lastNetworkDamageServerSeq === event.damageSeq &&
          metrics.lastNetworkDamageServerTick === event.serverTick
        );
      })();
    return matchingTrace && (victimEnergyMatches || remoteEnergyMatches);
  }, 6_000);
}

async function placeVictimOnGunLine({ shooter, victim, shooterOrientation, targetSign, label }) {
  await rotateToOrientation(shooter, shooterOrientation, `${label} shooter aim`, 0.007);
  const roots = [process.env.ZUBSPACE_SERVER_ZONE_ROOT, path.join(repoRoot, "assets/zones")].filter(Boolean);
  const { geometry } = loadRendezvousMap(arena, roots);
  const scope = metrics => JSON.stringify([metrics.selfPlayerId, metrics.playerLifeSeq,
    metrics.localGameplayState?.authorityScopeServerSessionId,
    metrics.localGameplayState?.authorityScopeArenaStreamId]);
  const initial = await Promise.all([shooter.metricsState(), victim.metricsState()]);
  const scopes = initial.map(state => scope(state.uiMetrics));
  const startedAt = Date.now();
  const deadline = startedAt + 20_000;
  const routes = [];
  // Fix both stations for this bounded preparation. A previous victim can
  // become the next shooter while still coasting sideways; chasing that
  // moving source with only the other pilot never establishes a stopped line.
  const stations = [
    { x: initial[0].uiMetrics.playerX, y: initial[0].uiMetrics.playerY },
    { x: initial[0].uiMetrics.playerX,
      y: initial[0].uiMetrics.playerY + targetSeparation * targetSign },
  ];
  let latest = [];
  let best = null;
  let attempt = 0;
  const sample = async () => {
    const [shooterState, victimState] = await Promise.all([
      shooter.metricsState(),
      victim.metricsState(),
    ]);
    const shooterMetrics = shooterState.uiMetrics || {};
    const victimMetrics = victimState.uiMetrics || {};
    if (scope(shooterMetrics) !== scopes[0] || scope(victimMetrics) !== scopes[1]) {
      throw new Error(`${label} crossed a player lifecycle boundary`);
    }
    latest = [shooterMetrics, victimMetrics];
    const shooterX = shooterMetrics.playerX;
    const shooterY = shooterMetrics.playerY;
    const victimX = victimMetrics.playerX;
    const victimY = victimMetrics.playerY;
    const dx = victimX - shooterX;
    const dy = victimY - shooterY;
    const watcherDx = (shooterMetrics.remotePositionX ?? victimX) - shooterX;
    const watcherDy = (shooterMetrics.remotePositionY ?? victimY) - shooterY;
    const targetY = shooterY + targetSeparation * targetSign;
    const error = targetY - victimY;
    const velocityY = victimMetrics.playerVelocityY ?? 0;
    const victimSpeed = Math.hypot(victimMetrics.playerVelocityX, victimMetrics.playerVelocityY);
    const shooterSpeed = Math.hypot(shooterMetrics.playerVelocityX, shooterMetrics.playerVelocityY);
    const shooterFacing = Math.abs(signedOrientationDelta(shooterMetrics.playerOrientation ?? 0, shooterOrientation));
    const localGoodSide = Math.sign(dy) === targetSign;
    const watcherGoodSide = Math.sign(watcherDy) === targetSign;
    const localSeparation = Math.abs(dy);
    const watcherSeparation = Math.abs(watcherDy);

    best = {
      attempt,
      elapsedMs: Date.now() - startedAt,
      dx,
      dy,
      watcherDx,
      watcherDy,
      error,
      velocityY,
      victimSpeed,
      shooterSpeed,
      shooterFacing,
      shooter: compactMetrics(shooterMetrics),
      victim: compactMetrics(victimMetrics),
    };

    if (
      localGoodSide &&
      watcherGoodSide &&
      Math.abs(dx) <= 11 &&
      Math.abs(watcherDx) <= 17 &&
      localSeparation >= targetSeparation - 12 &&
      localSeparation <= targetSeparation + 12 &&
      watcherSeparation >= targetSeparation - 18 &&
      watcherSeparation <= targetSeparation + 22 &&
      victimSpeed <= 2.7 &&
      shooterSpeed <= 2.7 &&
      shooterFacing <= 0.011
    ) {
      return true;
    }
    return false;
  };
  for (; attempt < 170 && Date.now() < deadline; attempt += 1) {
    if (await sample() && Date.now() < deadline) return { ...best, routes };
    let jointArrival = false, failure = null;
    const settled = (metrics, index) =>
      Math.hypot(metrics.playerX - stations[index].x, metrics.playerY - stations[index].y) <= 1 &&
      Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) <= 2.7;
    const observeArrival = async () => {
      if (failure) throw failure;
      if (jointArrival) return true;
      await sample();
      // Another observer may have committed arrival during the metrics read.
      if (!jointArrival) jointArrival = latest.every(settled);
      return jointArrival;
    };
    const flights = await Promise.allSettled([shooter, victim].map(async (client, index) => {
      try {
        while (!jointArrival && Date.now() < deadline) {
          if (await observeArrival()) break;
          const origin = latest[index], target = stations[index];
          if (settled(origin, index)) { await sleep(45); continue; }
          if (!movingHullPathIsClear({ x: origin.playerX, y: origin.playerY,
            velocityX: target.x - origin.playerX, velocityY: target.y - origin.playerY,
            seconds: 1, hullIsOpen: (x, y) => geometry.authoredHullIsTraversable(x, y, 20) })) {
            throw new Error(`${label} lacks a full-hull clear gun-line preparation corridor`);
          }
          const remaining = deadline - Date.now();
          if (remaining <= 0) break;
          const route = await driveClientToWorldTarget(client, target, remaining, true, 1, observeArrival, 42, 0.25, 5_000, 2.7);
          routes.push({ actor: index === 0 ? "shooter" : "victim", ...route });
          if (!route.reached) throw new Error(`${label} station-keeping route did not settle: ${JSON.stringify(route)}`);
          await sleep(45);
        }
      } catch (error) { failure ||= error; throw error; }
    }));
    // Drain both pilots before reporting a failure; no key-driving task may
    // outlive its setup and interfere with a later diagnostic or firing phase.
    if (flights.some(flight => flight.status === "rejected")) throw failure;
    if (!jointArrival || Date.now() >= deadline) break;
    await rotateToOrientation(shooter, shooterOrientation, `${label} shooter aim settle`, 0.007);
    if (await sample() && Date.now() < deadline) return { ...best, routes };
    await sleep(45);
  }

  throw new Error(`${label} could not place separated target on gun line: ${JSON.stringify({
    targetSeparation,
    targetSign,
    shooterOrientation,
    best,
    routes,
  })}`);
}

async function fireOneAimedBulletAndAssert({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
  shooterOrientation,
  targetSign,
}) {
  await waitForVictimEnergyFull(shooter, victim, `${label} pre-fire`);
  const placement = await placeVictimOnGunLine({
    shooter,
    victim,
    shooterOrientation,
    targetSign,
    label,
  });
  await waitForVictimEnergyFull(shooter, victim, `${label} pre-fire settled`);

  const [shooterBefore, victimBefore] = await Promise.all([shooter.metricsState(), victim.metricsState()]);
  const shooterTraceStart = shooterBefore.damageTrace?.length ?? 0;
  const victimTraceStart = victimBefore.damageTrace?.length ?? 0;
  const localDx = (victimBefore.uiMetrics?.playerX ?? 0) - (shooterBefore.uiMetrics?.playerX ?? 0);
  const localDy = (victimBefore.uiMetrics?.playerY ?? 0) - (shooterBefore.uiMetrics?.playerY ?? 0);
  const nonOverlapping = Math.hypot(localDx, localDy) >= targetSeparation - 16;
  if (!nonOverlapping) {
    throw new Error(`${label} target was not actually separated before firing: ${JSON.stringify({
      localDx,
      localDy,
      placement,
      shooter: shooterBefore.uiMetrics,
      victim: victimBefore.uiMetrics,
    })}`);
  }

  await shooter.focus();
  await waitForCombatViewWeaponReady(shooter, label);
  await shooter.press("Control", bulletPressMs, false);
  await waitUntil(`${label} one server bullet hit`, async () => {
    return matchingEvents(server, sinceCount, shooterId, victimId).length >= 1;
  }, 10_000);

  const events = matchingEvents(server, sinceCount, shooterId, victimId);
  if (events.length !== 1) {
    throw new Error(`${label} expected exactly one server damage event for one aimed bullet tap: ${JSON.stringify({
      bulletPressMs,
      events,
      allDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  const [event] = events;
  const projectileEvents = parseServerProjectileEvents(server.logs);
  const hitSpawn = projectileEvents.find((candidate) =>
    candidate.type === "server_projectile_spawned" &&
    candidate.projectileId === event.projectileId
  );
  const linkedChildren = hitSpawn == null
    ? []
    : projectileEvents.filter((candidate) =>
        candidate.type === "server_projectile_spawned" &&
        candidate.shotGroupId === hitSpawn.shotGroupId
      );
  if (hitSpawn == null || linkedChildren.length !== 2) {
    throw new Error(`${label} did not preserve the double-barrel shot group: ${JSON.stringify({
      event,
      hitSpawn,
      linkedChildren,
      recentProjectileEvents: projectileEvents.slice(-20),
    })}`);
  }
  const selfDamageEvents = bulletSelfDamageEvents(server, sinceCount, shooterId);
  if (selfDamageEvents.length > 0) {
    throw new Error(`${label} produced bullet self-damage while firing from separated geometry: ${JSON.stringify({
      shooterId,
      selfDamageEvents,
      allDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
      placement,
    })}`);
  }
  assertServerDamageAccounting(event, label);
  if (event.damage !== 400 || event.energyBefore !== 1700 || event.energyAfter !== 1300) {
    throw new Error(`${label} unexpected aimed bullet damage values: ${JSON.stringify({
      event,
      placement,
    })}`);
  }

  await Promise.all([
    assertSingleBrowserDamageTrace(shooter, event, `${label} shooter`, shooterTraceStart),
    assertSingleBrowserDamageTrace(victim, event, `${label} victim`, victimTraceStart),
    assertDamageVisible(shooter, event, `${label} shooter`, shooterTraceStart),
    assertDamageVisible(victim, event, `${label} victim`, victimTraceStart),
  ]);
  await assertShooterDidNotSelfDamage(shooter, shooterBefore, label);

  let heldFlight = null;
  if (heldContact) {
    // Trace rings already record every real frame/received packet. Read them
    // once after a complete terminal tail rather than polling during flight.
    await sleep(700);
    heldFlight = await Promise.all([shooter, victim].map(async client => {
      const state = await client.evaluate(`({
        packetLog: [...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]
          .filter(p => p.bytes?.[0] === 125 && p.bytes?.[1] === 2 && p.bytes?.[2] === 4),
        frames: (window.__zubspaceRemoteSpriteTrace || []).map(f => ({ at: f.at,
          projectiles: { overflow: f.projectiles.overflow, entries: f.projectiles.entries.filter(p =>
            p.sourcePlayerId === ${shooterId} && p.sourceLifeSeq === ${event.shooterLifeSeq} &&
            String(p.shotGroupId) === ${JSON.stringify(String(hitSpawn.shotGroupId))}) } }))
      })`);
      return heldProjectileEvidence({ ...state, hit: event, group: hitSpawn.shotGroupId,
        linkedIds: linkedChildren.map(child => child.projectileId) });
    }));
  }

  return {
    event,
    shotGroupId: hitSpawn.shotGroupId,
    linkedProjectileIds: linkedChildren.map((child) => child.projectileId),
    placement,
    ...(heldFlight ? { heldFlight } : {}),
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function main() {
  if (!Number.isFinite(bulletPressMs) || bulletPressMs <= 0) {
    throw new Error(`invalid ZUBSPACE_E2E_AIMED_BULLET_PRESS_MS=${process.env.ZUBSPACE_E2E_AIMED_BULLET_PRESS_MS || ""}`);
  }
  if (!Number.isFinite(targetSeparation) || (heldContact ? targetSeparation !== 48 : targetSeparation < 80)) {
    throw new Error(`invalid ZUBSPACE_E2E_AIMED_BULLET_SEPARATION=${process.env.ZUBSPACE_E2E_AIMED_BULLET_SEPARATION || ""}`);
  }
  if (heldContact) {
    assert.equal(arena, "chaos-zone", "held-contact fixture uses the authored Chaos geometry and weapons");
    assert.equal(udpProfiles, null, "held-contact and WAN fixtures are separate contracts");
  }
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }

  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/aimed_bullet_damage.mjs" });
  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort, udpProfiles || heldContact ? { botCount: 0,
    env: { ...(udpProfiles ? { ZUBSPACE_SERVER_RTC_UDP_BIND_IP: "127.0.0.1" } : {}),
      // The unchanged linked-child oracle consumes gameplay DEBUG events.
      // Keep those explicit without requiring verbose SCTP/ICE packet chatter.
      ZUBSPACE_SERVER_LOG_FILTER: "zubspace_server=debug,rtc=warn,webrtc=warn,rtc_sctp=warn,rtc_ice=warn,rtc_dtls=warn" } } : {});
  const alphaChrome = startChrome(alphaPort, "aim-alpha");
  const betaChrome = startChrome(betaPort, "aim-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  let bridge = null;
  const networkPath = udpProfiles ? { profile: udpProfile, startupReadyWaitMs: null,
    live: null, routes: null, views: null, cleanup: null } : null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs,
      ...(bridge ? { "udp-path": JSON.stringify(bridge.snapshot(), null, 2),
        "network-path": JSON.stringify(networkPath, null, 2) } : {}) },
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. Each client
    // is guarded so one already-dead session cannot make cleanup throw.
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
      if (bridge) {
        // Preserve the real Leave path through the impaired association before
        // closing owned sockets. Closing the proxy first would hide cleanup.
        await waitUntil("impaired aimed-bullet sessions retire", async () => {
          const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
          return status.clients === 0 && status.rtc_sessions === 0;
        }, 10000);
        await bridge.close();
        networkPath.cleanup = bridge.snapshot();
        for (const session of networkPath.cleanup.sessions) {
          assert.equal(session.transport.closed, true);
          assert.equal(session.transport.pendingBytes, 0);
          for (const lane of [session.transport.c2s, session.transport.s2c]) {
            assert.equal(lane.queuedPackets, 0); assert.equal(lane.queuedBytes, 0);
            assert.equal(lane.timerActive, false);
          }
        }
      }
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    let rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    if (udpProfiles) {
      const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
      assert.equal(status.observer_network_profile, "auto", "WAN combat uses the ordinary compiled policy");
      bridge = await startRtcUdpImpairment({ upstream: rtcUrl,
        origin: `http://127.0.0.1:${staticServer.port}`, profiles: udpProfiles });
      rtcUrl = bridge.url;
    }
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1${udpProfiles ? "&packetlog_limit=12000" : ""}`;
    alphaUrl = `${pageBase}&name=aimalpha`;
    betaUrl = `${pageBase}&name=aimbeta`;
    if (bridge) {
      // Profiles are reserved in offer-arrival order. Pin alpha to the fast
      // path before beta offers; concurrent offers would make that ambiguous.
      alpha = await openChromeClient(alphaChrome, alphaUrl);
      beta = await openChromeClient(betaChrome, betaUrl);
    } else {
      [alpha, beta] = await Promise.all([
        openChromeClient(alphaChrome, alphaUrl),
        openChromeClient(betaChrome, betaUrl),
      ]);
    }
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    if (bridge) {
      const started = Date.now();
      // Readiness and visibility assertions below are unchanged. Allow the
      // measured 2 MB transfer time before their ordinary 30-second window;
      // retain actual wait rather than asserting an unmeasured startup SLA.
      await waitUntil("both WAN pilots finish canonical file/baseline readiness", async () => {
        const states = await Promise.all([alpha.metricsState(), beta.metricsState()]);
        return states.every(state => state.uiMetrics?.localGameplayState?.gameplayStepReady === true);
      }, 75000);
      networkPath.startupReadyWaitMs = Date.now() - started;
    }
    // The firing line extends beyond mere collocation. Certify enough real
    // map clearance for its full separation, hulls and arrival tolerance.
    await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: targetSeparation + 48 });
    await waitForPlayersReady(alpha, beta);
    // Ship 5 is the arena's natural double-barrel profile. Selecting it via
    // normal number-key UI makes every aimed tap exercise linked children.
    await Promise.all([alpha.press("Escape", 60), beta.press("Escape", 60)]);
    await Promise.all([alpha.press("5", 60), beta.press("5", 60)]);
    await sleep(700);
    await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: targetSeparation + 48 });
    await waitForPlayersReady(alpha, beta);

    const [alphaStart, betaStart] = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    const alphaId = alphaStart.uiMetrics?.selfPlayerId;
    const betaId = betaStart.uiMetrics?.selfPlayerId;
    if (alphaId == null || betaId == null || alphaId === betaId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ alphaId, betaId })}`);
    }

    const shots = [];
    let sinceCount = parseServerDamageEvents(server.logs).length;
    const alphaToBeta = await fireOneAimedBulletAndAssert({
      label: "aimed separated alpha to beta",
      shooter: alpha,
      victim: beta,
      server,
      shooterId: alphaId,
      victimId: betaId,
      sinceCount,
      shooterOrientation: 0.5,
      targetSign: 1,
    });
    shots.push(alphaToBeta);
    sinceCount = alphaToBeta.nextSinceCount;

    const alphaToBetaRegen = await fireOneAimedBulletAndAssert({
      label: "aimed separated alpha to beta after regen",
      shooter: alpha,
      victim: beta,
      server,
      shooterId: alphaId,
      victimId: betaId,
      sinceCount,
      shooterOrientation: 0.5,
      targetSign: 1,
    });
    shots.push(alphaToBetaRegen);
    sinceCount = alphaToBetaRegen.nextSinceCount;

    const betaToAlpha = await fireOneAimedBulletAndAssert({
      label: "aimed separated beta to alpha counterfire",
      shooter: beta,
      victim: alpha,
      server,
      shooterId: betaId,
      victimId: alphaId,
      sinceCount,
      shooterOrientation: 0.0,
      targetSign: -1,
    });
    shots.push(betaToAlpha);

    const [alphaAfter, betaAfter] = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    if (bridge) {
      networkPath.live = bridge.snapshot();
      assert.equal(networkPath.live.errors.length, 0);
      assert.equal(networkPath.live.sessions.length, 2);
      // The stats hook is asynchronous: use its resolved result, never the
      // empty object produced by serializing a Promise over CDP.
      const settledRoutes = await Promise.all([alpha, beta].map((client, index) =>
        waitForSelectedRtcRoute(() => client.evaluateAwait("window.__zubspaceRtcStatsForTest()"),
          networkPath.live.sessions[index].frontPort)));
      networkPath.routes = settledRoutes.map(route => route.stats);
      networkPath.routeObservations = settledRoutes.map(route => route.observations);
      networkPath.views = [alphaAfter, betaAfter].map(state => state.uiMetrics?.combatViewTimeline);
      assert.equal(networkPath.views[0]?.targetDelayTicks, 7);
      assert.ok(networkPath.views[1]?.targetDelayTicks >= 15 && networkPath.views[1]?.targetDelayTicks <= 20);
      for (let i = 0; i < 2; i++) {
        const session = networkPath.live.sessions[i], stats = networkPath.routes[i];
        networkPath.selectedRoutes ||= [];
        networkPath.selectedRoutes.push(settledRoutes[i].selected);
        assert.equal(session.transport.unexpectedSources, 0);
        for (const lane of [session.transport.c2s, session.transport.s2c]) {
          assert.ok(lane.applicationForwarded > 0);
          assert.equal(lane.overflowDrops, 0); assert.equal(lane.sendErrors, 0);
        }
      }
      const slow = networkPath.live.sessions[1].transport;
      assert.ok(slow.c2s.applicationDrops + slow.s2c.applicationDrops > 0);
      assert.ok(slow.c2s.maximumQueueAgeMs >= 55 && slow.s2c.maximumQueueAgeMs >= 100);
    }
    const result = await evidence.succeed({
      ok: true,
      transport: "rtc",
      browserPair: "chrome-chrome",
      arena,
      bulletPressMs,
      targetSeparation,
      alphaId,
      betaId,
      events: shots.map((shot) => shot.event),
      shotGroups: shots.map((shot) => ({
        shotGroupId: shot.shotGroupId,
        projectileIds: shot.linkedProjectileIds,
      })),
      ...(heldContact ? { heldFlight: shots.map(shot => shot.heldFlight) } : {}),
      placement: shots.map((shot) => shot.placement),
      alpha: compactMetrics(alphaAfter.uiMetrics),
      beta: compactMetrics(betaAfter.uiMetrics),
      ...(networkPath ? { networkPath } : {}),
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    throw new Error(`${error.message}\n${browserE2eFailureFooter(artifacts)}`, { cause: error });
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await bridge?.close();
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(alphaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exit(1);
  });
}

export { main };
