#!/usr/bin/env node

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
  fireUntilDamage,
  openChromeClient,
  parseServerDamageEvents,
  startChrome,
  waitForChrome,
  waitForDeathPackets,
  waitForPlayersReady,
  rendezvousPlayers,
  waitForRespawnCycle,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { parseZubspaceEvents } from "../../tools/server_events.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { movingHullPathIsClear } from "./bot_combat_evidence.mjs";

const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";

function matchingEvents(server, sinceCount, shooterId, victimId) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) => event.shooterId === shooterId && event.victimId === victimId);
}

function assertBombPresentationGeometry(server, primary, selfSplash, label) {
  const witnesses = parseZubspaceEvents(server.logs)
    .map(({ event }) => event)
    .filter((event) =>
      event.type === "server_collision_witness" &&
      event.projectile_id === primary.projectileId &&
      event.owner_id === primary.shooterId &&
      event.owner_life_seq === primary.shooterLifeSeq
    );
  const primaryWitnesses = witnesses.filter((event) =>
    event.target_id === primary.victimId && event.target_life_seq === primary.victimLifeSeq
  );
  const selfWitnesses = witnesses.filter((event) =>
    event.target_id === selfSplash.victimId && event.target_life_seq === selfSplash.victimLifeSeq
  );
  const primaryWitness = primaryWitnesses[0];
  const selfWitness = selfWitnesses[0];
  const primaryGeometry = primaryWitness?.presentation_geometry;
  const selfGeometry = selfWitness?.presentation_geometry;
  if (
    primaryWitnesses.length !== 1 ||
    primaryWitness?.history_source !== "observer_presentation" ||
    typeof primaryGeometry?.status !== "string" ||
    !primaryGeometry.status.startsWith("resolved_") ||
    primaryWitness.requested_target_tick !== primaryGeometry.render_tick ||
    primaryWitness.selected_target_tick !== primaryGeometry.render_tick ||
    primaryWitness.collision_anchor_x !== primaryGeometry.target_x ||
    primaryWitness.collision_anchor_y !== primaryGeometry.target_y ||
    !Number.isFinite(primaryGeometry.authority_error) ||
    primaryGeometry.authority_error > 12 ||
    selfWitnesses.length !== 1 ||
    selfWitness?.contact_reason !== "splash" ||
    selfWitness?.history_source !== "local_self" ||
    selfGeometry?.status !== "local_self" ||
    selfWitness.requested_target_tick !== selfGeometry.render_tick ||
    selfWitness.selected_target_tick !== selfGeometry.render_tick ||
    selfWitness.damage_anchor_x !== selfGeometry.target_x ||
    selfWitness.damage_anchor_y !== selfGeometry.target_y
  ) {
    throw new Error(`${label} does not own one verified primary/splash geometry: ${JSON.stringify({
      primary,
      selfSplash,
      primaryWitnesses,
      selfWitnesses,
    })}`);
  }
}

function weaponType(weapon) {
  return weapon & 0x001f;
}

function selfDamageTraceMatches(entry, event) {
  return (
    entry &&
    entry.selfPlayerId === event.victimId &&
    entry.selfDamageServerSeq === event.damageSeq &&
    entry.selfDamageServerTick === event.serverTick &&
    entry.selfDamageVictimLifeSeq === event.victimLifeSeq &&
    entry.selfDamageShooterLifeSeq === event.shooterLifeSeq &&
    entry.selfDamageAttackerId === event.shooterId &&
    (entry.selfDamageWeapon & 0x001f) === (event.weapon & 0x001f) &&
    entry.selfDamage === event.damage &&
    (entry.selfDamageEnergyBefore == null ||
      Math.abs(entry.selfDamageEnergyBefore - event.energyBefore) <= 1) &&
    Math.abs(entry.selfDamageEnergyAfter - event.energyAfter) <= 1
  );
}

async function assertVictimSelfDamageTrace(victim, event, label, sinceIndex) {
  await waitUntil(`${label} victim self-damage trace`, async () => {
    const state = await victim.combatState();
    return (state.damageTrace || [])
      .slice(sinceIndex)
      .some((entry) => damageTraceMatches(entry, event) || selfDamageTraceMatches(entry, event));
  }, 8_000);
}

function shooterRemoteBombEvidence(state, event) {
  // The exact damage receipt is historical. The rendered energy can already
  // include recharge from a newer snapshot, and same-tick self-splash can
  // replace lastNetworkDamage*. Require both the exact receipt and a fresh,
  // same-life energy drop instead of conflating their sampling instants.
  const receipt = (state.damageTrace || []).some((entry) =>
    entry.forSelf === false && entry.selfPlayerId === event.shooterId &&
    damageTraceMatches(entry, event)
  );
  if (!receipt) return false;
  return (state.damagePresentationTrace || []).some((witness) => {
    const damage = witness?.damage;
    if (!damage || !damageTraceMatches({ ...damage, damage: damage.amount }, event)) return false;
    const snapshot = witness.sameLifeEnergySnapshot;
    const modulus = 0x8000_0000;
    return snapshot?.playerId === event.victimId &&
      snapshot.lifeSeq === event.victimLifeSeq &&
      Number.isInteger(snapshot.serverTick) &&
      Number.isInteger(event.serverTick) &&
      (snapshot.serverTick - event.serverTick + modulus) % modulus < modulus / 2 &&
      Number.isFinite(snapshot.energy) && Number.isFinite(snapshot.serverEnergy) &&
      snapshot.energy >= event.energyAfter - 1 &&
      snapshot.energy < event.energyBefore &&
      snapshot.serverEnergy >= event.energyAfter - 1 &&
      snapshot.energy <= snapshot.serverEnergy + 1;
  });
}

async function assertShooterRemoteDamageEvidence(shooter, event, label) {
  await waitUntil(`${label} shooter received and displayed victim bomb damage`, async () =>
    shooterRemoteBombEvidence(await shooter.metricsState(), event), 8_000);
}

async function waitForVictimReady(victim, label) {
  await waitUntil(`${label} victim alive and full energy`, async () => {
    const state = await victim.combatState();
    const metrics = state.uiMetrics || {};
    return (
      (metrics.playerRespawnTimer ?? 0) <= 0.05 &&
      Number.isFinite(metrics.playerEnergy) &&
      Number.isFinite(metrics.playerMaxEnergy) &&
      metrics.playerEnergy >= metrics.playerMaxEnergy - 1
    );
  }, 18_000);
}

async function waitForBombReady(shooter, label) {
  await waitUntil(`${label} shooter bomb cooldown`, async () => {
    const state = await shooter.combatState();
    const metrics = state.uiMetrics || {};
    return (
      (metrics.playerRespawnTimer ?? 0) <= 0.05 &&
      (metrics.playerEnergy ?? 0) > 0 &&
      (metrics.bombCooldown ?? 0) <= 0.05
    );
  }, 12_000);
}

function signedOrientationDelta(from, to) {
  return ((to - from + 0.5) % 1 + 1) % 1 - 0.5;
}

async function rotationDirection(client) {
  const before = await client.combatState();
  await client.hold("ArrowLeft", 140);
  await sleep(100);
  const after = await client.combatState();
  const delta = signedOrientationDelta(
    before.uiMetrics?.playerOrientation ?? 0,
    after.uiMetrics?.playerOrientation ?? 0,
  );
  return delta >= 0 ? { increase: "ArrowLeft", decrease: "ArrowRight" } : { increase: "ArrowRight", decrease: "ArrowLeft" };
}

async function rotateToOrientation(client, target, label, tolerance = 0.025) {
  const initial = await client.combatState();
  if (Math.abs(signedOrientationDelta(initial.uiMetrics?.playerOrientation ?? 0, target)) <= tolerance) {
    return initial.uiMetrics;
  }
  const direction = await rotationDirection(client);
  const nearVisits = new Map();
  let turnThroughWrap = false;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const state = await client.combatState();
    const orientation = state.uiMetrics?.playerOrientation ?? 0;
    const delta = signedOrientationDelta(orientation, target);
    if (Math.abs(delta) <= tolerance) {
      return state.uiMetrics;
    }
    if (Math.abs(delta) < 0.04) {
      const cell = Math.round(orientation * 100_000);
      const visits = (nearVisits.get(cell) ?? 0) + 1;
      nearVisits.set(cell, visits);
      if (visits >= 3) turnThroughWrap = true;
    }
    const turnDistance = turnThroughWrap ? ((target - orientation + 1) % 1) : Math.abs(delta);
    const key = turnThroughWrap || delta > 0 ? direction.increase : direction.decrease;
    const holdMs = turnDistance > 0.12 ? 180 : turnDistance > 0.04 ? 70 : 20;
    // The captured30ms holds span two logical steps and skip the whole tight
    // interval. Align a genuine pulse to an input step, preserving tolerance
    // and the original50 attempts; escape a repeated lattice through a turn.
    await client.pulseOnFreshInputStep(key, holdMs);
    await sleep(70);
  }
  throw new Error(`${label} could not rotate shooter toward target: ${JSON.stringify((await client.combatState()).uiMetrics)}`);
}

async function brakeVerticalVelocity(client, label) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const state = await client.combatState();
    const metrics = state.uiMetrics || {};
    const velocityY = metrics.playerVelocityY ?? 0;
    if (Math.abs(velocityY) <= 2.5) {
      return metrics;
    }
    const key = velocityY > 0 ? "ArrowDown" : "ArrowUp";
    const holdMs = Math.max(45, Math.min(180, Math.abs(velocityY) * 3.0));
    await client.hold(key, holdMs);
    await sleep(80);
  }
  throw new Error(`${label} could not brake shooter vertical velocity: ${JSON.stringify((await client.combatState()).uiMetrics)}`);
}

async function verticalKeyMap(client, label) {
  const before = await client.combatState();
  const beforeMetrics = before.uiMetrics || {};
  await client.hold("ArrowUp", 180);
  await sleep(180);
  const after = await client.combatState();
  const afterMetrics = after.uiMetrics || {};
  const positionDelta = (afterMetrics.playerY ?? 0) - (beforeMetrics.playerY ?? 0);
  const velocityDelta =
    (afterMetrics.playerVelocityY ?? 0) - (beforeMetrics.playerVelocityY ?? 0);
  const sample = Math.abs(positionDelta) > 0.5 ? positionDelta : velocityDelta;
  if (!Number.isFinite(sample) || Math.abs(sample) < 0.1) {
    throw new Error(`${label} could not calibrate vertical thrust keys: ${JSON.stringify({
      before: beforeMetrics,
      after: afterMetrics,
      positionDelta,
      velocityDelta,
    })}`);
  }
  return sample >= 0
    ? { increaseY: "ArrowUp", decreaseY: "ArrowDown" }
    : { increaseY: "ArrowDown", decreaseY: "ArrowUp" };
}

async function placeVictimInBombPath(shooter, victim, label) {
  const startedAt = Date.now(), deadline = startedAt + 20_000;
  const loaded = loadRendezvousMap(arena, [
    ...(process.env.ZUBSPACE_SERVER_ZONE_ROOT ? [process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
    path.join(repoRoot, "assets/zones"),
  ]);
  let expected = null, best = null, placement = null;
  const sample = async () => {
    const [shooterState, victimState] = await Promise.all([shooter.combatState(), victim.combatState()]);
    const shooterMetrics = shooterState.uiMetrics || {}, victimMetrics = victimState.uiMetrics || {};
    const scope = [shooterMetrics.selfPlayerId, shooterMetrics.playerLifeSeq,
      victimMetrics.selfPlayerId, victimMetrics.playerLifeSeq];
    if (!scope.every(Number.isInteger) || shooterMetrics.playerAlive !== true ||
        victimMetrics.playerAlive !== true || shooterMetrics.serverPlayerAlive !== true ||
        victimMetrics.serverPlayerAlive !== true || (expected && JSON.stringify(scope) !== expected)) {
      throw new Error(`${label} bomb placement crossed a player lifecycle boundary`);
    }
    expected ??= JSON.stringify(scope);
    const dx = shooterMetrics.playerX - victimMetrics.playerX;
    const dy = victimMetrics.playerY - shooterMetrics.playerY;
    const watcherDx = shooterMetrics.playerX - shooterMetrics.remotePositionX;
    const watcherDy = shooterMetrics.remotePositionY - shooterMetrics.playerY;
    best = { shooter: shooterMetrics, victim: victimMetrics, dx, dy, watcherDx, watcherDy,
      elapsedMs: Date.now() - startedAt, map: loaded.provenance };
    placement = { shooter: shooterMetrics, victim: victimMetrics, targetOrientation: 0.5 };
    // Preserve every original local and observer-view placement bound.
    return dy > 0 && watcherDy > 0 && Math.abs(dx) <= 10 && dy >= 92 && dy <= 96 &&
      Math.abs(victimMetrics.playerVelocityY) <= 2.5 && Math.abs(watcherDx) <= 15 &&
      watcherDy >= 92 && watcherDy <= 104;
  };
  while (Date.now() < deadline) {
    if (await sample()) return placement;
    const target = { x: best.shooter.playerX, y: best.shooter.playerY + 94 };
    const source = { x: best.victim.playerX, y: best.victim.playerY };
    // The post-respawn rendezvous explicitly certifies a128px empty stage.
    // Check the actual current20px swept hull too; the former48px meeting
    // certificate did not guarantee this94px southern firing corridor.
    if (!movingHullPathIsClear({ x: source.x, y: source.y,
      velocityX: target.x - source.x, velocityY: target.y - source.y, seconds: 1,
      hullIsOpen: (x, y) => loaded.geometry.authoredHullIsTraversable(x, y, 20) })) {
      throw new Error(`${label} bomb placement lacks a full-hull clear corridor: ${JSON.stringify(best)}`);
    }
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;
    // Rendezvous may leave any heading and residual lateral velocity. Use
    // genuine two-axis navigation instead of treating Up/Down as world Y.
    const route = await driveClientToWorldTarget(victim, target, remaining, true, 1, sample, 42, 0.25);
    if (await sample()) return placement;
    if (!route.reached) break;
    // A released leg can reach the local point before the observer catches
    // up. Re-sample both views within the same bounded placement window.
    await sleep(45);
  }
  throw new Error(`${label} could not place victim in bomb path: ${JSON.stringify({ best })}`);
}

async function prepareBombAttempt(shooter, victim, label) {
  await waitForBombReady(shooter, label);
  await rotateToOrientation(shooter, 0.5, `${label} pre-place`, 0.006);
  await brakeVerticalVelocity(shooter, `${label} pre-place`);
  await rotateToOrientation(shooter, 0.5, `${label} pre-place settled`, 0.006);
  const placement = await placeVictimInBombPath(shooter, victim, label);
  const [shooterState, victimState] = await Promise.all([
    shooter.combatState(),
    victim.combatState(),
  ]);
  const shooterMetrics = shooterState.uiMetrics || {};
  const victimMetrics = victimState.uiMetrics || {};
  const preFireDx = (shooterMetrics.remotePositionX ?? placement.victim.playerX) -
    (shooterMetrics.playerX ?? placement.shooter.playerX);
  const preFireDy = (shooterMetrics.remotePositionY ?? placement.victim.playerY) -
    (shooterMetrics.playerY ?? placement.shooter.playerY);
  const localDx = (victimMetrics.playerX ?? placement.victim.playerX) -
    (shooterMetrics.playerX ?? placement.shooter.playerX);
  const localDy = (victimMetrics.playerY ?? placement.victim.playerY) -
    (shooterMetrics.playerY ?? placement.shooter.playerY);
  if (
    Math.abs(preFireDx) > 12 ||
    preFireDy < 91 ||
    preFireDy > 106 ||
    Math.abs(localDx) > 12 ||
    localDy < 90 ||
    localDy > 108
  ) {
    throw new Error(`${label} target drifted out of deterministic fire range: ${JSON.stringify({
      placement,
      shooter: shooterMetrics,
      victim: victimMetrics,
      preFireDx,
      preFireDy,
      localDx,
      localDy,
    })}`);
  }
  return {
    placement,
    preFire: {
      shooter: shooterMetrics,
      victim: victimMetrics,
      preFireDx,
      preFireDy,
      localDx,
      localDy,
    },
  };
}

async function waitForOptionalServerBombHit(server, sinceCount, shooterId, victimId, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (
      matchingEvents(server, sinceCount, shooterId, victimId).some((event) =>
        [3, 4, 8].includes(weaponType(event.weapon)),
      )
    ) {
      return true;
    }
    await sleep(100);
  }
  return false;
}

async function fireOneBombAndAssert({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
  maxAttempts = 4,
}) {
  const attempts = [];
  let shooterTraceStart = 0;
  let victimTraceStart = 0;
  let victimRemoteBombsBefore = 0;
  let victimBeforeMetrics = null;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    const setup = await prepareBombAttempt(shooter, victim, `${label} attempt ${attempt}`);
    const [shooterBefore, victimBefore] = await Promise.all([
      shooter.combatState(),
      victim.combatState(),
    ]);
    shooterTraceStart = shooterBefore.damageTrace?.length ?? 0;
    victimTraceStart = victimBefore.damageTrace?.length ?? 0;
    victimRemoteBombsBefore = victimBefore.uiMetrics?.remoteBombSpawns ?? 0;
    victimBeforeMetrics = victimBefore.uiMetrics;

    await waitUntil(`${label} attempt ${attempt} combat view weapon ready`, async () =>
      (await shooter.combatState()).uiMetrics?.combatViewTimeline?.weaponReady === true,
    10_000);
    await shooter.fireBomb();
    const hit = await waitForOptionalServerBombHit(
      server,
      sinceCount,
      shooterId,
      victimId,
      4_000,
    );
    const [shooterAfter, victimAfter] = await Promise.all([
      shooter.combatState(),
      victim.combatState(),
    ]);
    attempts.push({
      attempt,
      hit,
      setup: {
        preFire: {
          preFireDx: setup.preFire.preFireDx,
          preFireDy: setup.preFire.preFireDy,
          localDx: setup.preFire.localDx,
          localDy: setup.preFire.localDy,
          shooterVelocityX: setup.preFire.shooter.playerVelocityX,
          shooterVelocityY: setup.preFire.shooter.playerVelocityY,
          victimVelocityX: setup.preFire.victim.playerVelocityX,
          victimVelocityY: setup.preFire.victim.playerVelocityY,
          shooterOrientation: setup.preFire.shooter.playerOrientation,
        },
      },
      shooter: compactMetrics(shooterAfter.uiMetrics),
      victim: compactMetrics(victimAfter.uiMetrics),
      recentEvents: parseServerDamageEvents(server.logs).slice(-8),
    });
    if (hit) break;
  }

  if (
    !matchingEvents(server, sinceCount, shooterId, victimId).some((event) =>
      [3, 4, 8].includes(weaponType(event.weapon)),
    )
  ) {
    throw new Error(`${label} did not produce server bomb damage after ${maxAttempts} real-browser attempts: ${JSON.stringify({
      attempts,
      recentEvents: parseServerDamageEvents(server.logs).slice(-20),
    })}`);
  }
  await sleep(700);

  const bombEvents = matchingEvents(server, sinceCount, shooterId, victimId).filter((event) =>
    [3, 4, 8].includes(weaponType(event.weapon)),
  );
  if (bombEvents.length !== 1) {
    throw new Error(`${label} expected exactly one server bomb damage event: ${JSON.stringify({
      bombEvents,
      recentEvents: parseServerDamageEvents(server.logs).slice(-20),
    })}`);
  }
  const [event] = bombEvents;
  assertServerDamageAccounting(event, label);
  if (event.damage <= 0 || event.energyAfter >= event.energyBefore) {
    throw new Error(`${label} bomb did not reduce energy: ${JSON.stringify(event)}`);
  }

  await Promise.all([
    assertSingleBrowserDamageTrace(shooter, event, `${label} shooter direct bomb trace`, shooterTraceStart),
    assertSingleBrowserDamageTrace(victim, event, `${label} victim direct bomb trace`, victimTraceStart),
    assertShooterRemoteDamageEvidence(shooter, event, `${label} shooter`),
    assertVictimSelfDamageTrace(victim, event, `${label} victim`, victimTraceStart),
  ]);

  const selfSplash = parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .find((candidate) =>
      candidate.serverTick === event.serverTick &&
      candidate.shooterId === shooterId &&
      candidate.victimId === shooterId &&
      [3, 4, 8].includes(weaponType(candidate.weapon))
    );
  if (!selfSplash) {
    throw new Error(`${label} expected same-tick shooter self-splash event: ${JSON.stringify({
      event,
      recentEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  assertBombPresentationGeometry(server, event, selfSplash, label);
  await assertSingleBrowserDamageTrace(
    shooter,
    selfSplash,
    `${label} shooter direct self-splash trace`,
    shooterTraceStart,
  );

  const victimAfter = await victim.combatState();
  if ((victimAfter.uiMetrics?.remoteBombSpawns ?? 0) <= victimRemoteBombsBefore) {
    throw new Error(`${label} victim did not render a remote bomb spawn: ${JSON.stringify({
      before: victimBeforeMetrics,
      after: victimAfter.uiMetrics,
      event,
    })}`);
  }
  return {
    event,
    attempts,
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }

  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/bomb_damage.mjs" });
  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaPort, "bomb-alpha");
  const betaChrome = startChrome(betaPort, "bomb-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs },
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. Each client
    // is guarded so one already-dead session cannot make cleanup throw.
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    alphaUrl = `${pageBase}&name=bombalpha`;
    betaUrl = `${pageBase}&name=bombbeta`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta);

    const [alphaStart, betaStart] = await Promise.all([
      alpha.combatState(),
      beta.combatState(),
    ]);
    const alphaId = alphaStart.uiMetrics?.selfPlayerId;
    const betaId = betaStart.uiMetrics?.selfPlayerId;
    if (alphaId == null || betaId == null || alphaId === betaId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ alphaId, betaId })}`);
    }

    const previousRespawnSeq = betaStart.uiMetrics?.playerRespawnSeq ?? 0;
    const kill = await fireUntilDamage({
      label: "bomb setup bullet kill",
      shooter: alpha,
      victim: beta,
      server,
      shooterId: alphaId,
      victimId: betaId,
      sinceCount: parseServerDamageEvents(server.logs).length,
      requireLethal: true,
    });
    await waitForDeathPackets({
      shooter: alpha,
      victim: beta,
      shooterId: alphaId,
      victimId: betaId,
      expectedWins: 1,
      expectedLosses: 1,
      previousRespawnSeq,
    });
    await waitForRespawnCycle(beta, previousRespawnSeq);
    await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: 128 });
    await waitForVictimReady(beta, "post-respawn bomb");

    const bomb = await fireOneBombAndAssert({
      label: "post-respawn bomb",
      shooter: alpha,
      victim: beta,
      server,
      shooterId: alphaId,
      victimId: betaId,
      sinceCount: kill.nextSinceCount,
    });

    const [alphaAfter, betaAfter] = await Promise.all([
      alpha.metricsState(),
      beta.metricsState(),
    ]);
    const result = await evidence.succeed({
      ok: true,
      transport: "rtc",
      browserPair: "chrome-chrome",
      arena,
      alphaId,
      betaId,
      kill: kill.event,
      bomb: bomb.event,
      bombAttempts: bomb.attempts,
      alpha: compactMetrics(alphaAfter.uiMetrics),
      beta: compactMetrics(betaAfter.uiMetrics),
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    throw new Error(`${error.message}\n${browserE2eFailureFooter(artifacts)}`, { cause: error });
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
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

export { main, shooterRemoteBombEvidence };
