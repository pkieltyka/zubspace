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
  brakeClientVelocity,
  compactMetrics,
  damageTraceMatches,
  openChromeClient,
  orientClientForProtocolDirection,
  parseServerDamageEvents,
  startChrome,
  summarizeClient,
  waitForChrome,
  waitForPlayersReady,
  rendezvousPlayers,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { readBulletFeedback } from "./bullet_feedback.mjs";
import { authoredMapGeometry, runwayAlongHeading } from "./authored_map_geometry.mjs";

function ownerRecoveryRunway(geometry, metrics) {
  const { playerX: x, playerY: y, playerOrientation: orientation } = metrics;
  if (![x, y, orientation].every(Number.isFinite)) throw new Error("owner recovery staging requires finite ship geometry");
  const angle = orientation * Math.PI * 2;
  for (const [key, sign] of [["ArrowUp", 1], ["ArrowDown", -1]]) {
    const dx = Math.sin(angle) * sign, dy = -Math.cos(angle) * sign;
    const runway = runwayAlongHeading({ hullIsTraversable: geometry.authoredHullIsOpen,
      safetyDistance: () => Infinity, x, y, dx, dy, travelPx: 512, stepPx: 4 });
    if (runway?.runwayPx >= 512) return { key, x, y, ...runway };
  }
  throw new Error(`owner recovery staging has no clear forward/backward runway at ${JSON.stringify({ x, y, orientation })}`);
}

const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const clientEntry = path.relative(repoRoot, path.join(applicationRoot, "index.html"));
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const shotCount = Number(process.env.ZUBSPACE_E2E_SINGLE_BULLET_SHOTS || "3");
const damageSampleWindowMs = Number(process.env.ZUBSPACE_E2E_DAMAGE_WINDOW_MS || "3000");
const bulletPressMs = Number(process.env.ZUBSPACE_E2E_SINGLE_BULLET_PRESS_MS || "30");
// Diagnostic only: retain actual render/input traces rather than treating a
// requested CPU multiplier as proof of any particular rendered frame rate.
const cpuThrottleRate = Number(process.env.ZUBSPACE_E2E_SINGLE_BULLET_CPU_RATE || "1");
// The regression needs multiple physical browser auto-repeat keydowns while
// remaining one intentional weapon opportunity. Keep the whole chord inside
// the authored initial-Warbird bullet cooldown instead of holding long enough
// to exercise correct continuous fire (24 ticks in Chaos, 6 in Extreme).
const initialWarbirdBulletCooldownMs = (arena === "extreme" ? 6 : 24) * 10;
const repeatedKeyDownEveryMs = Math.max(
  10,
  Math.floor(initialWarbirdBulletCooldownMs / 4),
);
const repeatedBulletPressMs = Number(
  process.env.ZUBSPACE_E2E_SINGLE_BULLET_REPEAT_PRESS_MS ||
    String(Math.floor(initialWarbirdBulletCooldownMs * 0.75)),
);

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
      [1, 2, 7, 15].includes(event.weapon & 0x001f)
    );
}

async function waitForCombatViewReadyAfterCapture(client, label) {
  await waitUntil(`${label} combat view ready after retained-state capture`, async () => {
    const state = await client.metricsState();
    const metrics = state.uiMetrics || {};
    return metrics.playerAlive === true &&
      metrics.serverPlayerAlive === true &&
      metrics.combatViewTimeline?.weaponReady === true;
  }, 10_000);
}

async function aimSingleBulletAtVictim(shooter, victim, label) {
  // The real-key rendezvous admits a small residual velocity. During repeated
  // full-recharge waits the two hulls can coast out of the ORIGINAL firing line.
  // Aim each intentional tap with genuine turn keys; never retry a missed shot
  // or change the one-press/one-hit, damage, energy or trace requirements.
  const [source, target] = (await Promise.all([
    shooter.metricsState(), victim.metricsState(),
  ])).map(state => state.uiMetrics);
  const dx = target.playerX - source.playerX;
  const dy = target.playerY - source.playerY;
  if (![dx, dy].every(Number.isFinite)) throw new Error(`${label}: missing live aim geometry`);
  const aim = await orientClientForProtocolDirection(shooter, dx || dy ? dx : 0, dx || dy ? dy : -1);
  if (!aim.oriented) throw new Error(`${label}: genuine single-tap aim failed: ${JSON.stringify(aim)}`);
  (shooter.singleBulletAimTrace ||= []).push({ label, at: Date.now(),
    source: { x: source.playerX, y: source.playerY, life: source.playerLifeSeq },
    target: { x: target.playerX, y: target.playerY, life: target.playerLifeSeq },
    desiredDirection: aim.desiredDirection, direction: aim.direction });
}

function remotePlayerMetric(metrics, playerId) {
  return (metrics?.remotePlayers || []).find((player) => player.id === playerId) || null;
}

function remotePlayerEnergy(metrics, playerId) {
  return remotePlayerMetric(metrics, playerId)?.energy ?? metrics?.remoteEnergy ?? 0;
}

function assertVictimFullBeforeShot({ label, shooterState, victimState, victimId }) {
  const victimMetrics = victimState.uiMetrics || {};
  const shooterMetrics = shooterState.uiMetrics || {};
  const victimMax = victimMetrics.playerMaxEnergy ?? 1700;
  const victimEnergy = victimMetrics.playerEnergy ?? 0;
  const victimDigits = victimMetrics.visibleEnergyDigits ?? 0;
  const victimServerEnergy = victimMetrics.serverPlayerEnergy ?? 0;
  const shooterRemote = remotePlayerMetric(shooterMetrics, victimId);
  const shooterRemoteEnergy = shooterRemote?.energy ?? shooterMetrics.remoteEnergy ?? 0;
  const shooterRemoteServerEnergy = shooterRemote?.serverEnergy ?? 0;
  const problems = [];
  if (victimMetrics.playerAlive !== true || victimMetrics.serverPlayerAlive !== true) {
    problems.push("victim was not alive locally and on the server");
  }
  if (victimEnergy < victimMax - 1) {
    problems.push("victim visible HUD energy was not full");
  }
  if (victimDigits < victimMax - 1) {
    problems.push("victim visible HUD digits were not full");
  }
  if (victimServerEnergy < victimMax - 1) {
    problems.push("victim server-authoritative self energy was not full");
  }
  if (shooterRemoteEnergy < victimMax - 1) {
    problems.push("shooter remote label for victim was not full");
  }
  if (shooterRemoteServerEnergy < victimMax - 1) {
    problems.push("shooter remote server-fed energy for victim was not full");
  }
  if (problems.length > 0) {
    throw new Error(`${label} fired before victim was full in all player-visible/server views: ${JSON.stringify({
      problems,
      victimId,
      victimMax,
      victimEnergy,
      victimDigits,
      victimServerEnergy,
      shooterRemoteEnergy,
      shooterRemoteServerEnergy,
      victimMetrics,
      shooterRemote,
      shooterMetrics,
    })}`);
  }
}

function sampleEnergy(entry, energyField, remotePlayerId = null) {
  if (remotePlayerId != null) {
    return remotePlayerMetric(entry, remotePlayerId)?.energy;
  }
  return entry[energyField];
}

function sampleServerEnergy(entry, remotePlayerId = null) {
  if (remotePlayerId != null) {
    return remotePlayerMetric(entry, remotePlayerId)?.serverEnergy;
  }
  return entry.serverPlayerEnergy;
}

async function assertShooterDidNotSelfDie(client, beforeState, label) {
  const afterState = await readBulletFeedback(client);
  const beforeMetrics = beforeState.uiMetrics || {};
  const afterMetrics = afterState.uiMetrics || {};
  const beforeRespawnSeq = beforeMetrics.playerRespawnSeq ?? 0;
  const beforeLosses = beforeMetrics.selfLosses ?? 0;
  const selfId = afterMetrics.selfPlayerId ?? beforeMetrics.selfPlayerId;
  const newBulletSelfDamage = (afterState.damageTrace || [])
    .slice(beforeState.damageTrace?.length ?? 0)
    .filter((entry) =>
      entry.forSelf === true &&
      entry.victimId === selfId &&
      entry.attackerId === selfId &&
      [1, 2, 7, 15].includes(entry.weapon & 0x001f)
    );

  if (
    afterMetrics.playerAlive !== true ||
    afterMetrics.serverPlayerAlive !== true ||
    (afterMetrics.playerRespawnTimer ?? 0) > 0 ||
    (afterMetrics.playerRespawnSeq ?? 0) > beforeRespawnSeq ||
    (afterMetrics.selfLosses ?? 0) > beforeLosses ||
    newBulletSelfDamage.length > 0
  ) {
    throw new Error(`${label} shooter died or recorded bullet self-damage while firing: ${JSON.stringify({
      before: beforeMetrics,
      after: afterMetrics,
      newBulletSelfDamage,
      damageTrace: (afterState.damageTrace || []).slice(beforeState.damageTrace?.length ?? 0),
    })}`);
  }
}

async function assertVictimDamageTraceIdentity(victim, event, label, sinceIndex) {
  const state = await readBulletFeedback(victim);
  const matches = (state.damageTrace || [])
    .slice(sinceIndex)
    .filter((entry) => damageTraceMatches(entry, event));
  if (matches.length !== 1) {
    throw new Error(`${label} expected one matching victim damage trace before first-post-hit HUD assertion: ${JSON.stringify({
      event,
      matches,
      damageTrace: (state.damageTrace || []).slice(sinceIndex),
      uiMetrics: state.uiMetrics,
    })}`);
  }
  const [entry] = matches;
  const problems = [];
  if (entry.forSelf !== true) {
    problems.push("trace is not marked forSelf");
  }
  if (entry.damageSeq !== event.damageSeq || entry.serverTick !== event.serverTick) {
    problems.push("damage trace did not preserve server-owned damage identity");
  }
  if (entry.projectileId !== event.projectileId) {
    problems.push("damage trace did not preserve server-owned projectile identity");
  }
  if (!Number.isFinite(entry.energyBefore) || Math.abs(entry.energyBefore - event.energyBefore) > 1) {
    problems.push("damage trace energyBefore did not match server event");
  }
  // WATCH_DAMAGE is a presentation fact processed immediately before its
  // carrier snapshot, or independently on the reliable event lane. It must
  // not mutate self energy by reusable player id. Therefore these callback-
  // time HUD fields can still contain the predecessor snapshot and are not a
  // rendered-energy assertion boundary. assertEnergyTraceWindow below binds
  // the exact identity to the first post-poll/render samples instead.
  if (entry.playerAlive !== true || entry.serverPlayerAlive !== true) {
    problems.push("nonlethal damage trace did not preserve alive state");
  }
  if (!Number.isFinite(entry.selfDamageEnergyAfter) || Math.abs(entry.selfDamageEnergyAfter - event.energyAfter) > 1) {
    problems.push("self damage energy trace did not match server event");
  }
  if (problems.length > 0) {
    throw new Error(`${label} victim damage identity trace mismatch: ${JSON.stringify({
      problems,
      event,
      entry,
      uiMetrics: state.uiMetrics,
      recentMetrics: (state.metricsTrace || []).slice(-20),
    })}`);
  }
}

async function assertEnergyTraceWindow(client, event, label, _traceStart, damageTraceStart, {
  energyField,
  seqField,
  requireDrop,
  requireAuthoritativeResult = false,
  remotePlayerId = null,
}) {
  const state = await readBulletFeedback(client);
  const damageEntry = (state.damageTrace || [])
    .slice(damageTraceStart)
    .find((entry) => damageTraceMatches(entry, event));
  if (!damageEntry) {
    throw new Error(`${label} missing matching damage trace before energy-window assertion: ${JSON.stringify({
      event,
      damageTrace: (state.damageTrace || []).slice(damageTraceStart),
    })}`);
  }

  const damageSeq = damageEntry[seqField];
  const samples = (state.metricsTrace || [])
    .filter((entry) =>
      Number.isFinite(entry.at) &&
      entry.at >= damageEntry.at &&
      entry.at <= damageEntry.at + Math.max(1_200, damageSampleWindowMs + 120) &&
      (damageSeq == null || (entry[seqField] ?? 0) >= damageSeq) &&
      Number.isFinite(sampleEnergy(entry, energyField, remotePlayerId))
    );
  if (samples.length < 4) {
    throw new Error(`${label} did not capture enough post-damage energy samples: ${JSON.stringify({
      event,
      damageEntry,
      samples,
      traceTail: (state.metricsTrace || []).slice(-30),
    })}`);
  }

  const values = samples.map((entry) => sampleEnergy(entry, energyField, remotePlayerId));
  const min = Math.min(...values);
  const first = samples[0];
  const firstEnergy = sampleEnergy(first, energyField, remotePlayerId);
  if (min < event.energyAfter - 2) {
    throw new Error(`${label} energy dipped below server-authoritative damage result: ${JSON.stringify({
      event,
      damageEntry,
      min,
      samples,
    })}`);
  }
  if (requireDrop && min >= event.energyBefore - 2) {
    throw new Error(`${label} never displayed the authoritative damage drop: ${JSON.stringify({
      event,
      damageEntry,
      min,
      samples,
    })}`);
  }
  if (requireAuthoritativeResult) {
    // WATCH_DAMAGE preserves the exact historical impact result but does not
    // mutate energy by reusable player id. Its reliable-lane identity can be
    // observed after a newer snapshot has already advanced authoritative
    // recharge. Bind the rendered value to both ends of that contract: it
    // may be newer than `energyAfter`, but it may neither fall below the
    // impact result nor lead the matching current server-fed energy.
    const windowSamples = samples.filter((entry) => entry.at <= damageEntry.at + damageSampleWindowMs);
    if (windowSamples.length < 3) {
      throw new Error(`${label} did not capture enough post-damage energy samples: ${JSON.stringify({
        event,
        damageEntry,
        damageSampleWindowMs,
        windowSamples,
        samples,
      })}`);
    }
    const firstWindowSample = windowSamples[0];
    const firstWindowEnergy = sampleEnergy(firstWindowSample, energyField, remotePlayerId);
    const firstWindowServerEnergy = sampleServerEnergy(firstWindowSample, remotePlayerId);
    if (
      !Number.isFinite(firstWindowServerEnergy) ||
      firstWindowServerEnergy < event.energyAfter - 2 ||
      firstWindowEnergy < event.energyAfter - 2 ||
      firstWindowEnergy > firstWindowServerEnergy + 2 ||
      (
        energyField === "playerEnergy" &&
        (
          Math.abs((firstWindowSample.localShipLabelEnergy ?? Number.NaN) - firstWindowEnergy) > 2 ||
          Math.abs((firstWindowSample.visibleEnergyDigits ?? Number.NaN) - firstWindowEnergy) > 2
        )
      )
    ) {
      throw new Error(`${label} first visible post-damage sample escaped its authoritative impact-to-current bounds: ${JSON.stringify({
        event,
        damageEntry,
        firstWindowEnergy,
        firstWindowServerEnergy,
        firstWindowSample,
        windowSamples,
      })}`);
    }
    // Persistent damage-result metrics must match the event while displayed
    // and server energy never dips below the hit result during recharge.
    const invalid = windowSamples.filter((entry) => {
      const energy = sampleEnergy(entry, energyField, remotePlayerId);
      const remote = remotePlayerId == null ? null : remotePlayerMetric(entry, remotePlayerId);
      return energy < event.energyAfter - 2 ||
        (
          energyField === "playerEnergy" &&
          Math.abs((entry.lastSelfDamageEnergy ?? Number.NaN) - event.energyAfter) > 1
        ) ||
        (
          remotePlayerId != null &&
          (
            !remote ||
            Math.abs((entry.lastNetworkDamageEnergy ?? Number.NaN) - event.energyAfter) > 1 ||
            (remote.serverEnergy ?? 0) < event.energyAfter - 2
          )
        );
    });
    if (invalid.length > 0) {
      throw new Error(`${label} live post-damage energy contradicted the authoritative damage result: ${JSON.stringify({
        event,
        damageEntry,
        damageSampleWindowMs,
        invalid,
        windowSamples,
      })}`);
    }
    // The bar should recover (recharge) rather than fall back down: each
    // sample stays at or above the previous one, modulo a small tolerance
    // for sampling jitter.
    let previousEnergy = firstWindowEnergy;
    const regressed = windowSamples.slice(1).filter((entry) => {
      const energy = sampleEnergy(entry, energyField, remotePlayerId);
      const dropped = energy < previousEnergy - 2;
      previousEnergy = Math.max(previousEnergy, energy);
      return dropped;
    });
    if (regressed.length > 0) {
      throw new Error(`${label} live post-damage energy regressed instead of recharging upward: ${JSON.stringify({
        event,
        damageEntry,
        damageSampleWindowMs,
        regressed,
        windowSamples,
      })}`);
    }
  }
}

async function waitForVictimEnergyFull(shooter, victim, label) {
  await waitUntil(`${label} victim energy full on local and remote views`, async () => {
    const [shooterState, victimState] = await Promise.all([readBulletFeedback(shooter), readBulletFeedback(victim)]);
    const victimId = victimState.uiMetrics?.selfPlayerId;
    // Roster/position readiness can precede the first remote energy sample.
    // Poll the same strict pre-shot contract; never fire against a partial
    // initial HUD or weaken the later accounting/damage assertions.
    assertVictimFullBeforeShot({label, shooterState, victimState, victimId});
    return true;
  }, 18_000);
}

function isVisibleEnergyFull(metrics) {
  const max = metrics?.playerMaxEnergy ?? 1700;
  return (
    (metrics?.playerEnergy ?? 0) >= max - 1 ||
    (metrics?.visibleEnergyDigits ?? 0) >= max - 1 ||
    (metrics?.localShipLabelEnergy ?? 0) >= max - 1
  );
}

function postSelfDamageSamples(state, event) {
  return (state.metricsTrace || []).filter((entry) =>
    entry.selfPlayerId === event.victimId &&
    entry.lastSelfDamageServerSeq === event.damageSeq &&
    entry.lastSelfDamageServerTick === event.serverTick &&
    entry.lastSelfDamageVictimLifeSeq === event.victimLifeSeq
  );
}

function postRemoteDamageSamples(state, event) {
  return (state.metricsTrace || []).filter((entry) =>
    entry.selfPlayerId === event.shooterId &&
    entry.lastNetworkDamageServerSeq === event.damageSeq &&
    entry.lastNetworkDamageServerTick === event.serverTick &&
    entry.lastNetworkDamagePlayerId === event.victimId &&
    entry.lastNetworkDamageVictimLifeSeq === event.victimLifeSeq
  );
}

function findPrematureVisibleFullSamples(victimState, shooterState, event) {
  const victimSamples = postSelfDamageSamples(victimState, event);
  const shooterSamples = postRemoteDamageSamples(shooterState, event);
  const victimPremature = victimSamples.filter((entry) => {
    const max = entry.playerMaxEnergy ?? 1700;
    return isVisibleEnergyFull(entry) && (entry.serverPlayerEnergy ?? 0) < max - 1;
  });
  const shooterPremature = shooterSamples.filter((entry) => {
    const remote = remotePlayerMetric(entry, event.victimId);
    const max = entry.playerMaxEnergy ?? 1700;
    return (remote?.energy ?? 0) >= max - 1 && (remote?.serverEnergy ?? 0) < max - 1;
  });
  return {
    victimSamples,
    shooterSamples,
    victimPremature,
    shooterPremature,
  };
}

async function assertLocalEnergyDoesNotReachFullBeforeServer(shooter, victim, label, event) {
  if (!event) {
    throw new Error(`${label} missing previous damage event for visible/server energy ordering assertion`);
  }
  const [shooterState, victimState] = await Promise.all([readBulletFeedback(shooter), readBulletFeedback(victim)]);
  const evidence = findPrematureVisibleFullSamples(victimState, shooterState, event);
  if (evidence.victimSamples.length === 0) {
    throw new Error(`${label} could not inspect post-damage victim samples for visible/server energy ordering: ${JSON.stringify({
      event,
      victimMetrics: victimState.uiMetrics,
      traceTail: (victimState.metricsTrace || []).slice(-30),
    })}`);
  }
  if (evidence.victimPremature.length > 0 || evidence.shooterPremature.length > 0) {
    throw new Error(`${label} visible energy reached full before matching server-fed energy: ${JSON.stringify({
      event,
      victimPremature: evidence.victimPremature,
      shooterPremature: evidence.shooterPremature,
      victimTraceTail: (victimState.metricsTrace || []).slice(-30),
      shooterTraceTail: (shooterState.metricsTrace || []).slice(-30),
    })}`);
  }
}

async function waitForVictimLocalHudFullAfterDamage({ shooter, victim, label, previousEvent }) {
  let full = null;
  await waitUntil(`${label} victim local HUD first reaches full after damage`, async () => {
    const [shooterState, victimState] = await Promise.all([readBulletFeedback(shooter), readBulletFeedback(victim)]);
    const evidence = findPrematureVisibleFullSamples(victimState, shooterState, previousEvent);
    if (evidence.victimPremature.length > 0 || evidence.shooterPremature.length > 0) {
      throw new Error(`${label} visible energy reached full before matching server-fed energy: ${JSON.stringify({
        previousEvent,
        victimPremature: evidence.victimPremature,
        shooterPremature: evidence.shooterPremature,
        victimTraceTail: (victimState.metricsTrace || []).slice(-30),
        shooterTraceTail: (shooterState.metricsTrace || []).slice(-30),
      })}`);
    }

    const metrics = victimState.uiMetrics || {};
    const max = metrics.playerMaxEnergy ?? 1700;
    if (
      metrics.playerAlive === true &&
      metrics.serverPlayerAlive === true &&
      isVisibleEnergyFull(metrics)
    ) {
      full = {
        victim: metrics,
        shooterRemote: remotePlayerMetric(shooterState.uiMetrics, previousEvent.victimId),
      };
      if ((metrics.serverPlayerEnergy ?? 0) < max - 1) {
        throw new Error(`${label} victim HUD reached full before victim server energy reached full: ${JSON.stringify({
          previousEvent,
          full,
          victimTraceTail: (victimState.metricsTrace || []).slice(-30),
        })}`);
      }
      return true;
    }
    return false;
  }, 18_000);
  return full;
}

async function fireOneBulletAndAssert({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
  repeatKeyDowns = false,
  pressMs = bulletPressMs,
}) {
  await aimSingleBulletAtVictim(shooter, victim, label);
  const [shooterBefore, victimBefore] = await Promise.all([readBulletFeedback(shooter), readBulletFeedback(victim)]);
  assertVictimFullBeforeShot({
    label,
    shooterState: shooterBefore,
    victimState: victimBefore,
    victimId,
  });
  const shooterTraceStart = shooterBefore.damageTrace?.length ?? 0;
  const victimTraceStart = victimBefore.damageTrace?.length ?? 0;
  const shooterMetricStart = shooterBefore.metricsTrace?.length ?? 0;
  const victimMetricStart = victimBefore.metricsTrace?.length ?? 0;
  await waitForCombatViewReadyAfterCapture(shooter, label);

  if (repeatKeyDowns) {
    await shooter.pressWithRepeats("Control", pressMs, repeatedKeyDownEveryMs);
  } else {
    await shooter.press("Control", pressMs);
  }
  await waitUntil(`${label} one server bullet hit`, async () => {
    return matchingEvents(server, sinceCount, shooterId, victimId).length >= 1;
  }, 10_000);
  await sleep(damageSampleWindowMs + 150);

  const events = matchingEvents(server, sinceCount, shooterId, victimId);
  if (events.length !== 1) {
    throw new Error(`${label} expected exactly one server damage event for one bullet key press: ${JSON.stringify({
      bulletPressMs,
      repeatedBulletPressMs,
      events,
    })}`);
  }
  const [event] = events;
  const selfDamageEvents = bulletSelfDamageEvents(server, sinceCount, shooterId);
  if (selfDamageEvents.length > 0) {
    throw new Error(`${label} produced authoritative bullet self-damage: ${JSON.stringify({
      shooterId,
      selfDamageEvents,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  if (!Number.isInteger(event.damageSeq) || !Number.isInteger(event.serverTick)) {
    throw new Error(`${label} missing server-owned damage sequence/tick: ${JSON.stringify(event)}`);
  }
  if (!Number.isInteger(event.victimLifeSeq) || !Number.isInteger(event.shooterLifeSeq)) {
    throw new Error(`${label} missing server-owned life sequence identity: ${JSON.stringify(event)}`);
  }
  assertServerDamageAccounting(event, label);
  if (event.damage !== 400 || event.energyBefore !== 1700 || event.energyAfter !== 1300) {
    throw new Error(`${label} unexpected single bullet damage values: ${JSON.stringify(event)}`);
  }
  await Promise.all([
    assertSingleBrowserDamageTrace(shooter, event, `${label} shooter`, shooterTraceStart),
    assertSingleBrowserDamageTrace(victim, event, `${label} victim`, victimTraceStart),
  ]);
  await assertVictimDamageTraceIdentity(victim, event, `${label} victim`, victimTraceStart);
  await Promise.all([
    assertEnergyTraceWindow(victim, event, `${label} victim local HUD`, victimMetricStart, victimTraceStart, {
      energyField: "playerEnergy",
      seqField: "lastSelfDamageSeq",
      requireDrop: true,
      requireAuthoritativeResult: true,
    }),
    assertEnergyTraceWindow(shooter, event, `${label} shooter remote HUD`, shooterMetricStart, shooterTraceStart, {
      energyField: "remoteEnergy",
      seqField: "lastNetworkDamageSeq",
      requireDrop: true,
      requireAuthoritativeResult: true,
      remotePlayerId: victimId,
    }),
  ]);
  await assertShooterDidNotSelfDie(shooter, shooterBefore, label);
  return {
    event,
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function fireOneBulletServerOnlyAssert({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
  repeatKeyDowns = false,
  pressMs = bulletPressMs,
  cpuThrottleClients = [],
  cpuThrottleEvidence = null,
}) {
  await aimSingleBulletAtVictim(shooter, victim, label);
  const [shooterBefore, victimBefore] = await Promise.all([readBulletFeedback(shooter), readBulletFeedback(victim)]);
  assertVictimFullBeforeShot({
    label,
    shooterState: shooterBefore,
    victimState: victimBefore,
    victimId,
  });
  const shooterTraceStart = shooterBefore.damageTrace?.length ?? 0;
  const victimTraceStart = victimBefore.damageTrace?.length ?? 0;
  const shooterMetricStart = shooterBefore.metricsTrace?.length ?? 0;
  const victimMetricStart = victimBefore.metricsTrace?.length ?? 0;
  if (cpuThrottleClients.length) {
    // Complete the pre-shot full-state captures before stressing gameplay.
    // Throttling their serialization instead manufactured multi-second pauses
    // before any physical weapon input. Retain that failed profile separately.
    cpuThrottleEvidence.startedAt = Date.now();
    await Promise.all(cpuThrottleClients.map(client => client.setCpuThrottlingRate(cpuThrottleRate)));
    await waitUntil("CPU-stressed rendered frames before physical shot", async () => {
      cpuThrottleEvidence.frames = await Promise.all(cpuThrottleClients.map(client => client.evaluate(
        `window.__zubspaceMetricsTrace.filter(f => f.at >= ${cpuThrottleEvidence.startedAt}).slice(-32).map(f => ({at:f.at, sequence:f.clientInputSeq}))`,
      )));
      return cpuThrottleEvidence.frames.every(frames => frames.length >= 8);
    }, 10_000);
  }
  await waitForCombatViewReadyAfterCapture(shooter, label);

  if (repeatKeyDowns) {
    await shooter.pressWithRepeats("Control", pressMs, repeatedKeyDownEveryMs);
  } else {
    await shooter.press("Control", pressMs);
  }
  await waitUntil(`${label} one server bullet hit`, async () => {
    return matchingEvents(server, sinceCount, shooterId, victimId).length >= 1;
  }, 10_000);
  await sleep(180);

  const events = matchingEvents(server, sinceCount, shooterId, victimId);
  if (events.length !== 1) {
    throw new Error(`${label} expected exactly one server damage event for one immediate counterfire bullet: ${JSON.stringify({
      bulletPressMs,
      repeatedBulletPressMs,
      events,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  const [event] = events;
  const selfDamageEvents = bulletSelfDamageEvents(server, sinceCount, shooterId);
  if (selfDamageEvents.length > 0) {
    throw new Error(`${label} produced authoritative bullet self-damage: ${JSON.stringify({
      shooterId,
      selfDamageEvents,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  assertServerDamageAccounting(event, label);
  if (event.damage !== 400 || event.energyBefore !== 1700 || event.energyAfter !== 1300) {
    throw new Error(`${label} unexpected immediate single bullet damage values: ${JSON.stringify(event)}`);
  }
  await Promise.all([
    assertSingleBrowserDamageTrace(shooter, event, `${label} shooter`, shooterTraceStart),
    assertSingleBrowserDamageTrace(victim, event, `${label} victim`, victimTraceStart),
  ]);
  await assertVictimDamageTraceIdentity(victim, event, `${label} victim`, victimTraceStart);
  await Promise.all([
    assertEnergyTraceWindow(victim, event, `${label} victim local HUD`, victimMetricStart, victimTraceStart, {
      energyField: "playerEnergy",
      seqField: "lastSelfDamageSeq",
      requireDrop: true,
      requireAuthoritativeResult: true,
    }),
    assertEnergyTraceWindow(shooter, event, `${label} shooter remote HUD`, shooterMetricStart, shooterTraceStart, {
      energyField: "remoteEnergy",
      seqField: "lastNetworkDamageSeq",
      requireDrop: true,
      requireAuthoritativeResult: true,
      remotePlayerId: victimId,
    }),
  ]);
  await assertShooterDidNotSelfDie(shooter, shooterBefore, label);
  return {
    event,
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function fireOneBulletAtVictimHudFullAndAssert({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
  previousEvent,
  repeatKeyDowns = false,
  pressMs = bulletPressMs,
}) {
  // Turn BEFORE waiting for the first full-HUD sample, so aiming adds no
  // deliberate delay between that observed recharge boundary and the tap.
  await aimSingleBulletAtVictim(shooter, victim, label);
  const full = await waitForVictimLocalHudFullAfterDamage({
    shooter,
    victim,
    label,
    previousEvent,
  });
  const [shooterBefore, victimBefore] = await Promise.all([readBulletFeedback(shooter), readBulletFeedback(victim)]);
  const shooterTraceStart = shooterBefore.damageTrace?.length ?? 0;
  const victimTraceStart = victimBefore.damageTrace?.length ?? 0;
  const shooterMetricStart = shooterBefore.metricsTrace?.length ?? 0;
  const victimMetricStart = victimBefore.metricsTrace?.length ?? 0;
  const victimMetrics = victimBefore.uiMetrics || {};
  const victimMax = victimMetrics.playerMaxEnergy ?? 1700;
  if (!isVisibleEnergyFull(victimMetrics) || (victimMetrics.serverPlayerEnergy ?? 0) < victimMax - 1) {
    throw new Error(`${label} fired without victim HUD and server energy both full: ${JSON.stringify({
      previousEvent,
      full,
      victimMetrics,
      shooterRemote: remotePlayerMetric(shooterBefore.uiMetrics, victimId),
    })}`);
  }
  await waitForCombatViewReadyAfterCapture(shooter, label);

  if (repeatKeyDowns) {
    await shooter.pressWithRepeats("Control", pressMs, repeatedKeyDownEveryMs);
  } else {
    await shooter.press("Control", pressMs);
  }
  await waitUntil(`${label} one server bullet hit`, async () => {
    return matchingEvents(server, sinceCount, shooterId, victimId).length >= 1;
  }, 10_000);
  await sleep(damageSampleWindowMs + 150);

  const events = matchingEvents(server, sinceCount, shooterId, victimId);
  if (events.length !== 1) {
    throw new Error(`${label} expected exactly one server damage event for one HUD-full bullet tap: ${JSON.stringify({
      bulletPressMs,
      repeatedBulletPressMs,
      previousEvent,
      full,
      events,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  const [event] = events;
  const selfDamageEvents = bulletSelfDamageEvents(server, sinceCount, shooterId);
  if (selfDamageEvents.length > 0) {
    throw new Error(`${label} produced authoritative bullet self-damage: ${JSON.stringify({
      shooterId,
      selfDamageEvents,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  assertServerDamageAccounting(event, label);
  if (event.damage !== 400 || event.energyBefore !== 1700 || event.energyAfter !== 1300) {
    throw new Error(`${label} unexpected HUD-full single bullet damage values: ${JSON.stringify({
      previousEvent,
      full,
      event,
      victimBefore: victimMetrics,
      shooterRemoteBefore: remotePlayerMetric(shooterBefore.uiMetrics, victimId),
    })}`);
  }
  await Promise.all([
    assertSingleBrowserDamageTrace(shooter, event, `${label} shooter`, shooterTraceStart),
    assertSingleBrowserDamageTrace(victim, event, `${label} victim`, victimTraceStart),
  ]);
  await assertVictimDamageTraceIdentity(victim, event, `${label} victim`, victimTraceStart);
  await Promise.all([
    assertEnergyTraceWindow(victim, event, `${label} victim local HUD`, victimMetricStart, victimTraceStart, {
      energyField: "playerEnergy",
      seqField: "lastSelfDamageSeq",
      requireDrop: true,
      requireAuthoritativeResult: true,
    }),
    assertEnergyTraceWindow(shooter, event, `${label} shooter remote HUD`, shooterMetricStart, shooterTraceStart, {
      energyField: "remoteEnergy",
      seqField: "lastNetworkDamageSeq",
      requireDrop: true,
      requireAuthoritativeResult: true,
      remotePlayerId: victimId,
    }),
  ]);
  await assertShooterDidNotSelfDie(shooter, shooterBefore, label);
  return {
    event,
    full,
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function main() {
  if (!Number.isFinite(cpuThrottleRate) || cpuThrottleRate < 1 || cpuThrottleRate > 100) {
    throw new Error("single-bullet CPU rate must be in 1..100");
  }
  if (!Number.isInteger(shotCount) || shotCount < 2) {
    throw new Error(`invalid ZUBSPACE_E2E_SINGLE_BULLET_SHOTS=${process.env.ZUBSPACE_E2E_SINGLE_BULLET_SHOTS || ""}`);
  }
  if (!Number.isFinite(bulletPressMs) || bulletPressMs <= 0) {
    throw new Error(`invalid ZUBSPACE_E2E_SINGLE_BULLET_PRESS_MS=${process.env.ZUBSPACE_E2E_SINGLE_BULLET_PRESS_MS || ""}`);
  }
  if (
    !Number.isFinite(repeatedBulletPressMs) ||
    repeatedBulletPressMs <= repeatedKeyDownEveryMs * 2 ||
    repeatedBulletPressMs >= initialWarbirdBulletCooldownMs
  ) {
    throw new Error(`invalid repeated-key cadence: ${JSON.stringify({
      arena,
      repeatedBulletPressMs,
      repeatedKeyDownEveryMs,
      initialWarbirdBulletCooldownMs,
    })}`);
  }
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }

  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/single_bullet_damage.mjs", wasm: wasmPath, clientEntry });
  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaPort, "single-alpha");
  const betaChrome = startChrome(betaPort, "single-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const cpuThrottleEvidence = { rate: cpuThrottleRate,
    diagnosticOnly: cpuThrottleRate !== 1, participants: ["alpha", "beta"] };
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs,
      "cpu-throttling": JSON.stringify(cpuThrottleEvidence),
      "single-tap-aim": JSON.stringify({ alpha: alpha?.singleBulletAimTrace || [],
        beta: beta?.singleBulletAimTrace || [] }) },
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
    alphaUrl = `${pageBase}&name=singlealpha`;
    betaUrl = `${pageBase}&name=singlebeta`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta);

    const [alphaBefore, betaBefore] = await Promise.all([alpha.pageState(), beta.pageState()]);
    const alphaId = summarizeClient(alphaBefore).selfId;
    const betaId = summarizeClient(betaBefore).selfId;
    if (alphaId == null || betaId == null || alphaId === betaId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ alphaId, betaId })}`);
    }
    const shots = [];
    const counterShots = [];
    let sinceCount = parseServerDamageEvents(server.logs).length;
    await Promise.all([
      waitForVictimEnergyFull(alpha, beta, "manual-regression initial beta readiness"),
      waitForVictimEnergyFull(beta, alpha, "manual-regression initial alpha readiness"),
    ]);
    const immediateAlpha = await fireOneBulletServerOnlyAssert({
      label: "manual-regression repeated-keydown first shot alpha to beta",
      cpuThrottleClients: cpuThrottleRate !== 1 ? [alpha, beta] : [],
      cpuThrottleEvidence,
      shooter: alpha,
      victim: beta,
      server,
      shooterId: alphaId,
      victimId: betaId,
      sinceCount,
      repeatKeyDowns: true,
      pressMs: repeatedBulletPressMs,
    });
    shots.push(immediateAlpha);
    sinceCount = immediateAlpha.nextSinceCount;
    const immediateBeta = await fireOneBulletServerOnlyAssert({
      label: "manual-regression immediate counter shot beta to alpha",
      shooter: beta,
      victim: alpha,
      server,
      shooterId: betaId,
      victimId: alphaId,
      sinceCount,
    });
    counterShots.push(immediateBeta);
    sinceCount = immediateBeta.nextSinceCount;
    await Promise.all([
      waitForVictimEnergyFull(alpha, beta, "manual-regression beta regen after immediate hit"),
      waitForVictimEnergyFull(beta, alpha, "manual-regression alpha regen after immediate counterfire"),
    ]);

    for (let index = 0; index < shotCount; index += 1) {
      if (index > 0) {
        const previousShot = shots[shots.length - 1]?.event;
        await assertLocalEnergyDoesNotReachFullBeforeServer(
          alpha,
          beta,
          `between one-tap shots ${index} and ${index + 1}`,
          previousShot,
        );
        if (index === 1) {
          const result = await fireOneBulletAtVictimHudFullAndAssert({
            label: `manual-regression repeated-keydown visible-HUD-full shot ${index + 1}`,
            shooter: alpha,
            victim: beta,
            server,
            shooterId: alphaId,
            victimId: betaId,
            sinceCount,
            previousEvent: previousShot,
            repeatKeyDowns: true,
            pressMs: repeatedBulletPressMs,
          });
          shots.push(result);
          sinceCount = result.nextSinceCount;
          continue;
        } else {
          await waitForVictimEnergyFull(
            alpha,
            beta,
            `between one-tap shots ${index} and ${index + 1}`,
          );
        }
      }
      const result = await fireOneBulletAndAssert({
        label:
          index === 0
            ? "single bullet first shot"
            : `single bullet shot ${index + 1} after full regen`,
        shooter: alpha,
        victim: beta,
        server,
        shooterId: alphaId,
        victimId: betaId,
        sinceCount,
      });
      shots.push(result);
      sinceCount = result.nextSinceCount;
      if (index === 0) {
        const counterSinceCount = sinceCount;
        const counter = await fireOneBulletAndAssert({
          label: "single bullet immediate counter shot beta to alpha",
          shooter: beta,
          victim: alpha,
          server,
          shooterId: betaId,
          victimId: alphaId,
          sinceCount,
        });
        const betaSelfDamage = bulletSelfDamageEvents(server, counterSinceCount, betaId);
        if (betaSelfDamage.length > 0) {
          throw new Error(`beta counter-fire produced bullet self-damage: ${JSON.stringify({
            betaId,
            betaSelfDamage,
            serverDamageEvents: parseServerDamageEvents(server.logs).slice(counterSinceCount),
          })}`);
        }
        counterShots.push(counter);
        sinceCount = counter.nextSinceCount;
      }
    }

    const [alphaAfter, betaAfter] = await Promise.all([readBulletFeedback(alpha), readBulletFeedback(beta)]);
    const lifeSeqs = new Set(shots.map((shot) => shot.event.victimLifeSeq));
    if (lifeSeqs.size !== 1) {
      throw new Error(`single-bullet no-death sequence changed victim life_seq unexpectedly: ${JSON.stringify(shots.map((shot) => shot.event))}`);
    }
    // Existing one-tap cases intentionally overlap the spawn hulls: prediction
    // can hit before its first authoritative state arrives. Preserve them and
    // add a real-key separated-flight phase to witness live replacement.
    const beforeReconciliation = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    const counterBefore = beforeReconciliation.map(state =>
      state.uiMetrics?.localGameplayState?.ownedProjectileReconciliations || 0);
    // A northward line from Chaos spawn crosses door 169 after 56 clear
    // pixels. A timed hold can therefore measure the door cycle instead of
    // movement. Use the shipped map, treating every door as blocked, to
    // select a hull-clear forward or counter-thrust runway. Keep the same
    // 120-pixel displacement and eight-second movement deadline.
    const zoneDirectory = path.join(repoRoot, "assets/zones", arena);
    const manifest = fs.readFileSync(path.join(zoneDirectory, "zone.bundle"), "utf8");
    const mapFile = manifest.match(/^map=(.+)$/m)?.[1]?.trim();
    if (!mapFile || path.basename(mapFile) !== mapFile) throw new Error("invalid owner recovery map manifest");
    const geometry = authoredMapGeometry(fs.readFileSync(path.join(zoneDirectory, mapFile)));
    const runway = ownerRecoveryRunway(geometry, beforeReconciliation[1].uiMetrics);
    await beta.keyDown(runway.key);
    try {
      await waitUntil("separate ships for owner recovery flight", async () => {
        const m = (await beta.metricsState()).uiMetrics;
        return (m.playerX - runway.x) * runway.dx + (m.playerY - runway.y) * runway.dy > 120;
      }, 8_000);
    } finally { await beta.keyUp(runway.key); }
    const brake = await brakeClientVelocity(beta, 4_000);
    if (!brake.braked) throw new Error(`owner recovery staging failed to brake: ${JSON.stringify(brake)}`);
    const separated = await beta.metricsState();
    const separationPixels = Math.hypot(separated.uiMetrics.playerX - beforeReconciliation[0].uiMetrics.playerX,
      separated.uiMetrics.playerY - beforeReconciliation[0].uiMetrics.playerY);
    if (separationPixels <= 120) throw new Error(`owner recovery staging lost separation: ${separationPixels}`);
    for (const [index, client] of [alpha, beta].entries()) {
      // Alpha's original north-facing barrel also points at the same door.
      // Aim both real barrels down the certified corridor before demanding
      // an in-flight owner reconciliation, rather than a wall-impact race.
      const aim = await orientClientForProtocolDirection(client, runway.dx, runway.dy);
      if (!aim.oriented) throw new Error(`owner recovery pilot ${index} failed corridor aim: ${JSON.stringify(aim)}`);
      await waitForCombatViewReadyAfterCapture(client, `owner recovery pilot ${index}`);
      await client.press("Control", bulletPressMs);
      await waitUntil(`pilot ${index} reconciles an in-flight prediction`, async () =>
        (await client.metricsState()).uiMetrics.localGameplayState.ownedProjectileReconciliations > counterBefore[index],
      5_000);
    }
    const ownerReconciliation = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    const reconciliations = ownerReconciliation.map(state =>
      state.uiMetrics?.localGameplayState?.ownedProjectileReconciliations || 0);
    if (reconciliations.some(count => count <= 0)) {
      throw new Error(`both firing clients must realize owner projectile reconciliation: ${JSON.stringify(reconciliations)}`);
    }
    const result = await evidence.succeed({
      ownerProjectileReconciliations: reconciliations,
      ownerRecoveryFlight: { genuineMovement: true, separationPixels, runway, brake,
        counterBefore, counterAfter: reconciliations },
      ok: true,
      transport: "rtc",
      browserPair: "chrome-chrome",
      diagnosticOnly: cpuThrottleRate !== 1,
      cpuThrottleRate,
      cpuThrottleEvidence,
      arena,
      bulletPressMs,
      repeatedBulletPressMs,
      alphaId,
      betaId,
      events: shots.map((shot) => shot.event),
      counterEvents: counterShots.map((shot) => shot.event),
      alpha: compactMetrics(alphaAfter.uiMetrics),
      beta: compactMetrics(betaAfter.uiMetrics),
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    // Error.stack was already materialized by failure capture. Updating only
    // message leaves the CLI's stack output without its artifact location.
    error.stack = `${error.stack || String(error)}\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
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

export { main, assertEnergyTraceWindow, findPrematureVisibleFullSamples, ownerRecoveryRunway };
