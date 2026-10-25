#!/usr/bin/env node
import { prepareKeyboardLifecycleOrigin } from "./keyboard_lifecycle_origin.mjs";
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import { createBotSpawnFixture } from "./bot_spawn_fixture.mjs";

import { hasPlayerShip, playerShipReady, selectPlayerShip } from "./player_ship_identity.mjs";
import { botAuthorityLoadout, assertBotAuthorityLoadout, stationaryWarbirdBombLaneMinimum, botBombSafetyProfile, botBombSafetyFloor } from "./bot_authority_loadout.mjs";
import fs from "node:fs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { approachLiveBotEncounter } from "./bot_encounter_staging.mjs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { installBotMotionCapture } from "./bot_motion_capture.mjs";
import { proveRemoteMotionSoak } from "./remote_motion_soak.mjs";
import { prepareHostileCounterfireLife, alignCounterfireAssignment, prepareBotFacingSources, facingHunterTarget } from "./counterfire_life.mjs";
import { movingBombShotDisposition } from "./moving_bomb_coverage.mjs";
import { movingBombAttackPlan, movingBombControlStep, movingBombNeedsVelocityControl } from "./moving_bomb_pilot.mjs";
import { collectBotFacingEvidence } from "./bot_facing.mjs";
import { facingLaunchStep } from "./bot_facing_encounter.mjs";
import { agedServerJoinOptions, waitForAgedServerJoin } from "./aged_server_join.mjs";

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
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  brakeClientVelocity,
  browserE2eFailureFooter,
  compactMetrics,
  damageTraceMatches,
  driveClientToWorldTarget,
  openChromeClient,
  parseServerProjectileEvents,
  saveBrowserE2eArtifacts,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { assertRetainedArtifactDirectoryAvailable, validateArtifactPathReferences } from "./artifact_paths.mjs";
import { validateQaTraceArtifacts } from "./qa_trace_artifacts.mjs";
import { assertNetworkProfileEvidence } from "./network_profile_evidence.mjs";
import {
  assertServerObserverProfile,
  serverObserverProfileForNetworkProfile,
} from "./server_observer_profile.mjs";
import { parseZubspaceEvents } from "../../tools/server_events.mjs";
import {
  assertHumanProjectilePresentationGeometry,
  exactLifePrimaryBombTerminal,
  renewRetiredBombTarget,
  bombWireAimDisposition,
  botFiringHeadingReady,
  captureBotPresentationState,
  chaosBombStationaryLaneDisposition,
  lethalHitWithNonlethalPrecursor,
  longestTimedTraceSpan,
  metricsTraceWithViewport,
  observedActorSourceTick,
  ownedProjectileTailSettled,
  projectileInterceptVector as interceptAimVector,
  quantizedProjectileIntercept,
  remoteActorMotionAcrossStatesEvidence,
  botDoorRelocationEvidence,
  remoteActorMotionWithDoorEvidence,
  provePostRespawnBotDamage,
  provePresentedBotCombatCycle,
  proveAuthoritativeBotRepel,
  routedMapCounterfireWaypoint,
  serverDamageBoundary,
  serverDamageEventsSince,
  stationaryChaosBombHoldingPoint,
  stationaryBotMotionEvidence,
  stationaryFixtureLifeSeqs,
  requiredStationaryFixtureLifeSeqs,
  stageExtremeSafeEdgeCombat,
  timedMovementWindows,
  timedTraceContinuity,
  waitForPresentedLiveBotSuccessor,
  worldPointOnCenteredViewport,
  lureTargetOutOfSafety,
  leaveAuthoredSafety,
} from "./bot_combat_evidence.mjs";

const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const clientEntry = path.relative(repoRoot, path.join(applicationRoot, "index.html"));
const serverBotIdBase = 0x0300;
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const expectedBotCount = Number(process.env.ZUBSPACE_SMOKE_BOT_COUNT || "0");
const successArtifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
  (process.env.ZUBSPACE_E2E_ARTIFACT_ROOT ? path.join(process.env.ZUBSPACE_E2E_ARTIFACT_ROOT,
    `zubspace-bots-chrome-${arena}-${Date.now()}-${process.pid}`) : "");
assertRetainedArtifactDirectoryAvailable(successArtifactDir);
const networkProfile = process.env.ZUBSPACE_E2E_NETWORK_PROFILE || "";
const requireFacingCollection = process.env.ZUBSPACE_E2E_REQUIRE_FACING_COLLECTION === "1";
if (requireFacingCollection && networkProfile) throw new Error("fresh exact-tick facing requires a clean timeline");
const networkProfileSeed = Number(process.env.ZUBSPACE_E2E_NETWORK_PROFILE_SEED || "271828");
const serverObserverProfile = serverObserverProfileForNetworkProfile(networkProfile);
const stationaryBotAfterHumanDamage = ["1", "true"].includes(
  String(process.env.ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE || "")
    .trim()
    .toLowerCase(),
);
const stationaryBotFromLifeSeq = Number(
  process.env.ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ || "0",
);
const stationaryBotStaticLifeSeqs = String(
  process.env.ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ || "",
).split(",").map((value) => value.trim()).filter(Boolean).map(Number);
const passiveBotLifeSeqs = String(
  process.env.ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ || "",
).split(",").map((value) => value.trim()).filter(Boolean).map(Number);
const passiveBotLifeSeq = passiveBotLifeSeqs.at(-1) || 0;
const activeProjectileImpairment =
  process.env.ZUBSPACE_E2E_ACTIVE_PROJECTILE_IMPAIRMENT === "1";
const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/bot_authority.mjs";
const bulletWeaponTypes = new Set([1, 2]);
const loadoutFixture = botAuthorityLoadout(process.env, arena);
const bombSafetyProfile = botBombSafetyProfile(loadoutFixture, fs.readFileSync(
  path.join(repoRoot, "assets/zones", arena, "arena.conf"), "utf8"));
// Exact type, not a union: maximum entry grants proximity in both zones;
// the separately declared reference Extreme fixture still requires direct.
const bombWeaponTypes = new Set([loadoutFixture.bombWeaponType]);
const hiddenPrizeSeed = Number(process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED || "2017");
const extremeRecoveryMaximumSpeed = 30;
const extremeSafeEdgeSearchRadiusPixels = 40 * 16;
// Production bots pursue to a 320px combat envelope. Leave one 20px
// snapshot/hull margin so the real-key pilot can acquire that ordinary
// encounter instead of chasing a target that is behaving correctly. This is
// still far inside the authored Warbird bomb's 1,075px lifetime range, and
// exact quantized intercept plus server-owned damage remain mandatory.
const extremeBombAcquisitionDistance = 340;
const extremeBombFiringDistance = extremeBombAcquisitionDistance;
const extremeBombProtectedCrossingDistance = 44;
const extremeBombExpectedExitSpeed = 170;
const extremeBombMinimumStationaryLaneDistance = arena === "extreme"
  ? stationaryWarbirdBombLaneMinimum({
      fixture: loadoutFixture,
      arenaConfig: fs.readFileSync(path.join(repoRoot, "assets/zones/extreme/arena.conf"), "utf8"),
      expectedExitSpeed: extremeBombExpectedExitSpeed,
      rotationPerSecond: 300 / 400,
    })
  : 0;
// Extreme Warbird's authored InitialRotation=300 maps to 0.75 turns/s. A
// worst-case half-turn therefore needs 667 ms before DevTools sampling; keep
// the exposed revalidation bound physically possible without turning it into
// a combat-pursuit timeout.
const extremeBombExposedAimTimeoutMs = 750;
// Reciprocal lethality needs the map-backed clear lane owned by the shared
// 120/150/180px waypoint planner, not merely any point inside the bot's 320px
// pursuit envelope. At 250px a wire-quantized Extreme stream can land only
// sparse edge hits while authored recharge erases every decrement. The
// ordinary-key route and bounded brake below now make the 180px terminal
// physically reachable without changing damage, recharge, or bot authority.
const botCounterfireStagingDistance = 180;
const extremeBombProtectedCrossingSeconds =
  2 * extremeBombProtectedCrossingDistance / extremeBombExpectedExitSpeed;
process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = String(hiddenPrizeSeed);
process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";

function reportStage(stage, details = {}) {
  console.error(JSON.stringify({
    type: "bot_authority_chrome_stage",
    stage,
    at: Date.now(),
    ...details,
  }));
}

if (!["chaos-zone", "extreme"].includes(arena)) {
  throw new Error("bot authority replay is scoped to chaos-zone and extreme");
}
if (
  arena === "extreme" &&
  (!stationaryBotAfterHumanDamage || stationaryBotFromLifeSeq !== 1 ||
    stationaryBotStaticLifeSeqs.join(",") !== "2,3")
) {
  throw new Error(
    "Chrome Extreme exact-life geometry requires " +
    "ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 and " +
    "ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 and " +
    "ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3: life 1 must move " +
    "until its first human hit, lives 2/3 are stationary geometry oracles, and the " +
    "hostile life 4 successor must resume ordinary pursuit",
  );
}
if (
  stationaryBotStaticLifeSeqs.some((lifeSeq) => !Number.isInteger(lifeSeq) || lifeSeq <= 0) ||
  stationaryBotStaticLifeSeqs.some((lifeSeq, index) =>
    index > 0 && lifeSeq <= stationaryBotStaticLifeSeqs[index - 1]
  )
) {
  throw new Error("Chrome stationary bot lives must be unique increasing positive integers");
}
if (!Number.isInteger(expectedBotCount) || expectedBotCount < 1) {
  throw new Error("bot authority replay requires ZUBSPACE_SMOKE_BOT_COUNT>=1");
}
if (
  passiveBotLifeSeqs.some((lifeSeq) => !Number.isInteger(lifeSeq) || lifeSeq <= 0) ||
  passiveBotLifeSeqs.some((lifeSeq, index) => index > 0 && lifeSeq <= passiveBotLifeSeqs[index - 1])
) {
  throw new Error("Chrome passive bot lives must be unique increasing positive integers");
}
const supportedPassiveCollisionFixture =
  passiveBotLifeSeqs.length === 0 ||
  (
    arena === "chaos-zone" && passiveBotLifeSeqs.length === 1 &&
    passiveBotLifeSeq === 2
  ) ||
  (
    arena === "chaos-zone" && expectedBotCount === 2 && !networkProfile &&
    passiveBotLifeSeqs.join(",") === "1,2" &&
    !stationaryBotAfterHumanDamage && stationaryBotFromLifeSeq === 0 &&
    stationaryBotStaticLifeSeqs.length === 0
  ) ||
  (arena === "extreme" && passiveBotLifeSeqs.join(",") === "1,3");
if (!supportedPassiveCollisionFixture) {
  throw new Error(
    "Chrome passive combat isolation requires Chaos life 2, clean two-moving-bot Chaos lives 1/2, or Extreme lives 1 and 3",
  );
}
if (!Number.isInteger(hiddenPrizeSeed) || hiddenPrizeSeed === 0) {
  throw new Error("bot authority replay requires a nonzero integer hidden-prize seed");
}
if (!["", "mild", "slow"].includes(networkProfile)) {
  throw new Error("bot authority replay supports only mild/slow deterministic network profiles");
}
if (!Number.isInteger(networkProfileSeed) || networkProfileSeed < 1 || networkProfileSeed > 0xffff_ffff) {
  throw new Error("bot authority replay network profile seed must be a nonzero u32 integer");
}

function loadAuthoredTiles(mapFile) {
  const bytes = fs.readFileSync(path.join(repoRoot, mapFile));
  const offset = bytes.subarray(0, 2).toString() === "BM" ? bytes.readUInt32LE(2) : 0;
  const tiles = new Uint8Array(1_024 * 1_024);
  for (let cursor = offset; cursor + 4 <= bytes.length; cursor += 4) {
    const packed = bytes.readUInt32LE(cursor);
    const x = packed & 0x0fff;
    const y = (packed >>> 12) & 0x0fff;
    if (x < 1_024 && y < 1_024) tiles[y * 1_024 + x] = packed >>> 24;
  }
  return tiles;
}

const authoredTiles = loadAuthoredTiles(arena === "extreme"
  ? "assets/zones/extreme/doomsy8.lvl"
  : "assets/zones/chaos-zone/bdhadar.lvl");

function positionIsOnAuthoredSafeTile(x, y) {
  if (!Number.isFinite(x) || !Number.isFinite(y)) return false;
  const tileX = Math.floor(x / 16);
  const tileY = Math.floor(y / 16);
  if (tileX < 0 || tileY < 0 || tileX >= 1_024 || tileY >= 1_024) return false;
  return authoredTiles[tileY * 1_024 + tileX] === 171;
}

function authoredTileIsSolid(id) {
  return id !== 0 && (
    (id >= 162 && id <= 169) ||
    id < 170 ||
    (id >= 192 && id <= 240) ||
    (id >= 242 && id <= 252)
  );
}

function authoredLineIsClear(fromX, fromY, toX, toY) {
  const distance = Math.hypot(toX - fromX, toY - fromY);
  if (!Number.isFinite(distance) || distance <= 0.01) return false;
  for (let traveled = 8; traveled < distance - 8; traveled += 4) {
    const ratio = traveled / distance;
    const x = fromX + (toX - fromX) * ratio;
    const y = fromY + (toY - fromY) * ratio;
    const tileX = Math.floor(x / 16);
    const tileY = Math.floor(y / 16);
    if (
      tileX < 0 || tileY < 0 || tileX >= 1_024 || tileY >= 1_024 ||
      authoredTileIsSolid(authoredTiles[tileY * 1_024 + tileX])
    ) {
      return false;
    }
  }
  return true;
}

function authoredHullIsTraversable(x, y, radius = 14) {
  if (!Number.isFinite(x) || !Number.isFinite(y)) return false;
  const minTileX = Math.floor((x - radius) / 16);
  const maxTileX = Math.floor((x + radius) / 16);
  const minTileY = Math.floor((y - radius) / 16);
  const maxTileY = Math.floor((y + radius) / 16);
  if (minTileX < 0 || minTileY < 0 || maxTileX >= 1_024 || maxTileY >= 1_024) {
    return false;
  }
  for (let tileY = minTileY; tileY <= maxTileY; tileY += 1) {
    for (let tileX = minTileX; tileX <= maxTileX; tileX += 1) {
      if (authoredTileIsSolid(authoredTiles[tileY * 1_024 + tileX])) return false;
    }
  }
  return true;
}

function authoredHullIsOpen(x, y, radius = 14) {
  return authoredHullIsTraversable(x, y, radius) && !positionIsOnAuthoredSafeTile(x, y);
}

function routedCounterfireWaypoint(metrics, target) {
  return routedMapCounterfireWaypoint({
    metrics,
    target,
    authoredHullIsTraversable,
    authoredHullIsOpen,
    authoredLineIsClear,
  });
}

async function returnExtremeChromePilotToSafety(actor) {
  const center = { x: 8_200, y: 8_200 };
  if (!positionIsOnAuthoredSafeTile(center.x, center.y)) {
    throw new Error("Doomsy8's retained central recovery point is not authored safety");
  }
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await waitUntil("Extreme Chrome pilot is live before returning to safety", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
    }, 15_000);
    const before = (await actor.metricsState()).uiMetrics || {};
    const beforeSpeed = Math.hypot(
      Number(before.playerVelocityX) || 0,
      Number(before.playerVelocityY) || 0,
    );
    if (before.inSafeZone === true && beforeSpeed <= extremeRecoveryMaximumSpeed) return true;
    try {
      await navigateAuthoredTarget(actor, center, null, {
        timeoutMs: 90_000, arrivalRadiusPx: 24, maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
      });
    } catch (error) {
      const after = (await actor.metricsState()).uiMetrics || {};
      const retired = after.playerLifeSeq !== before.playerLifeSeq ||
        after.playerAlive !== true || after.serverPlayerAlive !== true;
      // Recovery setup may legitimately lose its pilot to the hostile
      // successor. Retire that route and reacquire the next exact live scope
      // within the original three attempts. A live route failure stays RED.
      if (error.message !== "authored navigation crossed a lifecycle boundary" || !retired) throw error;
      continue;
    }
    const returned = await driveClientToWorldTarget(
      actor,
      center,
      20_000,
      true,
      12,
      null,
      100,
    );
    const after = (await actor.metricsState()).uiMetrics || {};
    if (returned.reached && after.inSafeZone === true) return true;
  }
  return false;
}

const extremeChromeSafetyGeometry = {
  authoredTiles,
  positionIsOnAuthoredSafeTile,
  authoredTileIsSolid,
  authoredHullIsOpen,
  authoredLineIsClear,
};

async function lureExtremeChromeTargetOutOfSafety(actor, preferredTargetId, preferredTargetLifeSeq) {
  return lureTargetOutOfSafety({
    safety: extremeChromeSafetyGeometry,
    actor,
    preferredTargetId,
    preferredTargetLifeSeq,
    label: "Extreme Chrome",
    leaveSafety: leaveAuthoredSafety,
  });
}

// Maintain a real hostile firing encounter while the unchanged exact-tick
// oracle collects. Merely leaving the bot outside safety does not establish range.
async function provokeChromeBotLaunches(actor, server, botIds, cycle = 0) {
  const metrics = (await actor.metricsState()).uiMetrics || {};
  const hostile = facingHunterTarget(metrics, botIds, server.logs.slice(-256).join(""));
  if (hostile == null) return false;
  const plan = facingLaunchStep({metrics, target: hostile, cycle,
    geometry: {authoredHullIsTraversable, authoredHullIsOpen, authoredLineIsClear}});
  reportStage("facing_launch_step", {cycle, playerLifeSeq: metrics.playerLifeSeq,
    targetId: hostile.id, targetLifeSeq: hostile.lifeSeq, ...plan});
  if (plan.waypoint) return driveClientToWorldTarget(
    actor, plan.waypoint, 1_200, false, 24, null, 250, 0.8, 1_200);
  if (plan.phase === "observe") return brakeClientVelocity(actor, 1_200);
  return false;
}

async function stageExtremeChromeBombEncounter(actor, preferredTargetId) {
  await waitUntil("Extreme Chrome pilot is live before bomb staging", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
  }, 15_000);
  const initial = (await actor.metricsState()).uiMetrics || {};
  const initialTarget = (initial.remotePlayers || []).find((player) =>
    player.id === preferredTargetId &&
    Number.isFinite(player.positionX) && Number.isFinite(player.positionY)
  );
  const stationaryDirectBombTarget = initialTarget != null &&
    stationaryFixtureLifeSeqs(
      [initialTarget.lifeSeq],
      stationaryBotFromLifeSeq,
      stationaryBotStaticLifeSeqs,
    ).length === 1;
  if (initial.inSafeZone !== true && !await returnExtremeChromePilotToSafety(actor)) {
    return { staged: false, diagnostics: [{ phase: "return_to_safety_failed" }] };
  }
  await waitUntil("Extreme Chrome bomb reserve recovers in authored safety", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
      metrics.inSafeZone === true &&
      (metrics.playerEnergy ?? 0) >= 650 &&
      (metrics.serverPlayerEnergy ?? 0) >= 650;
  }, 15_000);

  const diagnostics = [];
  const staged = await stageExtremeSafeEdgeCombat({
    actor,
    botIdBase: serverBotIdBase,
    authoredTiles,
    positionIsOnAuthoredSafeTile,
    authoredTileIsSolid,
    authoredHullIsOpen,
    authoredLineIsClear,
    preferredTargetId,
    crossingFireKey: "z",
    crossingProjectileSpeed: 430,
    crossingProjectileMaxTime: 2,
    // Acquire an authored edge before Firefox/Chrome route-and-brake latency,
    // then wait for the exact target life to enter the tighter direct-bomb
    // corridor immediately before the real safe->unsafe crossing.
    crossingApproachDistance: extremeBombAcquisitionDistance,
    crossingMinimumApproachDistance: stationaryDirectBombTarget
      ? extremeBombMinimumStationaryLaneDistance
      : 0,
    crossingFinalApproachDistance: extremeBombFiringDistance,
    crossingMinimumApproachAlignment: 0.9,
    crossingProtectedAimTimeout: 12_000,
    crossingMaximumDistance: extremeBombFiringDistance,
    crossingExposedMaximumDistance: extremeBombFiringDistance,
    // Reacquire after the real safe->unsafe transition from the actual
    // canonical velocity, but do not emit at a full-speed receding target.
    // Retained lethal artifacts fire during the bot's ordinary aim/braking
    // phase (roughly 20px/s radial separation); the failed matrix attempt
    // fired at more than 200px/s and missed by a complete collision radius.
    crossingMaximumRecedingSpeed: 80,
    // The accepted weapon direction is a 40-bin Continuum heading and the
    // target advances between headed-browser samples. This bound permits one
    // adjacent moving-target bin; exact server-owned lethal damage remains
    // the acceptance boundary below.
    crossingOrientationTolerance: 0.04,
    crossingThrustModifierKey: "Shift",
    // Life 2 and its successors are held stationary by the exact-life server
    // fixture above. Solve those immutable targets while protected, then
    // submit the real bomb key on the first observed nonsafe sample.
    // Reacquiring only after crossing makes a southbound pilot turn while it
    // is already passing the target; once the inherited velocity becomes
    // receding the correct radial-velocity guard can never accept the shot.
    // The early authored bomb proof can still target moving life 1 and must
    // retain post-crossing revalidation.
    crossingExposedSettleDistance: 0,
    crossingRequiredExposedRunwayDistance: 48,
    crossingPreserveProtectedAim: true,
    crossingRevalidateProtectedAim: !stationaryDirectBombTarget,
    crossingReleaseThrustBeforeProtectedFire: true,
    crossingPreFireDelaySeconds: extremeBombProtectedCrossingSeconds,
    crossingExpectedExitSpeed: extremeBombExpectedExitSpeed,
    crossingExpectedExitSpeedAlongAim: extremeBombExpectedExitSpeed,
    // A legal 40-bin bomb can leave an axis-aligned safe edge one adjacent
    // heading away (the retained west-edge life-2 path is 18 degrees off the
    // edge normal once inherited velocity is included). The shared helper
    // still requires the exact quantized hull path to clear authored geometry;
    // this matches the Firefox bomb contract instead of rejecting that valid
    // path with the generic 0.97 bullet alignment.
    crossingMinimumExitAimAlignment: 0.9,
    crossingExposedAimTimeout: extremeBombExposedAimTimeoutMs,
    diagnostics,
  });
  if (
    staged === false &&
    diagnostics.at(-1)?.phase === "no_safe_edge" &&
    initialTarget != null
  ) {
    // A missing aligned edge is a statement about this exact bot position.
    // The shared search covers forty authored tiles around the protected
    // pilot, then owns a 300px rendezvous. A preceding mobile combat pass can
    // carry the retained bot beyond that geometry, so wait for the bot to
    // re-enter the exact derived envelope instead of spending every bounded
    // retry after an arbitrary 32px movement.
    try {
      await waitUntil("Extreme Chrome bot enters the safe-edge search envelope", async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        const current = (metrics.remotePlayers || []).find((player) =>
          player.id === initialTarget.id &&
          Number.isFinite(player.positionX) && Number.isFinite(player.positionY)
        );
        return current == null || current.lifeSeq !== initialTarget.lifeSeq ||
          Math.hypot(
            current.positionX - metrics.playerX,
            current.positionY - metrics.playerY,
          ) <= extremeSafeEdgeSearchRadiusPixels + extremeBombAcquisitionDistance;
      }, 20_000);
    } catch (error) {
      diagnostics.push({
        at: Date.now(),
        phase: "no_safe_edge_envelope_wait_expired",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return { staged, diagnostics };
}

function botDamageEvents(server, playerId, boundary = 0) {
  return serverDamageEventsSince(server, boundary).filter((event) =>
    event.shooterId >= serverBotIdBase && event.victimId === playerId
  );
}

function playerDamageToBots(server, playerId, boundary = 0) {
  return serverDamageEventsSince(server, boundary).filter((event) =>
    event.shooterId === playerId && event.victimId >= serverBotIdBase
  );
}

function playerWeaponDamageToBots(server, playerId, offset, weaponTypes) {
  return playerDamageToBots(server, playerId, offset).filter((event) =>
    weaponTypes.has(event.weapon & 0x1f)
  );
}

function lethalPlayerWeaponDamageToBots(server, playerId, offset, weaponTypes) {
  return playerWeaponDamageToBots(server, playerId, offset, weaponTypes).filter((event) =>
    event.lethal === true
  );
}

function traceRecords(server, marker) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes(marker))
    .map((line) => {
      const record = {};
      for (const match of line.matchAll(/(\w+)=("[^"]*"|\S+)/g)) {
        const raw = match[2];
        if (raw === "true" || raw === "false") {
          record[match[1]] = raw === "true";
        } else if (raw !== "" && Number.isFinite(Number(raw))) {
          record[match[1]] = Number(raw);
        } else {
          record[match[1]] = raw.replace(/^"|"$/g, "");
        }
      }
      return record;
    });
}

function combatHiddenPrizeEvidence(server) {
  const prizeIds = server.logs.join("").split("\n")
    .filter((line) => line.includes("server hidden prize spawned"))
    .map((line) => Number(line.match(/\bprize_id=(-?\d+)/)?.[1]))
    .filter(Number.isFinite);
  const protectionIds = new Set([13, 17, 18, 25]);
  const protectionSpawns = prizeIds.filter((prizeId) =>
    protectionIds.has(Math.abs(prizeId))
  );
  // Preserve every potentially disruptive spawn as evidence, but do not
  // confuse an unclaimed green elsewhere in the arena with a target-side
  // reset. The combat cycle binds preparation, damage, and lethal ownership
  // to one exact bot life; that is the authoritative interference boundary.
  return {
    hiddenPrizeSeed,
    spawnedCount: prizeIds.length,
    prizeIds,
    protectionSpawns,
  };
}

function botWallCollisionEvidence(server, botIds) {
  const records = traceRecords(server, "server bot movement trace")
    .filter((record) => botIds.includes(record.bot_id));
  const unsafe = records.filter((record) => record.hull_clear !== true);
  const contacts = records.filter((record) =>
    record.collision_x === true || record.collision_y === true
  );
  const avoidance = records.filter((record) => record.navigation_avoiding_wall === true);
  if (records.length < 120 || unsafe.length > 0 || contacts.length + avoidance.length < 1) {
    throw new Error(`server bot wall trace was incomplete or unsafe: ${JSON.stringify({
      records: records.length,
      unsafe: unsafe.slice(0, 5),
      contacts: contacts.slice(0, 5),
      avoidance: avoidance.slice(0, 5),
    })}`);
  }
  return {
    records: records.length,
    unsafeHullRecords: 0,
    contactCount: contacts.length,
    avoidanceCount: avoidance.length,
    contacts: contacts.slice(0, 20),
    avoidance: avoidance.slice(0, 20),
  };
}

function botMetrics(metrics) {
  return (metrics?.remotePlayers || []).filter((player) => player.id >= serverBotIdBase);
}

function visibleRemotePosition(player) {
  return {
    x: Number.isFinite(player?.displayX) ? player.displayX : player?.positionX,
    y: Number.isFinite(player?.displayY) ? player.displayY : player?.positionY,
  };
}

function correlateMovingBombPresentation(server, state, playerId, boundary) {
  const presentation = state.damagePresentationTrace || [];
  const collisions = parseZubspaceEvents(server.logs)
    .map((entry) => entry.event)
    .filter((event) => event.type === "server_collision_witness");
  const correlated = playerDamageToBots(server, playerId, boundary)
    .filter((hit) => bombWeaponTypes.has(hit.weapon & 0x1f))
    .map((hit) => {
      const browser = presentation.findLast((entry) =>
        entry?.damage?.projectileId === hit.projectileId &&
        entry?.damage?.victimId === hit.victimId &&
        entry?.damage?.victimLifeSeq === hit.victimLifeSeq
      );
      const serverWitness = collisions.find((event) =>
        event.projectile_id === hit.projectileId &&
        event.target_id === hit.victimId &&
        event.target_life_seq === hit.victimLifeSeq
      );
      return browser && serverWitness ? { hit, browser, server: serverWitness } : null;
    })
    .filter(Boolean)
    .filter(({ browser, server: witness }) =>
      witness.history_source === "observer_presentation" &&
      typeof witness.presentation_geometry?.status === "string" &&
      witness.presentation_geometry.status.startsWith("resolved_") &&
      witness.requested_target_tick === witness.presentation_geometry.render_tick &&
      witness.selected_target_tick === witness.presentation_geometry.render_tick &&
      witness.collision_anchor_x === witness.presentation_geometry.target_x &&
      witness.collision_anchor_y === witness.presentation_geometry.target_y &&
      Number.isFinite(witness.presentation_geometry.authority_error) &&
      witness.presentation_geometry.authority_error <= 12 &&
      browser.sprite?.drawn === true &&
      browser.sprite?.suppression === "none" &&
      Number.isFinite(browser.sprite?.authoritativeX) &&
      Number.isFinite(browser.sprite?.authoritativeY) &&
      Number.isFinite(browser.sprite?.renderedX) &&
      Number.isFinite(browser.sprite?.renderedY) &&
      Number.isFinite(witness.collision_anchor_x) &&
      Number.isFinite(witness.collision_anchor_y) &&
      witness.collision_anchor_x === witness.damage_anchor_x &&
      witness.collision_anchor_y === witness.damage_anchor_y
    );
  let timelineMotion = null;
  for (let left = 0; left < correlated.length; left += 1) {
    for (let right = left + 1; right < correlated.length; right += 1) {
      const first = correlated[left];
      const second = correlated[right];
      if (
        first.hit.victimId !== second.hit.victimId ||
        first.hit.victimLifeSeq !== second.hit.victimLifeSeq
      ) continue;
      const anchorTravel = Math.hypot(
        second.server.collision_anchor_x - first.server.collision_anchor_x,
        second.server.collision_anchor_y - first.server.collision_anchor_y,
      );
      if (timelineMotion == null || anchorTravel > timelineMotion.anchorTravel) {
        timelineMotion = { anchorTravel, first, second };
      }
    }
  }

  const presentationErrors = timelineMotion == null
    ? null
    : [timelineMotion.first, timelineMotion.second].map(({ browser, server: witness }) => ({
        authoritative: Math.hypot(
          browser.sprite.authoritativeX - witness.collision_anchor_x,
          browser.sprite.authoritativeY - witness.collision_anchor_y,
        ),
        rendered: Math.hypot(
          browser.sprite.renderedX - witness.collision_anchor_x,
          browser.sprite.renderedY - witness.collision_anchor_y,
        ),
        snapshotAgeTicks: Math.abs(
          witness.collision_tick - browser.sprite.lastServerPositionTick,
        ),
      }));
  const materialSameLifeMotion = timelineMotion?.anchorTravel >= 64;
  const boundedPresentation = presentationErrors?.every((error) =>
    error.authoritative <= 32 &&
    error.rendered <= 48 &&
    error.snapshotAgeTicks <= 12
  ) === true;
  if (!materialSameLifeMotion || !boundedPresentation) {
    throw new Error(`moving-bot bomb trace did not retain material same-life motion with bounded presentation error: ${JSON.stringify({
      timelineMotion,
      presentationErrors,
      correlated: correlated.map(({ hit, browser, server: witness }) => ({
        hit,
        browser: {
          at: browser.at,
          snapshotId: browser.snapshotId,
          receiptTiers: browser.receiptTiers,
          sprite: browser.sprite,
        },
        server: witness,
      })),
    })}`);
  }
  return {
    ...timelineMotion,
    damageDelta: Math.abs(timelineMotion.first.hit.damage - timelineMotion.second.hit.damage),
    presentationErrors,
  };
}

function u32IsStrictlyNewer(candidate, predecessor) {
  if (!Number.isInteger(candidate) || !Number.isInteger(predecessor)) return false;
  const distance = (candidate - predecessor + 0x1_0000_0000) % 0x1_0000_0000;
  return distance > 0 && distance < 0x8000_0000;
}

function assertBotFeelTrace(metricsTrace, botIds, startedAt, minimumSpanMs, label, doorRelocations = []) {
  const frames = (metricsTrace || []).filter((entry) => entry.at >= startedAt);
  const continuity = timedTraceContinuity(frames, { minimumSpanMs });
  if (!continuity.ok) {
    throw new Error(`${label} did not retain a continuous elapsed-time metrics trace: ${JSON.stringify(continuity)}`);
  }
  const bots = [];
  for (const botId of botIds) {
    const lifeSamples = new Map();
    let presentFrames = 0;
    let authoritativeLiveFrames = 0;
    let authoritativeRenderedFrames = 0;
    let overtakenDeathFrames = 0;
    let longestPresenceMs = 0;
    let currentPresenceStartedAt = null;
    let lastPresentLife = null;
    for (const frame of frames) {
      const authoritativeActor = (frame.authoritativeSnapshotActors || [])
        .find((candidate) => candidate.id === botId);
      const presentation = (frame.remoteActorPresentation || [])
        .find((candidate) => candidate.id === botId);
      const player = (frame.remotePlayers || []).find((candidate) => candidate.id === botId);
      // The reliable death event can overtake one final unordered snapshot of
      // the predecessor life. The renderer must suppress that dead life. It
      // is not a live-AOI omission when the roster already carries a strictly
      // newer wrap-safe life; matching-life snapshots remain exact-required.
      const deathOvertookSnapshot = authoritativeActor &&
        u32IsStrictlyNewer(presentation?.lifeSeq, authoritativeActor.lifeSeq);
      if (deathOvertookSnapshot) {
        overtakenDeathFrames += 1;
      } else if (authoritativeActor) {
        authoritativeLiveFrames += 1;
        const exactLiveRender = player &&
          player.lifeSeq === authoritativeActor.lifeSeq &&
          presentation?.lifeSeq === authoritativeActor.lifeSeq &&
          presentation.positionPresent === true &&
          presentation.renderPositionPresent === true;
        if (!exactLiveRender) {
          throw new Error(`${label} bot ${botId} was omitted while alive in the authoritative AOI: ${JSON.stringify({
            at: frame.at,
            authoritativeActor,
            presentation: presentation || null,
            player: player || null,
          })}`);
        }
        authoritativeRenderedFrames += 1;
      }
      if (!player) {
        if (currentPresenceStartedAt != null) {
          longestPresenceMs = Math.max(longestPresenceMs, frame.at - currentPresenceStartedAt);
          currentPresenceStartedAt = null;
        }
        continue;
      }
      if (lastPresentLife != null && player.lifeSeq !== lastPresentLife) {
        currentPresenceStartedAt = frame.at;
      } else if (currentPresenceStartedAt == null) {
        currentPresenceStartedAt = frame.at;
      }
      lastPresentLife = player.lifeSeq;
      presentFrames += 1;
      const samples = lifeSamples.get(player.lifeSeq) || [];
      samples.push({
        at: frame.at,
        lifeSeq: player.lifeSeq,
        snapCount: presentation?.snapCount,
        initialCount: presentation?.initialCount,
        lastSnapReason: presentation?.lastSnapReason,
        x: player.positionX,
        y: player.positionY,
        positionX: player.positionX,
        positionY: player.positionY,
        positionTick: observedActorSourceTick(player.lastServerPositionTick, frame.combatViewTimeline),
        positionSnapshotId: presentation?.lastSeenSnapshotId,
        displayX: player.displayX,
        displayY: player.displayY,
        velocityX: player.velocityX,
        velocityY: player.velocityY,
        combatViewTimeline: frame.combatViewTimeline || null,
      });
      lifeSamples.set(player.lifeSeq, samples);
    }
    if (currentPresenceStartedAt != null) {
      longestPresenceMs = Math.max(
        longestPresenceMs,
        frames.at(-1).at - currentPresenceStartedAt,
      );
    }
    const lifeEvidence = [...lifeSamples.entries()].map(([lifeSeq, samples]) => {
      const finite = samples.filter((sample) =>
        [sample.x, sample.y, sample.displayX, sample.displayY].every(Number.isFinite)
      );
      const xValues = finite.map((sample) => sample.x);
      const yValues = finite.map((sample) => sample.y);
      const displacement = finite.length > 0
        ? Math.hypot(Math.max(...xValues) - Math.min(...xValues), Math.max(...yValues) - Math.min(...yValues))
        : 0;
      const displayErrorMax = finite.reduce((maximum, sample) => Math.max(
        maximum,
        Math.hypot(sample.displayX - sample.x, sample.displayY - sample.y),
      ), 0);
      const rollingMovement = timedMovementWindows(finite);
      const smoothness = remoteActorMotionWithDoorEvidence(finite, botId, doorRelocations);
      return {
        lifeSeq,
        sampleCount: samples.length,
        displacement,
        displayErrorMax,
        rollingMovement,
        smoothness,
      };
    });
    const maximumLifeDisplacement = Math.max(0, ...lifeEvidence.map((life) => life.displacement));
    const totalLifeDisplacement = lifeEvidence.reduce(
      (total, life) => total + life.displacement,
      0,
    );
    const rollingMovement = lifeEvidence.flatMap((life) =>
      life.rollingMovement.map((window) => ({ lifeSeq: life.lifeSeq, ...window }))
    );
    const stalledWindows = rollingMovement.filter((window) => window.displacement < 16);
    const failedSmoothness = lifeEvidence.filter((life) => !life.smoothness.ok);
    const presenceRatio = authoritativeLiveFrames === 0
      ? 0
      : authoritativeRenderedFrames / authoritativeLiveFrames;
    const overallPresenceRatio = presentFrames / frames.length;
    // Four authored tiles is unambiguously material on-screen translation,
    // while remaining stable when the eight-second sample begins partway
    // through a bot's deliberate aim/braking phase. The server wall trace
    // below independently requires a continuously clear authoritative hull,
    // so interpolation jitter cannot satisfy this threshold.
    if (
      authoritativeRenderedFrames !== authoritativeLiveFrames ||
      totalLifeDisplacement < 64 ||
      rollingMovement.length === 0 ||
      stalledWindows.length > 0 ||
      failedSmoothness.length > 0
    ) {
      throw new Error(`${label} bot ${botId} did not present credible continuous movement: ${JSON.stringify({
        presenceRatio,
        overallPresenceRatio,
        authoritativeLiveFrames,
        authoritativeRenderedFrames,
        longestPresenceMs,
        maximumLifeDisplacement,
        totalLifeDisplacement,
        rollingMovement,
        stalledWindows,
        failedSmoothness,
        lifeEvidence,
      })}`);
    }
    bots.push({
      botId,
      presentFrames,
      frameCount: frames.length,
      presenceRatio,
      overallPresenceRatio,
      authoritativeLiveFrames,
      authoritativeRenderedFrames,
      overtakenDeathFrames,
      longestPresenceMs,
      maximumLifeDisplacement,
      totalLifeDisplacement,
      rollingMovement,
      lifeEvidence,
    });
  }
  return {
    startedAt,
    endedAt: frames.at(-1).at,
    frameCount: frames.length,
    continuity,
    bots,
    frames: frames.map((frame) => ({
      at: frame.at,
      latestServerTick: frame.latestServerTick,
      latestSnapshotServerTick: frame.latestSnapshotServerTick,
      combatViewTimeline: frame.combatViewTimeline || null,
      authoritativeSnapshotId: frame.authoritativeSnapshotId,
      authoritativeSnapshotActors: frame.authoritativeSnapshotActors || [],
      remoteActorPresentation: (frame.remoteActorPresentation || [])
        .filter((actor) => botIds.includes(actor.id)),
      bots: (frame.remotePlayers || [])
        .filter((player) => botIds.includes(player.id))
        .map((player) => ({
          id: player.id,
          lifeSeq: player.lifeSeq,
          positionX: player.positionX,
          positionY: player.positionY,
          displayX: player.displayX,
          displayY: player.displayY,
          velocityX: player.velocityX,
          velocityY: player.velocityY,
          displayOrientation: player.displayOrientation,
          energy: player.energy,
        })),
    })),
  };
}

function assertBotSpriteTrace(
  spriteTrace,
  botIds,
  startedAt,
  minimumSpanMs,
  label,
) {
  const frames = (spriteTrace || []).filter((frame) => frame.at >= startedAt);
  const continuity = timedTraceContinuity(frames, { minimumSpanMs });
  if (!continuity.ok) {
    throw new Error(`${label} did not retain a continuous elapsed-time sprite trace: ${JSON.stringify(continuity)}`);
  }
  const bots = botIds.map((botId) => {
    let authoritativeFrames = 0;
    let drawnFrames = 0;
    let onCanvasFrames = 0;
    const suppressionCounts = {};
    const shipFrames = new Set();
    const lifecycleSamples = new Map();
    const onCanvasSamples = [];
    for (const frame of frames) {
      const entry = (frame.entries || []).find((candidate) => candidate.id === botId);
      if (!entry?.authoritativeMember) continue;
      authoritativeFrames += 1;
      suppressionCounts[entry.suppression] =
        (suppressionCounts[entry.suppression] || 0) + 1;
      if (entry.drawn) {
        drawnFrames += 1;
        shipFrames.add(entry.shipFrame);
      }
      const onCanvas = Number.isFinite(entry.screenX) && Number.isFinite(entry.screenY) &&
        entry.screenX >= -24 && entry.screenX <= 1_304 &&
        entry.screenY >= -24 && entry.screenY <= 744;
      if (onCanvas) {
        onCanvasFrames += 1;
        onCanvasSamples.push(frame);
        if (
          entry.drawn !== true ||
          entry.suppression !== "none" ||
          !(entry.alpha > 0) ||
          entry.positionPresent !== true ||
          entry.renderPositionPresent !== true
        ) {
          throw new Error(`${label} bot ${botId} suppressed an authoritative on-canvas sprite: ${JSON.stringify({ frame, entry })}`);
        }
      }
      const life = lifecycleSamples.get(entry.lifeSeq) || {
        lifeSeq: entry.lifeSeq,
        frames: 0,
        initialCounts: new Set(),
        snapCounts: new Set(),
      };
      life.frames += 1;
      life.initialCounts.add(entry.presentationInitialCount);
      life.snapCounts.add(entry.presentationSnapCount);
      lifecycleSamples.set(entry.lifeSeq, life);
    }
    const longestOnCanvasSpanMs = longestTimedTraceSpan(onCanvasSamples);
    if (
      authoritativeFrames === 0 ||
      drawnFrames !== authoritativeFrames ||
      longestOnCanvasSpanMs < 750
    ) {
      throw new Error(`${label} bot ${botId} lacked continuous actual sprite draws: ${JSON.stringify({
        authoritativeFrames,
        drawnFrames,
        onCanvasFrames,
        longestOnCanvasSpanMs,
        suppressionCounts,
        shipFrames: [...shipFrames],
      })}`);
    }
    return {
      botId,
      authoritativeFrames,
      drawnFrames,
      onCanvasFrames,
      longestOnCanvasSpanMs,
      suppressionCounts,
      shipFrames: [...shipFrames].sort((left, right) => left - right),
      lives: [...lifecycleSamples.values()].map((life) => ({
        lifeSeq: life.lifeSeq,
        frames: life.frames,
        initialCounts: [...life.initialCounts],
        snapCounts: [...life.snapCounts],
      })),
    };
  });
  return {
    startedAt,
    endedAt: frames.at(-1).at,
    frameCount: frames.length,
    continuity,
    bots,
    frames: frames.map((frame) => ({
      at: frame.at,
      snapshotId: frame.snapshotId,
      entries: (frame.entries || []).filter((entry) => botIds.includes(entry.id)),
    })),
  };
}

function assertBotProjectileOwnersVisible(
  metricsTrace,
  spriteTrace,
  botIds,
  label,
  { required = true } = {},
) {
  const observed = new Map();
  for (const frame of metricsTrace || []) {
    // Runtime state may be waiting for the source actor's delayed birth cursor.
    // Only the prepared render view claims presentation in this frame.
    if (!Array.isArray(frame.presentedRemoteProjectileSources)) {
      throw new Error("missing prepared projectile diagnostics");
    }
    for (const projectile of frame.presentedRemoteProjectileSources) {
      if (
        !botIds.includes(projectile.playerId) ||
        !Number.isInteger(projectile.lifeSeq) ||
        !Number.isInteger(projectile.eventId) ||
        !Number.isInteger(projectile.serverTick) ||
        !Number.isInteger(frame.latestServerTick)
      ) continue;
      const presentationTick = Number.isInteger(projectile.authorityStateTick)
        ? projectile.authorityStateTick
        : projectile.serverTick;
      const age = (frame.latestServerTick - presentationTick) >>> 0;
      if (age > 20) continue;
      // AOI intentionally extends well beyond the viewport. A projectile and
      // its owner may both be authoritative and drawn off-canvas; that is not
      // an invisible-attacker presentation. Bind this assertion only to a
      // projectile source that has actually entered the centered viewport.
      if (!worldPointOnCenteredViewport(frame, projectile.x, projectile.y)) continue;
      const key = `${projectile.playerId}:${projectile.lifeSeq}:${projectile.eventId}`;
      if (!observed.has(key)) observed.set(key, { frame, projectile });
    }
  }
  if (observed.size < 1) {
    if (!required) return null;
    throw new Error(`${label} observed no fresh bot projectile presentation source`);
  }
  const evidence = [];
  for (const { frame, projectile } of observed.values()) {
    if (Number.isInteger(projectile.authorityProjectileId)) {
      const candidates = (spriteTrace || [])
        .filter((spriteFrame) =>
          spriteFrame.at <= frame.at && frame.at - spriteFrame.at <= 1_000
        )
        .flatMap((spriteFrame) =>
          (spriteFrame.projectiles?.entries || [])
            .filter((entry) =>
              entry.authorityProjectileId === projectile.authorityProjectileId &&
              entry.authorityStateTick === projectile.authorityStateTick &&
              entry.sourcePlayerId === projectile.playerId &&
              entry.sourceSessionId === projectile.sessionId &&
              entry.sourceLifeSeq === projectile.lifeSeq &&
              entry.shotGroupId === projectile.shotGroupId &&
              entry.sourceProjectileIndex === projectile.sourceProjectileIndex
            )
            .map((entry) => ({ at: spriteFrame.at, entry }))
        )
        .sort((left, right) => right.at - left.at);
      const drawn = candidates.find(({ entry }) => entry.drawn === true);
      if (!drawn) {
        throw new Error(`${label} installed exact active-projectile state without a matching texture draw: ${JSON.stringify({
          frameAt: frame.at,
          latestServerTick: frame.latestServerTick,
          projectile,
          candidates: candidates.slice(0, 8),
        })}`);
      }
      evidence.push({
        playerId: projectile.playerId,
        lifeSeq: projectile.lifeSeq,
        eventId: projectile.eventId,
        serverTick: projectile.serverTick,
        kind: projectile.kind,
        presentationPath: "exact-active-projectile",
        authorityProjectileId: projectile.authorityProjectileId,
        authorityStateTick: projectile.authorityStateTick,
        projectileFrameAt: frame.at,
        drawFrameAt: drawn.at,
      });
      continue;
    }
    const candidates = (spriteTrace || [])
      // The threat contract is directional: the exact owner life must have
      // been actually drawn before its harmful projectile is presented. A
      // symmetric nearest-frame search could accidentally use a later draw
      // to excuse one invisible-attacker frame.
      .filter((spriteFrame) =>
        spriteFrame.at <= frame.at && frame.at - spriteFrame.at <= 1_000
      )
      .flatMap((spriteFrame) =>
        (spriteFrame.entries || [])
          .filter((entry) =>
            entry.id === projectile.playerId && entry.lifeSeq === projectile.lifeSeq
          )
          .map((entry) => ({ at: spriteFrame.at, entry }))
      )
      .sort((left, right) => right.at - left.at);
    const visible = candidates.find(({ entry }) =>
      entry.authoritativeMember === true &&
      entry.drawn === true &&
      entry.suppression === "none" &&
      entry.alpha > 0 &&
      Number.isFinite(entry.screenX) &&
      Number.isFinite(entry.screenY) &&
      entry.screenX >= -24 && entry.screenX <= 1_304 &&
      entry.screenY >= -24 && entry.screenY <= 744
    );
    if (!visible) {
      throw new Error(`${label} presented a fresh projectile without its live on-canvas owner: ${JSON.stringify({
        frameAt: frame.at,
        latestServerTick: frame.latestServerTick,
        projectile,
        candidates: candidates.slice(0, 8),
      })}`);
    }
    evidence.push({
      playerId: projectile.playerId,
      lifeSeq: projectile.lifeSeq,
      eventId: projectile.eventId,
      serverTick: projectile.serverTick,
      kind: projectile.kind,
      presentationPath: "source-life-shot-group",
      projectileFrameAt: frame.at,
      ownerFrameAt: visible.at,
      ownerLeadMs: frame.at - visible.at,
      ownerScreenX: visible.entry.screenX,
      ownerScreenY: visible.entry.screenY,
    });
  }
  if (!evidence.some((entry) => entry.presentationPath === "exact-active-projectile")) {
    throw new Error(`${label} never proved a self-contained active-projectile texture draw`);
  }
  return {
    freshProjectileGroups: evidence.length,
    exactActiveProjectileDraws: evidence.filter((entry) =>
      entry.presentationPath === "exact-active-projectile"
    ).length,
    evidence: evidence.slice(0, 64),
  };
}

function assertBrowserAudio(state, label) {
  const unlock = state.audioUnlock;
  const probe = state.botAudioProbe;
  if (
    !unlock?.installed ||
    !unlock.gestureSeen ||
    unlock.trackedContexts < 1 ||
    unlock.resumeFailures !== 0
  ) {
    throw new Error(`${label} did not unlock game audio from trusted input: ${JSON.stringify(unlock)}`);
  }
  if (
    !probe ||
    probe.starts < 1 ||
    probe.runningStarts < 1 ||
    probe.sourceConnections < 1 ||
    probe.errors.length > 0
  ) {
    throw new Error(`${label} did not call connected game audio while running: ${JSON.stringify(probe)}`);
  }
  return { unlock, probe };
}

function assertSnapshotOnlyBotPresentation(state, label) {
  const packetAudit = state.packetAudit || null;
  const legacyPositionRelays = packetAudit == null
    ? (state.packetLog || []).filter((entry) =>
        entry.direction === "received" && entry.bytes?.[0] === 0x3a
      )
    : packetAudit.samples || [];
  const legacyPositionRelayCount = packetAudit == null
    ? legacyPositionRelays.length
    : packetAudit.receivedLegacyPositionRelays;
  if (
    packetAudit != null &&
    (
      packetAudit.inspectedPackets < 1 ||
      packetAudit.truncatedPackets < 1 ||
      packetAudit.maximumRetainedBytes > 1
    )
  ) {
    throw new Error(`${label} did not use bounded header-only packet evidence: ${JSON.stringify(packetAudit)}`);
  }
  if (legacyPositionRelayCount > 0) {
    throw new Error(`${label} received retired legacy position relays: ${JSON.stringify({
      count: legacyPositionRelayCount,
      sample: legacyPositionRelays.slice(0, 3),
    })}`);
  }
  return {
    inspectedPackets: packetAudit?.inspectedPackets ?? (state.packetLog || []).length,
    truncatedPackets: packetAudit?.truncatedPackets ?? 0,
    maximumRetainedBytes: packetAudit?.maximumRetainedBytes ?? null,
    receivedLegacyPositionRelays: 0,
  };
}

function assertObjectivesStayDisabled(state, label) {
  const objective = state.uiMetrics?.localGameplayState || {};
  const failures = [];
  if (objective.networkedPlay !== true) failures.push("networkedPlay was not true");
  if (objective.objectivePresentationAuthoritativeOnly !== true) {
    failures.push("objective presentation was not authoritative-only");
  }
  for (const field of [
    "authoritativeFlagCount",
    "authoritativePowerballCount",
    "objectivePresentedFlagCount",
    "objectivePresentedPowerballCount",
  ]) {
    if ((objective[field] ?? 0) !== 0) failures.push(`${field} was not zero`);
  }
  if (objective.localFlagFallbackPresented === true || objective.localPowerballFallbackPresented === true) {
    failures.push("a local objective fallback was presented");
  }
  if (failures.length > 0) {
    throw new Error(`${label} objective fail-closed assertion failed: ${JSON.stringify({ failures, objective })}`);
  }
  return objective;
}

function signedOrientationDelta(current, desired) {
  return ((desired - current + 1.5) % 1) - 0.5;
}

async function proveHeldFireRotation(actor, server, playerId) {
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const before = (await actor.metricsState()).uiMetrics || {};
    if (before.playerAlive !== true || before.serverPlayerAlive !== true) {
      await sleep(250);
      continue;
    }
    const traceOffset = traceRecords(server, "human authority input trace").length;
    await actor.holdThenChord("Control", "ArrowRight", 220, 520);
    const after = (await actor.metricsState()).uiMetrics || {};
    const inputs = traceRecords(server, "human authority input trace").slice(traceOffset)
      .filter((record) =>
        record.player_id === playerId && record.life_seq === before.playerLifeSeq
      );
    const rotatingFire = inputs.filter((record) =>
      (record.input_bits & (1 << 3)) !== 0 &&
      (record.reported_weapon & 0x1f) !== 0
    );
    const rotatingDirections = [...new Set(inputs.filter((record) =>
      (record.input_bits & (1 << 3)) !== 0
    ).map((record) => record.input_direction))];
    if (rotatingFire.length > 0 && rotatingDirections.length >= 4) {
      return {
        firstKey: "Control",
        secondKey: "ArrowRight",
        firstOnlyMs: 220,
        chordMs: 520,
        orientationBefore: before.playerOrientation,
        orientationAfter: after.playerOrientation,
        authoritativeDirections: rotatingDirections,
        survivedChord: after.playerAlive === true &&
          after.serverPlayerAlive === true &&
          after.playerRespawnSeq === before.playerRespawnSeq,
        acceptedInputSamples: rotatingFire.slice(0, 30),
      };
    }
  }
  throw new Error("holding fire before right rotation produced no accepted simultaneous input/turn");
}

async function attackBotsWithRealKeyboard(
  actor,
  server,
  playerId,
  damageOffset,
  preferredTargetId = null,
  requireLethalTargetHit = false,
  weaponMode = "bullet",
  timeoutMs = weaponMode === "bomb" ? 180_000 : 120_000,
  preferredTargetLifeSeq = null,
) {
  const weaponTypes = weaponMode === "bomb" ? bombWeaponTypes : bulletWeaponTypes;
  // Bullet bursts use the game's held Ctrl path; Space owns the player list.
  const fireKey = weaponMode === "bomb" ? "z" : "Control";
  let protectedRespawn = null;
  let targetMotion = null;
  let targetMotionReady = false;
  let lockedBombTarget = null;
  let hits = [];
  let lastFireReleasedAt = null;
  const motionAnchors = new Map();
  const deadline = Date.now() + timeoutMs;
  const hasSatisfyingTarget = (events) => {
    const targetEvents = events.filter((hit) =>
      (preferredTargetId == null || hit.victimId === preferredTargetId) &&
      (preferredTargetLifeSeq == null || hit.victimLifeSeq === preferredTargetLifeSeq)
    );
    if (!requireLethalTargetHit) return targetEvents.length > 0;
    if (weaponMode === "bullet") {
      return lethalHitWithNonlethalPrecursor(targetEvents, preferredTargetId) != null;
    }
    return targetEvents.some((hit) => hit.lethal === true);
  };
  for (let pass = 0; Date.now() < deadline; pass += 1) {
    // A lethal projectile can land during a turn+fire or close+fire chord.
    // Observe the authoritative event at every pass boundary so the
    // configured respawn deadline is measured from the actual death instead
    // of after the remainder of a long keyboard-navigation loop.
    hits = playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    if (Number.isInteger(metrics.selfPlayerId) && metrics.selfPlayerId !== playerId) {
      throw new Error(`Chrome combat session changed player identity: ${JSON.stringify({
        expectedPlayerId: playerId, observedPlayerId: metrics.selfPlayerId,
        lifeSeq: metrics.playerLifeSeq, weaponMode,
      })}`);
    }
    if (hasSatisfyingTarget(hits)) {
      return hits;
    }
    if (weaponMode === "bomb" && requireLethalTargetHit &&
        Number.isInteger(preferredTargetId) && Number.isInteger(preferredTargetLifeSeq) &&
        exactLifePrimaryBombTerminal(serverDamageEventsSince(server, 0), playerId,
          { victimId: preferredTargetId, victimLifeSeq: preferredTargetLifeSeq }).kind !== "pending") return hits;
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) {
      await sleep(180);
      continue;
    }
    if (protectedRespawn !== metrics.playerRespawnSeq) {
      protectedRespawn = metrics.playerRespawnSeq;
      // A new life invalidates every prior motion sample. Extreme owns a
      // separately staged exact bot and must not emit before measured lead.
      // Chaos bullets cannot damage their owner, so retain the genuine
      // opening burst before reacquiring moving-target geometry. Bombs do
      // self-splash and a respawn heading may face nearby authored geometry;
      // they must re-enter the ordinary measured target/clear-lane loop.
      targetMotion = null;
      targetMotionReady = false;
      if (arena !== "extreme" && weaponMode === "bullet") {
        await actor.pressWithRepeats(fireKey, 100, 25);
        lastFireReleasedAt = Date.now();
        await sleep(40);
        hits = playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
        if (hasSatisfyingTarget(hits)) return hits;
      }
    }
    if (arena === "extreme" && metrics.inSafeZone === true) {
      // Leave authored safety through ordinary map-aware keyboard pursuit.
      // Combat admission below remains closed until the browser reports the
      // exact nonsafe state. Do not make the moving bot first volunteer a
      // stationary intercept at one preselected boundary tile; that geometry
      // oracle was unrelated to the human combat behavior under test.
      const protectedTarget = botMetrics(metrics).find((bot) =>
        (preferredTargetId == null || bot.id === preferredTargetId) &&
        (preferredTargetLifeSeq == null || bot.lifeSeq === preferredTargetLifeSeq) &&
        Number.isFinite(visibleRemotePosition(bot).x) &&
        Number.isFinite(visibleRemotePosition(bot).y)
      );
      if (protectedTarget == null) {
        await sleep(80);
        continue;
      }
      const targetPosition = visibleRemotePosition(protectedTarget);
      const directLane = authoredLineIsClear(
        metrics.playerX,
        metrics.playerY,
        targetPosition.x,
        targetPosition.y,
      );
      const routed = directLane
        ? null
        : routedCounterfireWaypoint(metrics, {
            ...protectedTarget,
            positionX: targetPosition.x,
            positionY: targetPosition.y,
          });
      await driveClientToWorldTarget(
        actor,
        directLane ? targetPosition : (routed ?? targetPosition),
        1_200,
        false,
        24,
        null,
        250,
        0.8,
        1_200,
      );
      continue;
    }
    const visibleBots = botMetrics(metrics)
      .filter((bot) =>
        Number.isFinite(visibleRemotePosition(bot).x) &&
        Number.isFinite(visibleRemotePosition(bot).y) &&
        (preferredTargetLifeSeq == null || bot.lifeSeq === preferredTargetLifeSeq) &&
        !positionIsOnAuthoredSafeTile(bot.positionX, bot.positionY)
      );
    if (lockedBombTarget != null && !visibleBots.some((bot) =>
      bot.id === lockedBombTarget.id && bot.lifeSeq === lockedBombTarget.lifeSeq
    )) {
      lockedBombTarget = null;
      targetMotion = null;
      targetMotionReady = false;
    }
    // Do not lock onto a merely selected bot: bot-v-bot combat can make the
    // first human hit unexpectedly lethal. Lock only after a human nonlethal
    // hit, and only while that exact life remains visible, so the proof
    // observes one complete human-owned energy-decrement -> death cycle.
    const openCycle = [...hits].reverse().find((hit) =>
      hit.lethal !== true &&
      (preferredTargetId == null || hit.victimId === preferredTargetId) &&
      (preferredTargetLifeSeq == null || hit.victimLifeSeq === preferredTargetLifeSeq) &&
      visibleBots.some((bot) =>
        bot.id === hit.victimId && bot.lifeSeq === hit.victimLifeSeq
      )
    );
    const target = preferredTargetId != null
      ? visibleBots.find((bot) => bot.id === preferredTargetId)
      : (openCycle == null ? null : visibleBots.find((bot) =>
          bot.id === openCycle.victimId && bot.lifeSeq === openCycle.victimLifeSeq
        )) || (lockedBombTarget == null ? null : visibleBots.find((bot) =>
          bot.id === lockedBombTarget.id && bot.lifeSeq === lockedBombTarget.lifeSeq
        )) || visibleBots.sort((left, right) => weaponMode === "bullet"
          ? (right.energy ?? Number.MIN_SAFE_INTEGER) - (left.energy ?? Number.MIN_SAFE_INTEGER)
          : (left.energy ?? Number.MAX_SAFE_INTEGER) - (right.energy ?? Number.MAX_SAFE_INTEGER)
        )[0];
    if (!target) {
      if (pass % 15 === 0) {
        reportStage("fire_progress", {
          arena,
          weaponMode,
          pass,
          elapsedMs: timeoutMs - Math.max(0, deadline - Date.now()),
          playerAlive: metrics.playerAlive,
          serverPlayerAlive: metrics.serverPlayerAlive,
          inSafeZone: metrics.inSafeZone,
          visibleBotCount: visibleBots.length,
          damageEvents: hits.length,
          semanticInputCounts: metrics.semanticInputCounts,
        });
      }
      await sleep(80);
      continue;
    }
    if (weaponMode === "bomb" && lockedBombTarget == null) {
      lockedBombTarget = { id: target.id, lifeSeq: target.lifeSeq };
    }
    let targetVelocityX = 0;
    let targetVelocityY = 0;
    const visibleTarget = visibleRemotePosition(target);
    if (arena === "chaos-zone" && weaponMode === "bomb" && requireLethalTargetHit) {
      const firstNonlethal = hits.find(hit => hit.victimId === target.id &&
        hit.victimLifeSeq === target.lifeSeq && hit.lethal === false);
      if (firstNonlethal) {
        const key = `${target.id}:${target.lifeSeq}`;
        if (!motionAnchors.has(key)) {
          const witness = parseZubspaceEvents(server.logs).map(entry => entry.event)
            .find(event => event.type === "server_collision_witness" &&
              event.projectile_id === firstNonlethal.projectileId &&
              event.target_id === target.id && event.target_life_seq === target.lifeSeq);
          if (witness) motionAnchors.set(key, witness);
        }
        const anchor = motionAnchors.get(key);
        const spacing = anchor ? movingBombShotDisposition(anchor, target)
          : {action: "await-collision-witness", ready: false};
        if (!spacing.ready) {
          if (pass % 15 === 0) reportStage("moving_bomb_shot_spacing", {
            targetId: target.id, targetLifeSeq: target.lifeSeq,
            firstProjectileId: firstNonlethal.projectileId, ...spacing,
          });
          // Await observed motion, not an arbitrary elapsed delay. The same
          // bounded attack deadline applies; a stationary life cannot pass
          // this moving-target scenario merely by receiving repeated hits.
          await sleep(80);
          continue;
        }
      }
    }
    const targetServerTick = target.lastServerPositionTick;
    const sameTargetSample = targetMotion?.id === target.id &&
      targetMotion.lifeSeq === target.lifeSeq;
    let measuredTargetVelocity = false;
    const sampledAt = Date.now();
    const canonicalTargetSpeed = Math.hypot(
      target.velocityX ?? Number.POSITIVE_INFINITY,
      target.velocityY ?? Number.POSITIVE_INFINITY,
    );
    const canonicalTargetVelocityReady =
      Number.isFinite(target.velocityX) && Number.isFinite(target.velocityY) &&
      canonicalTargetSpeed <= 500;
    if (canonicalTargetVelocityReady) {
      // Match the Firefox pilot: the remote actor metric carries velocity
      // from the same authoritative snapshot as its position. Prefer that
      // source-time pair to browser wall-clock differencing, which loses its
      // first sample after every real-key arrival/brake maneuver.
      targetVelocityX = target.velocityX;
      targetVelocityY = target.velocityY;
      measuredTargetVelocity = true;
      targetMotionReady = true;
    } else if (sameTargetSample && sampledAt > targetMotion.sampledAt) {
      const seconds = (sampledAt - targetMotion.sampledAt) / 1_000;
      const measured = {
        x: (visibleTarget.x - targetMotion.x) / seconds,
        y: (visibleTarget.y - targetMotion.y) / seconds,
      };
      if (Math.hypot(measured.x, measured.y) <= 500) {
        targetVelocityX = targetMotion.velocityX * 0.45 + measured.x * 0.55;
        targetVelocityY = targetMotion.velocityY * 0.45 + measured.y * 0.55;
        measuredTargetVelocity = true;
      }
    }
    if (!sameTargetSample || sampledAt > targetMotion.sampledAt) {
      if (!sameTargetSample) targetMotionReady = false;
      if (measuredTargetVelocity) targetMotionReady = true;
      targetMotion = {
        id: target.id,
        lifeSeq: target.lifeSeq,
        sampledAt,
        x: visibleTarget.x,
        y: visibleTarget.y,
        velocityX: targetVelocityX,
        velocityY: targetVelocityY,
      };
    } else {
      targetVelocityX = targetMotion.velocityX;
      targetVelocityY = targetMotion.velocityY;
    }
    const dx = visibleTarget.x - metrics.playerX;
    const dy = visibleTarget.y - metrics.playerY;
    const distance = Math.hypot(dx, dy);
    // Arena BulletSpeed/BombSpeed and projectile-spawn log velocities are
    // tenths of pixels per second. The registry divides them by ten before
    // integrating world positions; aim must use the same world-space units
    // as the measured target and inherited ship velocity. These exact ships
    // are selected and verified by this fixture: Extreme Lancaster bullets,
    // Extreme Warbird bombs, and Chaos Warbird for both.
    const projectileSpeed = arena === "extreme"
      ? (weaponMode === "bomb" ? 430 : 360)
      : 200;
    // Only the immediate Chaos 68..80px proximity lane can proceed without
    // measured target motion. Outside it, retain the real intercept even
    // inside 120px: a 200px/s bomb takes long enough for a moving hull to
    // leave the original bearing, wasting damage and the next firing window.
    const maximumFlightSeconds = weaponMode === "bomb"
      ? (arena === "extreme" ? 2.5 : 2)
      : (arena === "extreme" ? 0.65 : 5.5);
    const lead = interceptAimVector(
      dx,
      dy,
      targetVelocityX - (metrics.playerVelocityX ?? 0),
      targetVelocityY - (metrics.playerVelocityY ?? 0),
      projectileSpeed,
      maximumFlightSeconds,
    );
    // Moving Chaos needs a source-velocity-aware blast-safe attack. Keep the
    // explicit stationary fixture and Extreme's independent staging unchanged.
    const movingChaosBomb = arena === "chaos-zone" && weaponMode === "bomb" && !stationaryBotAfterHumanDamage;
    const chaosBombPlan = movingChaosBomb ? movingBombAttackPlan({
      relativeX: dx, relativeY: dy, targetVelocityX, targetVelocityY,
      playerVelocityX: metrics.playerVelocityX ?? 0, playerVelocityY: metrics.playerVelocityY ?? 0,
      projectileSpeed, maximumFlightSeconds,
      explosionRadius: bombSafetyProfile.explosionPixels * metrics.localGameplayState?.bombLevel,
      // The reference game's SimulateHit uses18px per proximity tile, independently
      // of the16px fire-only BombSafety exclusion.
      proximityRadius: (bombSafetyProfile.proximityDistance + metrics.localGameplayState?.bombLevel - 1) * 18,
      currentDirection: Number.isInteger(metrics.playerProtocolDirection) ? metrics.playerProtocolDirection
        : Math.floor(Math.fround(metrics.playerOrientation ?? 0) * 40) % 40,
      velocityPathIsClear: (x, y) => {
        const samples = Math.max(1, Math.ceil(Math.hypot(x, y) * 0.4 / 4));
        return Array.from({ length: samples + 1 }, (_, index) => 0.4 * index / samples)
          .every(seconds => authoredHullIsOpen(metrics.playerX + x * seconds, metrics.playerY + y * seconds));
      },
      projectilePathIsClear: (x, y, seconds) => authoredLineIsClear(metrics.playerX, metrics.playerY,
        metrics.playerX + x * seconds, metrics.playerY + y * seconds),
    }) : null;
    // Bullets and sensorless explosives bind to the actual wire cell.
    const quantizedAim = weaponMode === "bullet" ||
      (weaponMode === "bomb" && arena !== "chaos-zone")
      ? quantizedProjectileIntercept({
          relativeX: dx,
          relativeY: dy,
          targetVelocityX,
          targetVelocityY,
          playerVelocityX: metrics.playerVelocityX ?? 0,
          playerVelocityY: metrics.playerVelocityY ?? 0,
          projectileSpeed,
          maxTime: maximumFlightSeconds,
        })
      : null;
    // The cell the ship is emitting right now, so the key press is bound to the shot that can
    // physically leave the barrel instead of a cell the pilot is still turning into.
    const emittedWireAim = weaponMode === "bomb" && arena !== "chaos-zone"
      ? quantizedProjectileIntercept({
          relativeX: dx,
          relativeY: dy,
          targetVelocityX,
          targetVelocityY,
          playerVelocityX: metrics.playerVelocityX ?? 0,
          playerVelocityY: metrics.playerVelocityY ?? 0,
          projectileSpeed,
          maxTime: maximumFlightSeconds,
          fixedDirection: Number.isInteger(metrics.playerProtocolDirection)
            ? metrics.playerProtocolDirection
            : Math.floor(Math.fround(metrics.playerOrientation ?? 0) * 40) % 40,
        })
      : null;
    const aim = chaosBombPlan?.candidate ?? (arena === "chaos-zone" && weaponMode === "bomb" && distance <= 80
      ? { x: dx, y: dy }
      : quantizedAim ?? lead);
    const desired = Number.isFinite(aim?.orientation)
      ? aim.orientation
      : Math.atan2(aim.x, -aim.y) / (Math.PI * 2);
    const wrappedDesired = ((desired % 1) + 1) % 1;
    const current = ((metrics.playerOrientation % 1) + 1) % 1;
    const delta = ((wrappedDesired - current + 1.5) % 1) - 0.5;
    const aimTolerance = 0.013;
    // The moving Chaos plan uses the existing two-second forecast with an
    // explicit owner-clearance check. The stationary fixture keeps its140px
    // lane; neither changes authored weapon lifetime or any damage verdict.
    const fireRange = weaponMode === "bomb"
      // Extreme's 430px/s bomb travels about 1,075px in 2.5s;
      // Chaos' 200px/s bomb travels 400px within this pilot's 2s bound.
      // These shorter engagement lanes still require clear geometry and a
      // feasible moving-target intercept; a launch alone never proves a hit.
      ? (chaosBombPlan?.fireRange ?? (arena === "extreme" ? extremeBombFiringDistance : 140))
      // Extreme's 360px/s bullet travels 234px in its authored 650ms life.
      // Chaos has 1,100px of authored travel and uses a conservative 400px
      // engagement lane. Both retain their quantized hit-envelope checks.
      : (arena === "extreme" ? 234 : 400);
    // Use this zone's authored fire-only safety and the current ship's bomb
    // level. Maximum-entry Extreme also has proximity: a point-blank key
    // edge can be correctly suppressed without producing a server projectile.
    const bombSafetyFloor = weaponMode === "bomb"
      ? botBombSafetyFloor(bombSafetyProfile, metrics.localGameplayState?.bombLevel)
      : 0;
    const lineIsClear = authoredLineIsClear(
      metrics.playerX,
      metrics.playerY,
      visibleTarget.x,
      visibleTarget.y,
    );
    const stationaryBombHoldingPoint = stationaryChaosBombHoldingPoint({
      fixtureActive: stationaryBotAfterHumanDamage,
      arena,
      weaponMode,
      lineIsClear,
      humanDamageObserved: hits.some((event) =>
        event.victimId === target.id && event.victimLifeSeq === target.lifeSeq
      ),
      targetMotionReady,
      targetSpeed: Math.hypot(targetVelocityX, targetVelocityY),
      playerX: metrics.playerX,
      playerY: metrics.playerY,
      targetX: visibleTarget.x,
      targetY: visibleTarget.y,
      hullIsOpen: authoredHullIsOpen,
      firingLineIsClear: authoredLineIsClear,
    });
    if (stationaryBombHoldingPoint != null) {
      await driveClientToWorldTarget(
        actor,
        stationaryBombHoldingPoint,
        4_000,
        true,
        3,
        null,
        160,
        0.7,
        2_000,
      );
      targetMotion = null;
      targetMotionReady = false;
      continue;
    }
    // Continuum truncates orientation into one of 40 protocol directions.
    // A visually exact continuous heading can therefore select the adjacent
    // cell and miss a distant stationary hull. Bind every bullet attempt to
    // the best actual wire direction and its conservative 12px swept miss.
    const quantizedBulletCanHit = weaponMode !== "bullet" || (
      quantizedAim?.feasible === true && quantizedAim.missDistance <= 12
    );
    // The red Chrome/Extreme bomb cell fired fourteen bombs from 284 px at a stationary bot
    // and recorded zero damage. The pilot was aimed at the continuous bearing 0.9676 turns,
    // which Continuum truncated into wire cell 38 (base angle 342 deg): every launch left at
    // velocity (-1334, -4099), a swept path 30 px beside the target and outside the authored
    // 4px explosive + 14px hull contact envelope. The best wire cell at that range predicts a
    // 13.3 px miss, so the long-range explosive branch now requires the wire solution to fit
    // the envelope; inside it, the close-range branches are truncation-safe by geometry
    // (half a wire bin is under 9.4 px of lateral offset at 120 px) and stay as they were.
    const bombWire = bombWireAimDisposition({
      weaponMode, arena, quantized: quantizedAim, current: emittedWireAim,
    });
    const firingHeadingReady = chaosBombPlan ? (chaosBombPlan.candidate != null &&
      chaosBombPlan.current.direction === chaosBombPlan.candidate.direction && chaosBombPlan.current.safeToAttempt) : botFiringHeadingReady({
      delta, tolerance: aimTolerance, wireAim: bombWire,
      quantized: quantizedAim, current: emittedWireAim,
    });
    const targetInFireRange = metrics.inSafeZone !== true &&
      distance <= fireRange &&
      distance >= bombSafetyFloor &&
      lineIsClear &&
      quantizedBulletCanHit &&
      (chaosBombPlan ? (targetMotionReady && chaosBombPlan.candidate != null) :
        (weaponMode !== "bomb" ||
          (arena === "chaos-zone" && distance <= 80) ||
          (arena !== "chaos-zone" && distance <= 120) ||
          (targetMotionReady && lead.feasible && bombWire.fire)));
    const pilotSpeed = Math.hypot(
      metrics.playerVelocityX ?? 0,
      metrics.playerVelocityY ?? 0,
    );
    if (pass % 15 === 0) {
      reportStage("fire_progress", {
        arena,
        weaponMode,
        pass,
        elapsedMs: timeoutMs - Math.max(0, deadline - Date.now()),
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        inSafeZone: metrics.inSafeZone,
        targetId: target.id,
        targetLifeSeq: target.lifeSeq,
        distance,
        orientationDelta: delta,
        lineIsClear,
        targetMotionReady,
        interceptFeasible: lead.feasible,
        quantizedDirection: quantizedAim?.direction ?? null,
        quantizedMissDistance: quantizedAim?.missDistance ?? null,
        bombWireFire: bombWire.fire,
        bombWireMissDistance: bombWire.missDistance,
        bombWireCurrentMissDistance: bombWire.currentMissDistance,
        firingHeadingReady,
        targetInFireRange,
        pilotSpeed,
        damageEvents: hits.length,
        semanticInputCounts: metrics.semanticInputCounts,
      });
    }
    const brakingThreshold = weaponMode === "bomb" && arena === "extreme"
      ? 160
      : arena === "extreme"
        ? 360
        : 160;
    if (!chaosBombPlan && targetInFireRange && pilotSpeed > brakingThreshold) {
      // Cross the firing lane under control. Otherwise an accepted bomb can
      // circle into its moving owner or splash from nearby geometry instead
      // of testing the selected bot's authoritative damage lifecycle.
      await brakeClientVelocity(actor, 2_500);
      targetMotion = null;
      targetMotionReady = false;
      continue;
    }
    const stationaryChaosBombLane = chaosBombStationaryLaneDisposition({
      arena,
      weaponMode,
      targetInFireRange,
      lineIsClear,
      distance,
      pilotSpeed,
      holdingRange: fireRange,
      targetOwnsPursuit: !stationaryBotAfterHumanDamage && !chaosBombPlan,
    });
    if (stationaryChaosBombLane.action === "brake") {
      // The authoritative bot already pursues the only human in this leg.
      // Chasing it at the same time creates a two-body orbit and can keep both
      // actors outside a stable measured bomb intercept indefinitely. Hold a
      // real, stationary nonsafe target and let server AI close to its
      // authored 96px aim brake; only the explicit too-close branch below
      // moves away from it.
      await brakeClientVelocity(actor, 1_500);
      targetMotion = null;
      targetMotionReady = false;
      await sleep(200);
      continue;
    }
    if (stationaryChaosBombLane.action === "wait") {
      // Do not clear targetMotion here: the next authoritative bot snapshot
      // is what turns the stationary close encounter into a measured lead.
      await sleep(80);
      continue;
    }
    if (!lineIsClear || chaosBombPlan?.pathBlocked) {
      // Direct pursuit into an occluding wall can carry the browser pilot
      // farther from a freshly respawned bot until it leaves AOI entirely.
      // Use the same authored-map BFS waypoint for both ordinary weapon
      // classes; this remains genuine keyboard navigation and does not alter
      // target, weapon, packet, or gameplay state.
      const routeWaypoint = routedCounterfireWaypoint(metrics, {
        ...target,
        positionX: visibleTarget.x,
        positionY: visibleTarget.y,
      });
      if (routeWaypoint != null) {
        await driveClientToWorldTarget(
          actor,
          routeWaypoint,
          1_200,
          false,
          24,
          null,
          250,
          0.8,
          1_200,
        );
      } else {
        await sleep(100);
      }
      targetMotion = null;
      targetMotionReady = false;
      continue;
    }
    if (chaosBombPlan && movingBombNeedsVelocityControl(chaosBombPlan, targetInFireRange)) {
      const step = movingBombControlStep(chaosBombPlan, metrics);
      if (pass % 15 === 0) reportStage("moving_bomb_pursuit", { targetId: target.id,
        targetLifeSeq: target.lifeSeq, playerLifeSeq: metrics.playerLifeSeq, plan: chaosBombPlan, step });
      // Follow observed motion at blast-safe separation; reverse thrust keeps
      // the bow available during retreat. No state/weapon/packet injection.
      if (step.keys.length) await actor.holdKeys(step.keys, step.holdMs);
      else await sleep(step.holdMs);
      continue;
    }
    if (bombSafetyFloor > 0 && distance < bombSafetyFloor) {
      const retreatWaypoint = routedCounterfireWaypoint(metrics, {
        ...target,
        positionX: visibleTarget.x,
        positionY: visibleTarget.y,
      });
      if (retreatWaypoint != null) {
        await driveClientToWorldTarget(
          actor,
          retreatWaypoint,
          1_200,
          false,
          24,
          null,
          250,
          0.8,
          1_200,
        );
      } else {
        await brakeClientVelocity(actor, 1_000);
        await sleep(100);
      }
      targetMotion = null;
      targetMotionReady = false;
      continue;
    }
    // Chaos retains its continuous heading tolerance. Extreme must emit the
    // exact solved wire cell with a valid native contact prediction.
    if (
      weaponMode === "bomb" && targetInFireRange &&
      firingHeadingReady
    ) {
      if (chaosBombPlan) reportStage("moving_bomb_key_attempt", {
        playerLifeSeq: metrics.playerLifeSeq, targetId: target.id, targetLifeSeq: target.lifeSeq,
        playerEnergy: metrics.playerEnergy, plan: chaosBombPlan,
        meaning: "physical key attempt; authoritative acceptance and damage checked separately",
      });
      await actor.holdKeys([fireKey], 100);
      lastFireReleasedAt = Date.now();
      await sleep(50);
      hits = playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
      if (hasSatisfyingTarget(hits)) return hits;
    } else if (targetInFireRange && !firingHeadingReady) {
      const turnMs = Math.min(240, Math.max(25, Math.abs(delta) / 0.625 * 700));
      const keys = [delta > 0 ? "ArrowRight" : "ArrowLeft"];
      if (weaponMode === "bullet" && targetInFireRange && Math.abs(delta) <= 0.08) {
        keys.unshift(fireKey);
      }
      // Outside an unobstructed engagement lane, retain authored weapon
      // energy. Inside it, keep ordinary fire held during the final lead
      // correction exactly as a human pilot would; the separate chord matrix
      // still proves every press/release ordering independently.
      await actor.holdKeys(keys, Math.min(turnMs, keys.length > 1 ? 100 : turnMs));
      if (keys.includes(fireKey)) lastFireReleasedAt = Date.now();
    } else if (!targetInFireRange) {
      // Close to short combat range. A longer nominal projectile range is
      // insufficient evidence because an intervening wall can absorb every
      // shot while the pilot remains stationary. Stop thrusting once close so
      // inherited velocity cannot turn the proof into a high-speed chase.
      const desiredRange = weaponMode === "bomb"
        // Close well inside Extreme's conservative 340px authored projectile
        // envelope before settling and emitting the edge.
        ? (arena === "extreme" ? 160 : 76)
        : (arena === "extreme" ? 110 : 96);
      const retreatingFromBombSafety = bombSafetyFloor > 0 &&
        distance < bombSafetyFloor;
      if (
        weaponMode === "bomb" && distance <= fireRange && lineIsClear &&
        !retreatingFromBombSafety && bombWire.fire
      ) {
        // Wait for a second authoritative target sample instead of erasing
        // the first sample and launching behind a receding bot. A point-blank
        // target remains admissible above without a velocity estimate. When the
        // best wire direction cannot fit the contact envelope at this range,
        // waiting cannot help - the lane below closes to `desiredRange`, where
        // half a wire bin does fit inside it.
        await sleep(50);
        continue;
      }
      const closingDistance = retreatingFromBombSafety
        ? bombSafetyFloor + 8 - distance
        : Math.max(metrics.inSafeZone === true ? 80 : 0, distance - desiredRange);
      // Extreme bots can travel at 320 px/s. A 130 px/s browser pilot can
      // never close Lancaster's real 210 px bullet lane against a receding
      // target. Use the authored ship capability for pursuit, then let the
      // firing-lane branch settle before the key press.
      const desiredSpeed = arena === "extreme"
        ? Math.min(350, Math.max(300, Math.sqrt(2 * 180 * closingDistance)))
        : retreatingFromBombSafety
          ? Math.min(110, Math.max(60, Math.sqrt(2 * 180 * closingDistance) * 0.28))
          : Math.min(300, Math.max(
              closingDistance > 96 ? 220 : 80,
              Math.sqrt(2 * 180 * closingDistance),
            ));
      const approachDirection = retreatingFromBombSafety ? -1 : 1;
      const desiredVelocityX = distance > 0.01
        ? dx / distance * desiredSpeed * approachDirection
        : 0;
      const desiredVelocityY = distance > 0.01
        ? dy / distance * desiredSpeed * approachDirection
        : 0;
      const accelX = desiredVelocityX - metrics.playerVelocityX;
      const accelY = desiredVelocityY - metrics.playerVelocityY;
      const closeOrientation = ((Math.atan2(accelX, -accelY) / (Math.PI * 2)) + 1) % 1;
      const closeDelta = signedOrientationDelta(current, closeOrientation);
      // An Extreme bot can outrun the Warbird's ordinary starting speed, and
      // Chaos can settle into the same-speed orbit seen by the Firefox lane.
      // Use the real Shift+thrust Afterburner while closing a bomb lane. The
      // normal client/server paths still own the authored energy cost, speed
      // cap, movement admission, bomb edge, and damage result.
      const useAfterburner = weaponMode === "bomb" &&
        !retreatingFromBombSafety && distance > fireRange;
      if (Math.abs(closeDelta) > 0.025) {
        const turnMs = Math.min(220, Math.max(25, Math.abs(closeDelta) / 0.625 * 650));
        // A moving target rarely remains perfectly aligned on consecutive
        // browser samples. Thrust while steering, as an ordinary pilot does,
        // so pursuit does not degenerate into endless turn-only pulses. Fire
        // remains absent until the independently checked short clear lane.
        await actor.holdKeys(
          [
            ...(useAfterburner ? ["Shift"] : []),
            "ArrowUp",
            closeDelta > 0 ? "ArrowRight" : "ArrowLeft",
          ],
          Math.min(turnMs, 100),
        );
      } else {
        await actor.holdKeys(
          [...(useAfterburner ? ["Shift"] : []), "ArrowUp"],
          distance < 320 ? 55 : 100,
        );
      }
    } else {
      // Every edge is normal browser input. Metrics only close the aim loop;
      // they do not mutate position, weapon, or packet state.
      // The chord matrix separately proves simultaneous fire and steering.
      // Here settle inside a short unobstructed lane and fire without thrust,
      // so a miss cannot be explained by quantized aim plus inherited pilot
      // motion rather than projectile/damage authority.
      // Once an exact same-life target is inside the short unobstructed lane,
      // keep ordinary fire held long enough to land a complete Chaos damage
      // burst before authored recharge can erase the visible decrement. This
      // remains genuine browser keyboard input and mirrors the Firefox driver.
      // A single Chaos bullet can miss a laterally moving bot even with an
      // exact lead. Keep the genuine key held for one complete authored
      // burst on the first clear lane too; otherwise a missed opening round
      // sends the pilot back through a long chase before it can establish
      // the visible same-life decrement that locks this proof to one bot.
      // Bombs are edge-triggered. Holding Z for the full bullet-burst window
      // emits one bomb and then leaves the pilot idle for a second while the
      // target recharges. Release promptly, re-sample the authoritative
      // target, and let the next ordinary key edge occur once the authored
      // cooldown permits it. Bullets remain held across a complete burst.
      // Extreme's authored 1,650-energy bot needs three 610-damage Lancaster
      // rounds. Retain one additional cooldown/travel window so one ordinary
      // miss cannot turn a valid clear lane into another multi-minute chase;
      // Chaos keeps its shorter complete burst.
      const holdMs = weaponMode === "bomb"
        ? 100
        : arena === "extreme"
          ? 2_200
          : 1_100;
      await actor.holdKeys([fireKey], holdMs);
      lastFireReleasedAt = Date.now();
      await sleep(50);
      hits = playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
      if (hasSatisfyingTarget(hits)) {
        return hits;
      }
    }
  }
  // A fire key accepted just before the navigation budget expires still owns
  // an in-flight authoritative projectile. Observe that exact transaction
  // through the authored weapon lifetime instead of declaring failure while
  // the server is still advancing it. Continuum lifetimes are 10 ms ticks:
  // Extreme BulletAliveTime=65 and BombAliveTime=250.
  const authoredTerminalMs = arena === "extreme"
    ? (weaponMode === "bomb" ? 2_500 : 650)
    : (weaponMode === "bomb" ? 80_000 : 5_500);
  const terminalDeadline = lastFireReleasedAt == null
    ? Date.now()
    : lastFireReleasedAt + authoredTerminalMs;
  while (Date.now() < terminalDeadline) {
    hits = playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
    if (hasSatisfyingTarget(hits)) return hits;
    await sleep(20);
  }
  return hits.length > 0
    ? hits
    : playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
}

async function stageForLethalBotCounterfire(
  actor,
  server,
  playerId,
  damageOffset,
  assignedShooterId,
  botIds,
) {
  const preparation = await prepareHostileCounterfireLife({
    target: await waitForPresentedLiveBotSuccessor({
      actor, playerId: assignedShooterId, predecessorLifeSeq: 0,
      label: "Chrome counterfire source",
    }),
    passiveLifeSeqs: passiveBotLifeSeqs,
    retire: async target => {
      const offset = serverDamageBoundary(server);
      const hits = await attackBotsWithRealKeyboard(actor, server, playerId,
        offset, target.id, true, "bomb", 90_000, target.lifeSeq);
      return hits.find(hit => hit.victimId === target.id &&
        hit.victimLifeSeq === target.lifeSeq && hit.lethal === true);
    },
    successor: target => waitForPresentedLiveBotSuccessor({
      actor, playerId: target.id, predecessorLifeSeq: target.lifeSeq,
      label: "Chrome counterfire hostile successor",
    }),
  });
  preparation.assignment = await alignCounterfireAssignment({
    read: async () => (await actor.metricsState()).uiMetrics || {}, server,
    botIds,
    desiredBotId: assignedShooterId, waitUntil,
    changeShip: (before, ship, boundedWait) => selectPlayerShip({
      read: async () => (await actor.metricsState()).uiMetrics || {},
      press: (key, ms) => actor.press(key, ms), waitUntil: boundedWait,
      ship, fromShip: before.playerShip, label: "Chrome controlled counterfire assignment",
    }),
  });
  reportStage("counterfire_hostile_source", preparation);
  const deadline = Date.now() + 60_000;
  const samples = [];
  while (Date.now() < deadline) {
    const lethal = botDamageEvents(server, playerId, damageOffset).find((event) =>
      event.lethal === true && event.shooterId === assignedShooterId &&
      event.shooterLifeSeq === preparation.target.lifeSeq
    );
    if (lethal) return { lethal, samples, preparation };

    const metrics = (await actor.metricsState()).uiMetrics || {};
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) {
      await sleep(100);
      continue;
    }
    const target = botMetrics(metrics)
      .filter((bot) => Number.isFinite(bot.positionX) && Number.isFinite(bot.positionY))
      .find((bot) => bot.id === assignedShooterId);
    if (!target || !Number.isFinite(metrics.playerX) || !Number.isFinite(metrics.playerY)) {
      await sleep(100);
      continue;
    }
    const distance = Math.hypot(
      target.positionX - metrics.playerX,
      target.positionY - metrics.playerY,
    );
    const lineIsClear = authoredLineIsClear(
      metrics.playerX,
      metrics.playerY,
      target.positionX,
      target.positionY,
    );
    if (samples.length < 64) {
      samples.push({
        at: Date.now(),
        playerLifeSeq: metrics.playerLifeSeq,
        playerSpeed: Math.hypot(
          Number(metrics.playerVelocityX) || 0,
          Number(metrics.playerVelocityY) || 0,
        ),
        inSafeZone: metrics.inSafeZone,
        botId: target.id,
        botLifeSeq: target.lifeSeq,
        distance,
        lineIsClear,
      });
    }
    if (metrics.inSafeZone !== true && lineIsClear && distance <= botCounterfireStagingDistance) {
      // Present a stable, ordinary hostile target. No authority is edited:
      // the bot still owns aim, fire, projectile flight, damage, and death.
      // The shared controller's four-second physical bound covers a complete
      // Extreme reversal plus counter-thrust from authored top speed. Retain
      // pursuing only when that real-key maneuver has not yet completed.
      const braking = await brakeClientVelocity(actor);
      if (!braking.braked) {
        if (samples.length < 64) {
          samples.push({
            at: Date.now(),
            phase: "counterfire_brake_incomplete",
            reason: braking.reason,
            speed: braking.speed,
          });
        }
        continue;
      }
      await sleep(250);
      continue;
    }
    // The previous fixed 30s passive wait could leave the pilot and a live bot
    // on opposite sides of a Chaos wall forever. Close toward the current
    // authoritative bot position with normal keyboard feedback, stopping at
    // the bot's authored bullet envelope; shared wall collision remains in
    // force for both actors.
    const waypoint = routedCounterfireWaypoint(metrics, target);
    if (waypoint == null) {
      // The bot can be immediately across a one-tile wall. Wait for its
      // authoritative navigation rather than repeatedly steering the browser
      // hull into the same blocked direct line.
      await sleep(100);
      continue;
    }
    await driveClientToWorldTarget(
      actor,
      waypoint,
      1_200,
      false,
      24,
      null,
      250,
      0.8,
      1_200,
    );
  }
  return { lethal: null, samples, preparation };
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript, wasm: wasmPath, clientEntry });
  const serverPort = await freePort();
  const chromePort = await freePort();
  const staticServer = await startStaticServer();
  const joinOptions = agedServerJoinOptions(process.env, arena);
  const botSpawnFixture = createBotSpawnFixture(arena, process.env.ZUBSPACE_E2E_BOT_SPAWN_FIXTURE || "");
  const server = startRealServer(serverPort, {
    zone: joinOptions.initialZone,
    zoneRoot: botSpawnFixture?.root || null,
    observerNetworkProfile: serverObserverProfile,
    env: { ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: String(loadoutFixture.maxedSpawnLoadout),
      ...(botSpawnFixture ? { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } : {}) },
  });
  const chrome = startChrome(chromePort, `bot-authority-${arena}`);
  let actor = null;
  let actorUrl = null;
  const projectilePresentationStates = [];
  const facingCollectionStates = [];

  try {
    await Promise.all([
      waitForServerListening(serverPort, server.child, server.logs),
      waitForChrome(chrome),
    ]);
    const observedLoadoutFixture = await assertBotAuthorityLoadout(serverPort, loadoutFixture, joinOptions.initialZone);
    const agedJoinEvidence = await waitForAgedServerJoin(server, serverPort, joinOptions);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const networkProfileQuery = networkProfile
      ? `&network_profile=${encodeURIComponent(networkProfile)}&network_profile_seed=${networkProfileSeed}`
      : "";
    const activeProjectileImpairmentQuery = activeProjectileImpairment
      ? "&active_projectile_impairment=armed"
      : "";
    actorUrl = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&packetlog_bytes=1&trace=1&name=botactor${networkProfileQuery}${activeProjectileImpairmentQuery}`;
    actor = await openChromeClient(chrome, actorUrl);
    const actorOpenedAt = Date.now();
    await actor.evaluate(`(() => {
      window.__zubspaceBotAudioProbe = {
        starts: 0,
        runningStarts: 0,
        sourceConnections: 0,
        durations: [],
        errors: [],
      };
      const proto = window.AudioBufferSourceNode?.prototype;
      if (!proto || proto.__zubspaceBotAudioPatched) return true;
      const realStart = proto.start;
      const realConnect = proto.connect;
      proto.start = function(...args) {
        const probe = window.__zubspaceBotAudioProbe;
        probe.starts += 1;
        if (this.context?.state === "running") probe.runningStarts += 1;
        probe.durations.push(this.buffer?.duration ?? null);
        try {
          return realStart.apply(this, args);
        } catch (error) {
          probe.errors.push(String(error));
          throw error;
        }
      };
      proto.connect = function(...args) {
        window.__zubspaceBotAudioProbe.sourceConnections += 1;
        return realConnect.apply(this, args);
      };
      proto.__zubspaceBotAudioPatched = true;
      return true;
    })()`);
    await actor.enterShip();
    // A clean loopback bootstrap normally publishes the complete 2.2 MiB
    // Chaos/Extreme manifest inside this test's 30-second allowance. The
    // pinned WAN profiles deliberately delay every reliable map/LVZ fragment
    // and its application ACK, so retain a profile-sized first-load budget
    // instead of misclassifying honest reliable progress as a gameplay stall.
    // No packet class or product deadline is weakened: the browser still has
    // to publish every authenticated file before this acceptance leg begins.
    const zonePublicationTimeoutMs = networkProfile === "slow"
      ? 420_000
      : networkProfile === "mild"
        ? 240_000
        : 30_000;
    await waitUntil("bot actor authoritative lifecycle ready", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const local = metrics.localGameplayState || {};
      return metrics.playerAlive === true &&
        metrics.serverPlayerAlive === true &&
        metrics.playerLifeSeq != null &&
        local.zoneFilesReady === true &&
        local.zoneMapReady === true &&
        local.doorClockFresh === true &&
        local.authorityReadinessSecurity === true &&
        local.authorityPhase === "Live" &&
        local.pendingShipResetPosition === false &&
        local.pendingShipResetDeliveryId == null;
    }, zonePublicationTimeoutMs);
    const combatShipEvidence = arena === "extreme" ? await selectPlayerShip({
      read: async () => (await actor.metricsState()).uiMetrics || {},
      press: (key, ms) => actor.press(key, ms), waitUntil, ship: 6,
      label: "Extreme Chrome combat Lancaster",
    }) : null;
    const readinessState = await actor.metricsState();
    const initialReadinessEvidence = {
      elapsedMs: Date.now() - actorOpenedAt,
      timeoutMs: zonePublicationTimeoutMs,
      playerLifeSeq: readinessState.uiMetrics?.playerLifeSeq ?? null,
      playerRespawnSeq: readinessState.uiMetrics?.playerRespawnSeq ?? null,
      selfLosses: readinessState.uiMetrics?.selfLosses ?? null,
      zoneMapReady: readinessState.uiMetrics?.localGameplayState?.zoneMapReady ?? null,
      zoneFilesReady: readinessState.uiMetrics?.localGameplayState?.zoneFilesReady ?? null,
      doorClockFresh:
        readinessState.uiMetrics?.localGameplayState?.doorClockFresh ?? null,
      authorityReadinessSecurity:
        readinessState.uiMetrics?.localGameplayState?.authorityReadinessSecurity ?? null,
      authorityPhase:
        readinessState.uiMetrics?.localGameplayState?.authorityPhase ?? null,
      pendingFileRequests:
        readinessState.uiMetrics?.localGameplayState?.pendingFileRequests ?? null,
      pendingZoneDownloads:
        readinessState.uiMetrics?.localGameplayState?.pendingZoneDownloads ?? null,
    };

    const initial = await actor.metricsState();
    const playerId = initial.uiMetrics?.selfPlayerId;
    if (!Number.isInteger(playerId)) throw new Error("bot replay has no assigned player id");
    const initialRespawnSeq = initial.uiMetrics?.playerRespawnSeq ?? 0;
    // Random joins can begin outside published/radar membership. Establish
    // those real witnesses first; the original20sec assertion remains below.
    // The later sprite stage separately retains its300px encounter geometry.
    const initialReadinessEncounter = arena === "chaos-zone" && expectedBotCount > 0
      ? await approachLiveBotEncounter({
        actor, server, botIds: Array.from({ length: expectedBotCount }, (_, index) => serverBotIdBase + index),
        requireEveryBot: true, allowSafeSource: true,
        readiness: "radar",
        label: "Chaos Chrome initial radar readiness",
      }) : null;
    const observedBots = new Map();
    const observedSnapshotBots = new Set();
    const observedRadarBots = new Set();
    await waitUntil("configured bots enter authoritative AOI and radar presentation", async () => {
      const state = await actor.metricsState();
      for (const bot of botMetrics(state.uiMetrics)) {
        if (Number.isInteger(bot.lifeSeq) && Number.isFinite(bot.energy) && bot.energy > 0) {
          observedBots.set(bot.id, { lifeSeq: bot.lifeSeq, energy: bot.energy });
        }
      }
      for (const actor of state.uiMetrics?.authoritativeSnapshotActors || []) {
        if (actor.id >= serverBotIdBase && Number.isInteger(actor.lifeSeq)) {
          observedSnapshotBots.add(actor.id);
        }
      }
      for (const id of state.uiMetrics?.radarPlayerIds || []) {
        if (id >= serverBotIdBase) observedRadarBots.add(id);
      }
      return observedBots.size >= expectedBotCount &&
        observedSnapshotBots.size >= expectedBotCount &&
        observedRadarBots.size >= expectedBotCount;
    }, 20_000);
    const observedBotIds = [...observedBots.keys()].sort((left, right) => left - right);
    await installBotMotionCapture(actor, observedBotIds);
    actor.botRespawnSearchPoint = botSpawnFixture ? { x: 8200, y: 8200 } : null;
    actor.prepareBotObservationOrigin = botSpawnFixture ? async (deadline) => {
      const m = (await actor.metricsState()).uiMetrics;
      return prepareKeyboardLifecycleOrigin({ actor, server, playerId: m.selfPlayerId,
        initialLifeSeq: m.playerLifeSeq, geometry: { authoredHullIsTraversable }, deadline });
    } : null;
    actor.presentationState = (startedAt = 0, includePacketAudit = false) =>
      captureBotPresentationState(actor, observedBotIds, startedAt, includePacketAudit);
    const feelEncounter = arena === "chaos-zone" ? await approachLiveBotEncounter({
      actor, server, botIds: observedBotIds, requireEveryBot: true, allowSafeSource: true,
      label: "Chaos Chrome sprite observation",
    }) : null;
    const botFeelTraceStartedAt = Date.now();
    await sleep(arena === "extreme" ? 18_000 : 8_000);
    const botFeelState = await actor.presentationState(botFeelTraceStartedAt, true);
    const botFeelEvidence = assertBotFeelTrace(
      botFeelState.metricsTrace,
      observedBotIds,
      botFeelTraceStartedAt,
      arena === "extreme" ? 17_000 : 7_000,
      `${arena} Chrome bot feel`,
      botDoorRelocationEvidence(server.logs),
    );
    const botSpriteEvidence = assertBotSpriteTrace(
      botFeelState.remoteSpriteTrace,
      observedBotIds,
      botFeelTraceStartedAt,
      arena === "extreme" ? 17_000 : 7_000,
      `${arena} Chrome bot sprite`,
    );
    // Preserve bounded presentation windows at the points where bot fire is
    // actually exercised. The in-browser traces are intentionally bounded;
    // a long human combat/respawn cycle can otherwise evict the earlier
    // projectile/owner frames before the final audit runs.
    projectilePresentationStates.push(botFeelState);
    const snapshotOnlyPresentation = assertSnapshotOnlyBotPresentation(
      botFeelState,
      `${arena} Chrome bot presentation`,
    );
    const botRepelEvidence = arena === "chaos-zone"
      ? await proveAuthoritativeBotRepel({
          actor,
          server,
          playerId,
          botIds: observedBotIds,
          positionIsOnAuthoredSafeTile,
          authoredHullIsOpen,
          label: "Chaos Chrome",
        })
      : null;
    if (botRepelEvidence != null) {
      projectilePresentationStates.push(await actor.presentationState());
    }
    const motionSoakEvidence = process.env.ZUBSPACE_E2E_MOTION_SOAK_MS
      ? await proveRemoteMotionSoak(actor, observedBotIds,
          Number(process.env.ZUBSPACE_E2E_MOTION_SOAK_MS), reportStage,
          { server, turnRight: () => actor.press("ArrowRight", 150) })
      : null;
    await actor.keyDown("Space");
    let scoreboardEvidence = null;
    await waitUntil("held-Space player list contains every configured server bot", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const botIds = (metrics.scoreboardPlayerIds || [])
        .filter((id) => id >= serverBotIdBase)
        .sort((left, right) => left - right);
      scoreboardEvidence = {
        visible: metrics.scoreboardVisible === true,
        botIds,
      };
      return scoreboardEvidence.visible &&
        observedBotIds.every((id) => botIds.includes(id));
    }, 5_000);
    await actor.keyUp("Space");
    await waitUntil("held-Space player list closes before combat", async () =>
      (await actor.metricsState()).uiMetrics?.scoreboardVisible === false,
    5_000);
    const beforeCombat = await actor.metricsState();
    const objectiveEvidenceBefore = assertObjectivesStayDisabled(beforeCombat, "pre-combat");
    const winsBeforeCombat = beforeCombat.uiMetrics?.selfWins ?? 0;
    const damageOffset = serverDamageBoundary(server);

    // Checked-in arenas spawn the player on safety. The attack loop observes a
    // rendered bot and closes distance with ordinary keyboard movement/fire;
    // it never mutates gameplay or packet state, and safe fire stays rejected.
    // Chaos retains two standalone lethal weapon cycles. Extreme matches the
    // explicit acceptance contract used by the Firefox leg: one exact bot
    // life must first expose a browser-visible bullet decrement, then a bomb
    // must own the lethal terminal. If the real opening bullet burst already
    // kills that life, preserve the stronger standalone cycle and target its
    // successor. Bounded restaging changes only genuine keyboard geometry;
    // no packet or gameplay state is injected.
    let bulletHits = [];
    let bulletLethalHit = null;
    let bulletDecrement = null;
    const extremeBulletAttempts = [];
    const bulletProjectileOffset = parseServerProjectileEvents(server.logs).length;
    if (arena === "extreme") {
      const protectedBulletTarget = botMetrics(
        (await actor.metricsState()).uiMetrics || {},
      ).find((bot) => bot.id === serverBotIdBase && Number.isInteger(bot.lifeSeq));
      if (protectedBulletTarget == null) {
        throw new Error("Extreme Chrome bullet pursuit has no exact server-bot life");
      }
      const pursuitStartedAt = Date.now();
      await attackBotsWithRealKeyboard(
        actor,
        server,
        playerId,
        damageOffset,
        serverBotIdBase,
        false,
        "bullet",
        120_000,
        protectedBulletTarget.lifeSeq,
      );
      bulletHits = playerWeaponDamageToBots(
        server,
        playerId,
        damageOffset,
        bulletWeaponTypes,
      );
      bulletLethalHit = lethalHitWithNonlethalPrecursor(
        bulletHits,
        serverBotIdBase,
      );
      bulletDecrement = bulletHits.find((event) =>
        event.victimId === serverBotIdBase &&
        event.victimLifeSeq === protectedBulletTarget.lifeSeq &&
        event.lethal !== true && event.energyAfter < event.energyBefore
      ) || null;
      extremeBulletAttempts.push({
        mode: "genuine_keyboard_exact_life_pursuit",
        victimId: serverBotIdBase,
        victimLifeSeq: protectedBulletTarget.lifeSeq,
        elapsedMs: Date.now() - pursuitStartedAt,
        hitCount: bulletHits.length,
      });
    } else {
      await attackBotsWithRealKeyboard(
        actor,
        server,
        playerId,
        damageOffset,
        null,
        true,
        "bullet",
        120_000,
      );
      bulletHits = playerWeaponDamageToBots(
        server,
        playerId,
        damageOffset,
        bulletWeaponTypes,
      );
      bulletLethalHit = lethalHitWithNonlethalPrecursor(bulletHits);
      bulletDecrement = bulletHits.find((event) =>
        event.lethal !== true && event.energyAfter < event.energyBefore
      ) || null;
    }
    if (bulletDecrement == null) {
      throw new Error(`Chrome human bullet fire produced no visible authoritative bot-energy decrement: ${JSON.stringify({
        extremeBulletAttempts,
        bulletHits,
      })}`);
    }
    await waitUntil("Chrome browser-owned bullet tail reaches hit or authored expiry", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return ownedProjectileTailSettled({
        projectileEvents: parseServerProjectileEvents(server.logs)
          .slice(bulletProjectileOffset),
        ownerId: playerId,
        latestServerTick: metrics.latestServerTick,
        weaponTypes: bulletWeaponTypes,
      });
    }, 10_000);
    // A real-key burst can return after its first visible decrement while
    // accepted rounds from that same burst are still in flight. Classify the
    // exact bot life only after every server-owned round has hit or reached
    // its authored expiry; otherwise the bomb phase can retain a predecessor
    // life that the remaining bullets kill asynchronously.
    bulletHits = playerWeaponDamageToBots(
      server,
      playerId,
      damageOffset,
      bulletWeaponTypes,
    );
    bulletLethalHit = lethalHitWithNonlethalPrecursor(
      bulletHits,
      arena === "extreme" ? serverBotIdBase : null,
    );
    bulletDecrement = bulletHits.find((event) =>
      (arena !== "extreme" || event.victimId === serverBotIdBase) &&
      event.lethal !== true && event.energyAfter < event.energyBefore
    ) || null;
    if (bulletDecrement == null) {
      throw new Error("Chrome settled bullet tail lost its visible authoritative bot-energy decrement");
    }
    if (arena !== "extreme" && bulletLethalHit == null) {
      throw new Error("Chrome human bullet fire did not visibly damage then lethally kill one bot life");
    }
    const bulletCycle = bulletLethalHit != null
      ? await provePresentedBotCombatCycle({
          actor,
          server,
          playerId,
          playerName: new URL(actorUrl).searchParams.get("name"),
          hits: bulletHits,
          lethalHit: bulletLethalHit,
          winsBefore: winsBeforeCombat,
          label: `${arena} Chrome bullet cycle`,
          presentationBefore: beforeCombat.uiMetrics || {},
        })
      : {
          label: `${arena} Chrome bullet decrement before combined bomb cycle`,
          victimId: bulletDecrement.victimId,
          victimLifeSeq: bulletDecrement.victimLifeSeq,
          nextLifeSeq: null,
          winsBefore: winsBeforeCombat,
          winsAfter: winsBeforeCombat,
          damagePresentation: await provePostRespawnBotDamage({
            actor,
            hits: bulletHits,
            playerId,
            victimId: bulletDecrement.victimId,
            victimLifeSeq: bulletDecrement.victimLifeSeq,
            label: `${arena} Chrome bullet decrement before combined bomb cycle`,
          }),
          combinedBombCycle: true,
        };

    const chaosBombActor = arena === "extreme"
      ? null
      : botMetrics((await actor.metricsState()).uiMetrics || {})
          .find((bot) => bot.id === serverBotIdBase);
    const bombTarget = arena === "extreme"
      ? {
          victimId: bulletCycle.victimId,
          victimLifeSeq: bulletCycle.nextLifeSeq ?? bulletCycle.victimLifeSeq,
        }
      : {
          // Bot 768 owns human pursuit while bot 769 patrols/fights it. Use
          // bot 768 for the distinct bomb-owned life so autonomous bot fire
          // cannot steal the terminal after the browser's nonlethal damage.
          victimId: serverBotIdBase,
          victimLifeSeq: chaosBombActor?.lifeSeq,
        };
    if (!Number.isInteger(bombTarget.victimLifeSeq)) {
      throw new Error(`Chrome bullet cycle did not establish an exact bomb target: ${JSON.stringify(bulletCycle)}`);
    }

    // Lancaster owns the bullet leg; Warbird's authored faster bomb owns
    // the separate bomb leg. Identify ships directly, keeping maximum entry.
    const bombShipEvidence = arena === "extreme" ? await selectPlayerShip({
      read: async () => (await actor.metricsState()).uiMetrics || {},
      press: (key, ms) => actor.press(key, ms), waitUntil, ship: 0, fromShip: 6,
      label: "Extreme Chrome bomb Warbird",
    }) : null;
    if (bombShipEvidence) {
      await waitUntil("Extreme Chrome Warbird bomb clock is ready", async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        return playerShipReady(metrics) && hasPlayerShip(metrics, 0) &&
          metrics.playerLifeSeq === bombShipEvidence.lifeSeq &&
          (metrics.localGameplayState?.bombCooldown ?? 1) <= 0;
      }, 15_000);
    }

    let bombOffset = serverDamageBoundary(server);
    let bombPresentationBefore = (await actor.metricsState()).uiMetrics || {};
    let bombWinsBefore = bulletCycle.winsAfter;
    // Bind browser combat to the exact stable bot id and life established by
    // the bullet evidence. Bot 768 is the deterministic hostile pursuer; any
    // additional bots configured by standalone presentation/crossfire runs do
    // not make target selection stochastic.
    const preferredBombVictimId = bombTarget.victimId;
    let bombHits = [];
    let bombLethalHit = null;
    const bombAttempts = [];
    if (arena === "extreme") {
      // The protected safe edge proves that ordinary Extreme bullet damage
      // crosses the authored boundary. A bomb is a moving-target combat
      // action, not a stationary geometry oracle: leave safety and pursue the
      // exact retained bot life with genuine keyboard steering and Z edges.
      // The helper is life-bound, and the assertions below still require the
      // matching server-owned projectile, lethal damage, score, presentation,
      // death, and replacement-life respawn.
      const pursuitStartedAt = Date.now();
      await attackBotsWithRealKeyboard(
        actor,
        server,
        playerId,
        bombOffset,
        preferredBombVictimId,
        true,
        "bomb",
        30_000,
        bombTarget.victimLifeSeq,
      );
      bombHits = playerWeaponDamageToBots(
        server,
        playerId,
        bombOffset,
        bombWeaponTypes,
      );
      bombLethalHit = bombHits.find((event) =>
        event.lethal === true &&
        event.victimId === bombTarget.victimId &&
        event.victimLifeSeq === bombTarget.victimLifeSeq
      ) || null;
      bombAttempts.push({
        mode: "genuine_keyboard_existing_encounter",
        victimId: preferredBombVictimId,
        victimLifeSeq: bombTarget.victimLifeSeq,
        elapsedMs: Date.now() - pursuitStartedAt,
        hits: bombHits,
      });
      for (let attempt = 0; attempt < 6 && bombLethalHit == null; attempt += 1) {
        const attemptOffset = serverDamageBoundary(server);
        const staging = await stageExtremeChromeBombEncounter(
          actor,
          preferredBombVictimId,
        );
        bombAttempts.push({
          mode: "genuine_keyboard_safe_edge_then_exact_life_pursuit",
          attempt,
          victimId: preferredBombVictimId,
          victimLifeSeq: bombTarget.victimLifeSeq,
          staged: staging.staged !== false,
          diagnostics: staging.diagnostics.slice(-12),
          elapsedMs: Date.now() - pursuitStartedAt,
        });
        reportStage("extreme_bomb_staging_attempt", bombAttempts.at(-1));
        if (staging.staged === false) continue;
        bombHits = playerWeaponDamageToBots(
          server,
          playerId,
          bombOffset,
          bombWeaponTypes,
        );
        bombLethalHit = bombHits.find((event) =>
          event.lethal === true &&
          event.victimId === bombTarget.victimId &&
          event.victimLifeSeq === bombTarget.victimLifeSeq
        ) || null;
        if (bombLethalHit != null) break;
        // A correctly staged Z edge can own the terminal itself. The short
        // pursuit window observes that in-flight result and permits another
        // ordinary keyboard edge, but never replaces the exact server-owned
        // damage/death assertion below.
        await attackBotsWithRealKeyboard(
          actor,
          server,
          playerId,
          attemptOffset,
          preferredBombVictimId,
          true,
          "bomb",
          20_000,
          bombTarget.victimLifeSeq,
        );
        bombHits = playerWeaponDamageToBots(
          server,
          playerId,
          bombOffset,
          bombWeaponTypes,
        );
        bombLethalHit = bombHits.find((event) =>
          event.lethal === true &&
          event.victimId === bombTarget.victimId &&
          event.victimLifeSeq === bombTarget.victimLifeSeq
        ) || null;
      }
    } else {
      const deadline = Date.now() + 180_000;
      for (let attempt = 0; attempt < 6 && bombLethalHit == null && Date.now() < deadline; attempt += 1) {
        const renewed = await renewRetiredBombTarget({ actor, server, playerId, target: bombTarget,
          damageBoundary: bombOffset, label: "Chrome primary-bomb retry", timeoutMs: Math.min(10_000, deadline - Date.now()) });
        if (renewed) {
          reportStage("primary_bomb_target_retired", {
            previousLifeSeq: renewed.previousTarget.victimLifeSeq, nextLifeSeq: renewed.target.victimLifeSeq,
            terminalDamageSeq: renewed.retirement.damageSeq, terminalWeapon: renewed.retirement.weapon,
            primaryBombProofStillRequired: true,
          });
          bombAttempts.push({ mode: "retired_exact_life", ...renewed });
          Object.assign(bombTarget, renewed.target);
          bombOffset = renewed.damageBoundary;
          bombPresentationBefore = renewed.presentationBefore;
          bombWinsBefore = renewed.winsBefore;
        }
        await attackBotsWithRealKeyboard(actor, server, playerId, bombOffset,
          preferredBombVictimId, true, "bomb", Math.max(1, deadline - Date.now()), bombTarget.victimLifeSeq);
        bombHits = playerWeaponDamageToBots(server, playerId, bombOffset, bombWeaponTypes);
        bombLethalHit = bombHits.find(event => event.lethal === true &&
          event.victimId === bombTarget.victimId && event.victimLifeSeq === bombTarget.victimLifeSeq) || null;
      }
    }
    if (bombLethalHit == null) {
      throw new Error(`Chrome human bomb fire did not lethally damage a server bot: ${JSON.stringify({
        bombTarget,
        bombAttempts,
        bombHits,
      })}`);
    }
    if (bombLethalHit.victimLifeSeq !== bombTarget.victimLifeSeq) {
      throw new Error(`Chrome bomb target life changed during the bomb-only cycle: ${JSON.stringify({
        bombTarget,
        bombLethalHit,
      })}`);
    }
    const earlyAuthoredBombEvidence = {
      damageSeq: bombLethalHit.damageSeq,
      projectileId: bombLethalHit.projectileId,
      serverTick: bombLethalHit.serverTick,
      victimId: bombLethalHit.victimId,
      victimLifeSeq: bombLethalHit.victimLifeSeq,
      weapon: bombLethalHit.weapon,
      damage: bombLethalHit.damage,
      energyBefore: bombLethalHit.energyBefore,
      energyAfter: bombLethalHit.energyAfter,
      lethal: bombLethalHit.lethal,
    };
    const bombCycle = await provePresentedBotCombatCycle({
      actor,
      server,
      playerId,
      playerName: new URL(actorUrl).searchParams.get("name"),
      hits: bombHits,
      lethalHit: bombLethalHit,
      winsBefore: bombWinsBefore,
      label: `${arena} Chrome bomb cycle`,
      presentationBefore: bombPresentationBefore,
      // The bullet cycle above owns the visible decrement assertion. A fresh
      // bot life can have less energy than one authored bomb, so this distinct
      // bomb cycle proves exact-life bomb ownership and terminal presentation.
      requireNonlethal: false,
    });
    const postRespawnDamageOffset = serverDamageBoundary(server);
    const postRespawnProjectileOffset = parseServerProjectileEvents(server.logs).length;
    const postRespawnPresentationBefore = (await actor.metricsState()).uiMetrics || {};
    let postRespawnHits = [];
    const postRespawnAttempts = [];
    if (arena === "extreme") {
      const projectileOffset = parseServerProjectileEvents(server.logs).length;
      const pursuitStartedAt = Date.now();
      await attackBotsWithRealKeyboard(
        actor,
        server,
        playerId,
        postRespawnDamageOffset,
        bombCycle.victimId,
        false,
        "bullet",
        90_000,
        bombCycle.nextLifeSeq,
      );
      await waitUntil("Extreme Chrome post-respawn bullet tail settles", async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        return ownedProjectileTailSettled({
          projectileEvents: parseServerProjectileEvents(server.logs).slice(projectileOffset),
          ownerId: playerId,
          latestServerTick: metrics.latestServerTick,
          weaponTypes: bulletWeaponTypes,
        });
      }, 10_000);
      postRespawnHits = playerWeaponDamageToBots(
        server,
        playerId,
        postRespawnDamageOffset,
        bulletWeaponTypes,
      );
      postRespawnAttempts.push({
        mode: "genuine_keyboard_exact_life_pursuit",
        victimId: bombCycle.victimId,
        victimLifeSeq: bombCycle.nextLifeSeq,
        elapsedMs: Date.now() - pursuitStartedAt,
        hits: postRespawnHits,
      });
    } else {
      await attackBotsWithRealKeyboard(
        actor,
        server,
        playerId,
        postRespawnDamageOffset,
        bombCycle.victimId,
        false,
        "bullet",
        90_000,
      );
      postRespawnHits = playerWeaponDamageToBots(
        server,
        playerId,
        postRespawnDamageOffset,
        bulletWeaponTypes,
      );
    }
    const postRespawnOwnedProjectiles = parseServerProjectileEvents(server.logs)
      .slice(postRespawnProjectileOffset)
      .filter((event) =>
        event.type === "server_projectile_spawned" &&
        event.ownerId === playerId &&
        bulletWeaponTypes.has(event.weapon & 0x1f)
      );
    if (postRespawnOwnedProjectiles.length === 0) {
      throw new Error("Chrome post-respawn keyboard fire produced no authoritative projectile");
    }
    await waitUntil("Chrome post-respawn bullet tail reaches hit or authored expiry", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return ownedProjectileTailSettled({
        projectileEvents: parseServerProjectileEvents(server.logs)
          .slice(postRespawnProjectileOffset),
        ownerId: playerId,
        latestServerTick: metrics.latestServerTick,
        weaponTypes: bulletWeaponTypes,
      });
    }, 10_000);
    postRespawnHits = playerWeaponDamageToBots(
      server,
      playerId,
      postRespawnDamageOffset,
      bulletWeaponTypes,
    );
    const postRespawnBotDamage = await provePostRespawnBotDamage({
      actor,
      hits: postRespawnHits,
      playerId,
      victimId: bombCycle.victimId,
      victimLifeSeq: bombCycle.nextLifeSeq,
      label: `${arena} Chrome post-respawn bullet`,
    });
    postRespawnBotDamage.attempts = postRespawnAttempts;
    const repeatedBulletLethalHit = arena === "extreme"
      ? lethalHitWithNonlethalPrecursor(postRespawnHits, bombCycle.victimId)
      : null;
    const repeatedBulletCycle = repeatedBulletLethalHit == null
      ? null
      : await provePresentedBotCombatCycle({
          actor,
          server,
          playerId,
          playerName: new URL(actorUrl).searchParams.get("name"),
          hits: postRespawnHits,
          lethalHit: repeatedBulletLethalHit,
          winsBefore: bombCycle.winsAfter,
          label: "extreme Chrome repeated bullet-owned cycle",
          presentationBefore: postRespawnPresentationBefore,
        });
    let repeatedCombatCycle = null;
    let lateAuthoredBombEvidence = null;
    const lateAuthoredBombAttempts = [];
    if (arena === "extreme") {
      const repeatedTarget = {
        victimId: bombCycle.victimId,
        // A genuine Extreme multifire edge can itself finish the successor
        // life after first proving its visible decrement. That is a stronger
        // complete bullet cycle; bind the bomb to the exact server-authored
        // successor instead of continuing to target the retired life.
        victimLifeSeq: repeatedBulletCycle?.nextLifeSeq ??
          postRespawnBotDamage.event.victimLifeSeq,
      };
      const repeatedOffset = serverDamageBoundary(server);
      const repeatedPresentationBefore = (await actor.metricsState()).uiMetrics || {};
      const repeatedAttempts = [];
      let repeatedHits = [];
      let repeatedLethalHit = null;
      const projectileOffset = parseServerProjectileEvents(server.logs).length;
      const pursuitStartedAt = Date.now();
      await attackBotsWithRealKeyboard(
        actor,
        server,
        playerId,
        repeatedOffset,
        repeatedTarget.victimId,
        true,
        "bomb",
        30_000,
        repeatedTarget.victimLifeSeq,
      );
      repeatedHits = playerWeaponDamageToBots(
        server,
        playerId,
        repeatedOffset,
        bombWeaponTypes,
      );
      repeatedLethalHit = repeatedHits.find((event) =>
        event.lethal === true &&
        event.victimId === repeatedTarget.victimId &&
        event.victimLifeSeq === repeatedTarget.victimLifeSeq
      ) || null;
      repeatedAttempts.push({
        mode: "genuine_keyboard_existing_encounter",
        victimId: repeatedTarget.victimId,
        victimLifeSeq: repeatedTarget.victimLifeSeq,
        elapsedMs: Date.now() - pursuitStartedAt,
        hits: repeatedHits,
      });
      for (let attempt = 0; attempt < 6 && repeatedLethalHit == null; attempt += 1) {
        const attemptOffset = serverDamageBoundary(server);
        // A hostile life parked on an authored safe tile is unbombable, and a pilot holding the
        // central reserve is out of range of it, which is what produced the `distance` aim
        // rejections. Bait it off safety and retreat into the nearest safety entry first, exactly
        // as the Firefox twin does.
        const lured = await lureExtremeChromeTargetOutOfSafety(
          actor,
          repeatedTarget.victimId,
          repeatedTarget.victimLifeSeq,
        );
        const staging = lured
          ? await stageExtremeChromeBombEncounter(actor, repeatedTarget.victimId)
          : { staged: false, diagnostics: [{ phase: "lure_target_out_of_safety_failed" }] };
        repeatedAttempts.push({
          mode: "genuine_keyboard_safe_edge_then_exact_life_pursuit",
          attempt,
          victimId: repeatedTarget.victimId,
          victimLifeSeq: repeatedTarget.victimLifeSeq,
          lured,
          staged: staging.staged !== false,
          // An unstaged attempt still gets the genuine-key pursuit pass below: the safe-edge solve
          // only fires when the target is already inside the 340px direct-bomb corridor, and a
          // lured pursuer can hold just outside it (retained: `{"reason": "distance", "distance":
          // 348.48, "maximumDistance": 340}`). Closing that gap is what real pursuit input does, and
          // the unchanged exact-life lethal-damage boundary decides the attempt either way.
          pursued: true,
          diagnostics: staging.diagnostics.slice(-12),
          elapsedMs: Date.now() - pursuitStartedAt,
        });
        reportStage("extreme_repeated_bomb_staging_attempt", repeatedAttempts.at(-1));
        repeatedHits = playerWeaponDamageToBots(
          server,
          playerId,
          repeatedOffset,
          bombWeaponTypes,
        );
        repeatedLethalHit = repeatedHits.find((event) =>
          event.lethal === true &&
          event.victimId === repeatedTarget.victimId &&
          event.victimLifeSeq === repeatedTarget.victimLifeSeq
        ) || null;
        if (repeatedLethalHit != null) break;
        await attackBotsWithRealKeyboard(
          actor,
          server,
          playerId,
          attemptOffset,
          repeatedTarget.victimId,
          true,
          "bomb",
          20_000,
          repeatedTarget.victimLifeSeq,
        );
        repeatedHits = playerWeaponDamageToBots(
          server,
          playerId,
          repeatedOffset,
          bombWeaponTypes,
        );
        repeatedLethalHit = repeatedHits.find((event) =>
          event.lethal === true &&
          event.victimId === repeatedTarget.victimId &&
          event.victimLifeSeq === repeatedTarget.victimLifeSeq
        ) || null;
      }
      repeatedAttempts.push({
        mode: "exact_server_terminal_summary",
        victimId: repeatedTarget.victimId,
        victimLifeSeq: repeatedTarget.victimLifeSeq,
        elapsedMs: Date.now() - pursuitStartedAt,
        projectileEvents: parseServerProjectileEvents(server.logs)
          .slice(projectileOffset)
          .filter((event) =>
            event.ownerId === playerId && bombWeaponTypes.has(event.weapon & 0x1f)
          ),
        hits: repeatedHits,
      });
      if (repeatedLethalHit == null) {
        throw new Error(`repeated Extreme Chrome bomb cycle did not lethally damage the exact post-respawn bot life: ${JSON.stringify({
          repeatedTarget,
          repeatedAttempts,
          repeatedHits,
        })}`);
      }
      repeatedCombatCycle = await provePresentedBotCombatCycle({
        actor,
        server,
        playerId,
        playerName: new URL(actorUrl).searchParams.get("name"),
        hits: repeatedHits,
        lethalHit: repeatedLethalHit,
        winsBefore: repeatedBulletCycle?.winsAfter ?? bombCycle.winsAfter,
        label: repeatedBulletCycle == null
          ? "extreme Chrome repeated bullet-decrement/bomb-death cycle"
          : "extreme Chrome bomb cycle after repeated bullet-owned death",
        presentationBefore: repeatedPresentationBefore,
        requireNonlethal: false,
      });
      repeatedCombatCycle.attempts = repeatedAttempts;
      repeatedCombatCycle.priorBulletDamage = postRespawnBotDamage;
      repeatedCombatCycle.priorBulletCycle = repeatedBulletCycle;
      lateAuthoredBombEvidence = repeatedLethalHit;
    } else {
      // Re-run the same genuine Z-key path against a later authoritative bot
      // life. The LAN report was time-dependent, so one lucky opening
      // explosion is insufficient: this later phase must still produce an
      // exact type-4 damage fact after death/respawn and ordinary bullet play.
      for (let attempt = 0; attempt < 4 && lateAuthoredBombEvidence == null; attempt += 1) {
        // The preceding real-key bullet phase may itself kill the first
        // post-bomb life. Synchronize on the browser admitting an exact live
        // successor instead of polling for less than the authored respawn
        // deadline and then reporting a false combat failure.
        const currentTarget = await waitForPresentedLiveBotSuccessor({
          actor,
          playerId: bombCycle.victimId,
          predecessorLifeSeq: bombCycle.victimLifeSeq,
          label: `late Chaos Chrome proximity-bomb attempt ${attempt}`,
        });
        const attemptOffset = serverDamageBoundary(server);
        // This leg requires a fresh later-life bomb hit. The distinct
        // bomb-owned death is proved above; do not chase another unnecessary
        // lethal while the required nonlethal transaction is already present.
        await attackBotsWithRealKeyboard(
          actor,
          server,
          playerId,
          attemptOffset,
          currentTarget.id,
          false,
          "bomb",
          90_000,
          currentTarget.lifeSeq,
        );
        const attemptHits = playerWeaponDamageToBots(
          server,
          playerId,
          attemptOffset,
          bombWeaponTypes,
        );
        lateAuthoredBombEvidence = attemptHits.find((event) =>
          event.victimId === currentTarget.id &&
          event.victimLifeSeq === currentTarget.lifeSeq &&
          event.damage > 0 && event.energyAfter < event.energyBefore
        ) || null;
        lateAuthoredBombAttempts.push({
          attempt,
          victimId: currentTarget.id,
          victimLifeSeq: currentTarget.lifeSeq,
          hits: attemptHits,
        });
      }
      if (lateAuthoredBombEvidence == null) {
        throw new Error(`late Chaos Chrome proximity-bomb cycle produced no authoritative bot damage: ${JSON.stringify({
          earlyAuthoredBombEvidence,
          lateAuthoredBombAttempts,
        })}`);
      }
    }
    if (
      lateAuthoredBombEvidence.projectileId === earlyAuthoredBombEvidence.projectileId ||
      !u32IsStrictlyNewer(
        lateAuthoredBombEvidence.victimLifeSeq,
        earlyAuthoredBombEvidence.victimLifeSeq,
      )
    ) {
      throw new Error(`Chrome authored-bomb replay did not cross an exact bot life: ${JSON.stringify({
        earlyAuthoredBombEvidence,
        lateAuthoredBombEvidence,
      })}`);
    }
    const stationaryBotEvidence = arena === "extreme"
      ? stationaryBotMotionEvidence(server.logs, {
          botId: bombCycle.victimId,
          requiredLifeSeqs: requiredStationaryFixtureLifeSeqs(
            [
              earlyAuthoredBombEvidence.victimLifeSeq,
              postRespawnBotDamage.event.victimLifeSeq,
              lateAuthoredBombEvidence.victimLifeSeq,
            ],
            stationaryBotFromLifeSeq,
            stationaryBotStaticLifeSeqs,
          ),
        })
      : null;
    const movingBombTimelineEvidence = arena === "chaos-zone"
      ? correlateMovingBombPresentation(
          server,
          {
            damagePresentationTrace: await actor.evaluate(
              "window.__zubspaceDamagePresentationTrace || []",
            ),
          },
          playerId,
          damageOffset,
        )
      : null;
    const passiveBotProjectileSpawns = passiveBotLifeSeqs.length === 0
      ? []
      : parseServerProjectileEvents(server.logs).filter((event) =>
          event.type === "server_projectile_spawned" &&
          event.ownerId === bombCycle.victimId &&
          passiveBotLifeSeqs.includes(event.ownerLifeSeq)
        );
    if (passiveBotProjectileSpawns.length !== 0) {
      throw new Error(`passive Chrome collision life emitted a projectile: ${JSON.stringify({
        passiveBotLifeSeq,
        passiveBotLifeSeqs,
        passiveBotProjectileSpawns,
      })}`);
    }
    const passiveFireEvidence = passiveBotLifeSeqs.length === 0
      ? null
      : {
          lifeSeq: passiveBotLifeSeq,
          lifeSeqs: passiveBotLifeSeqs,
          projectileSpawns: passiveBotProjectileSpawns.length,
        };
    // The two moving collision sources can both still be passive here.
    // Retire their exact controlled lives with ordinary keyboard combat
    // before waiting for incoming fire; waiting cannot create a hostile life.
    // Plain bullets retire these setup lives; the measured moving-bomb
    // collision proofs above have already completed and remain unchanged.
    const incomingSourcePreparation = arena === "chaos-zone" && expectedBotCount === 2 &&
      passiveBotLifeSeqs.join(",") === "1,2"
      ? await prepareBotFacingSources({
          botIds: observedBotIds, passiveLifeSeqs: passiveBotLifeSeqs,
          readTarget: (id, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: id, predecessorLifeSeq: 0,
            label: "Chrome incoming-fire source", timeoutMs,
          }),
          retire: async (target, timeoutMs) => {
            const offset = serverDamageBoundary(server);
            const hits = await attackBotsWithRealKeyboard(actor, server, playerId,
              offset, target.id, true, "bullet", timeoutMs, target.lifeSeq);
            return hits.find(hit => hit.victimId === target.id &&
              hit.victimLifeSeq === target.lifeSeq && hit.lethal === true);
          },
          successor: (target, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: target.id, predecessorLifeSeq: target.lifeSeq,
            label: "Chrome incoming-fire hostile successor", timeoutMs,
          }),
        })
      : null;
    reportStage("incoming_hostile_sources", incomingSourcePreparation);
    // End the pilot's attack pass with genuine steering/braking input before
    // measuring bot lethality. A fast tangential coast can keep crossing the
    // bot's narrow aim-lock window while Chaos recharge erases sparse hits;
    // the stationary-hostile authority contract is the deterministic combat
    // boundary this leg intends to prove.
    const botLethalBrake = await brakeClientVelocity(actor, 5_000);
    const playerToBot = playerDamageToBots(server, playerId, damageOffset);
    const incomingDamageApproach = botDamageEvents(server, playerId, damageOffset).length === 0
      ? await approachLiveBotEncounter({ actor, server, botIds: observedBotIds,
          positionIsOnAuthoredSafeTile, label: `${arena} Chrome incoming damage observation` })
      : null;
    await waitUntil("server bot weapon damages browser player", async () =>
      botDamageEvents(server, playerId, damageOffset).length > 0,
    30_000);
    let appliedBotHit = null;
    await waitUntil("browser applies bot-owned damage identity", async () => {
      const state = await actor.metricsState();
      const metrics = state.uiMetrics || {};
      appliedBotHit = [...botDamageEvents(server, playerId, damageOffset)]
        .reverse()
        .find((event) =>
          (state.damageTrace || []).some((entry) => damageTraceMatches(entry, event)) ||
          (
            metrics.lastSelfDamageAttackerId === event.shooterId &&
            metrics.lastSelfDamageServerSeq === event.damageSeq &&
            metrics.lastSelfDamageShooterLifeSeq === event.shooterLifeSeq
          )
        ) || null;
      return appliedBotHit !== null;
    }, 10_000);
    projectilePresentationStates.push(await actor.presentationState());
    const botToPlayer = botDamageEvents(server, playerId, damageOffset);

    const counterfireOffset = serverDamageBoundary(server);
    const beforeCounterfire = (await actor.metricsState()).uiMetrics || {};
    const retainedProjectileOwnerEvidence = assertBotProjectileOwnersVisible(
      projectilePresentationStates.flatMap(metricsTraceWithViewport),
      projectilePresentationStates.flatMap((state) => state.remoteSpriteTrace || []),
      observedBotIds,
      `${arena} Chrome retained bot projectile ownership`,
      { required: false },
    );
    // The focused regression must exercise fresh counterfire even if an older
    // causally presented kill happens to survive in the bounded trace window.
    const requireFreshCounterfire = process.env.ZUBSPACE_E2E_REQUIRE_FRESH_COUNTERFIRE === "1";
    const observedLethal = requireFreshCounterfire || retainedProjectileOwnerEvidence == null
      ? null
      : ([...botToPlayer].reverse().find((event) =>
          event.lethal === true &&
          event.shooterId === appliedBotHit.shooterId &&
          Number.isInteger(beforeCounterfire.playerLifeSeq) &&
          u32IsStrictlyNewer(beforeCounterfire.playerLifeSeq, event.victimLifeSeq) &&
          (beforeCounterfire.playerRespawnSeq ?? 0) > initialRespawnSeq
        ) || null);
    // Bot counterfire commonly owns a complete death/respawn while the pilot
    // is staging the human weapon cycles. Preserve that exact server event
    // when the browser is already live in a wrap-newer life; only stage an
    // additional stationary-hostile kill when the scenario has not proved
    // one, but only while its exact on-screen projectile/draw proof is still
    // retained. Server-only damage cannot stand in for the browser
    // presentation requirement, so stage fresh counterfire when that bounded
    // evidence has expired.
    const botCounterfire = observedLethal == null
      ? await stageForLethalBotCounterfire(
          actor,
          server,
          playerId,
          counterfireOffset,
          appliedBotHit.shooterId,
          observedBotIds,
        )
      : { lethal: observedLethal, samples: [], source: "observed_during_combat" };
    if (botCounterfire.lethal == null) {
      throw new Error(`server bot did not lethally damage the staged browser player: ${JSON.stringify(botCounterfire)}`);
    }
    projectilePresentationStates.push(await actor.presentationState());
    await waitUntil("browser applies death and returns under a newer player life", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
        (metrics.playerRespawnSeq ?? 0) > initialRespawnSeq;
    }, 15_000);
    const lethalBotHit = botCounterfire.lethal;

    const finalState = await actor.presentationState();
    projectilePresentationStates.push(finalState);
    const longHorizonBotSmoothness = remoteActorMotionAcrossStatesEvidence(
      projectilePresentationStates,
      observedBotIds,
      { doorRelocations: botDoorRelocationEvidence(server.logs) },
    );
    if (!longHorizonBotSmoothness.ok) {
      throw new Error(`${arena} Chrome bot presentation crossed an unexpected same-life discontinuity: ${JSON.stringify(longHorizonBotSmoothness)}`);
    }
    const audioEvidence = assertBrowserAudio(finalState, `${arena} Chrome bot audio`);
    // The witness binds a launch to the draw at the *launch's own tick*, and a deterministic impairment profile
    // makes the client render a delayed timeline: the launch's server tick is not the presentation render tick, so
    // the exact-tick claim is not decidable and the collector cannot grow its way out of a systematic offset.
    // Retained in the frozen matrix: `bot facing requires three exact-tick launches; matched 0/18` under
    // `ZUBSPACE_E2E_NETWORK_PROFILE=mild` (30 ms one-way), while the same lane's clean cells match every time. The
    // skip is recorded rather than dropped, and the clean cells keep the unmodified witness and its three-launch
    // floor.
    const facingSourcePreparation = networkProfile || passiveBotLifeSeqs.length === 0
      ? null
      : await prepareBotFacingSources({
          botIds: observedBotIds, passiveLifeSeqs: passiveBotLifeSeqs,
          readTarget: (id, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: id, predecessorLifeSeq: 0,
            label: "Chrome facing source", timeoutMs,
          }),
          retire: async (target, timeoutMs) => {
            const offset = serverDamageBoundary(server);
            const hits = await attackBotsWithRealKeyboard(actor, server, playerId,
              offset, target.id, true, "bomb", timeoutMs, target.lifeSeq);
            return hits.find(hit => hit.victimId === target.id &&
              hit.victimLifeSeq === target.lifeSeq && hit.lethal === true);
          },
          successor: (target, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: target.id, predecessorLifeSeq: target.lifeSeq,
            label: "Chrome facing hostile successor", timeoutMs,
          }),
        });
    reportStage("facing_hostile_sources", facingSourcePreparation);
    const facingEncounterPreparation = networkProfile ? null : await approachLiveBotEncounter({
      actor, server, botIds: observedBotIds, positionIsOnAuthoredSafeTile,
      searchPoint: { x: 8_200, y: 8_200 },
      label: `${arena} Chrome facing collection`,
    });
    reportStage("facing_encounter_preparation", facingEncounterPreparation);
    const minimumFacingShotServerTick = requireFacingCollection
      ? (await actor.metricsState()).uiMetrics?.latestSnapshotServerTick
      : null;
    if (requireFacingCollection && !Number.isInteger(minimumFacingShotServerTick))
      throw new Error("fresh facing collection is missing its authoritative tick boundary");
    const facingEvidence = networkProfile
      ? {
          skipped: true,
          reason: `network profile ${networkProfile} delays the presentation timeline, so a launch tick is not a `
            + "presentation render tick and the exact-tick facing witness is not decidable here",
        }
      : await collectBotFacingEvidence({
          states: requireFacingCollection ? [] : projectilePresentationStates,
          minimumShotServerTick: minimumFacingShotServerTick,
          sample: async () => {
            const state = await actor.presentationState();
            facingCollectionStates.push(state);
            return state;
          },
          botIds: observedBotIds,
          timeoutMs: 90_000,
          provoke: (cycle) => provokeChromeBotLaunches(actor, server, observedBotIds, cycle),
        });
    if (facingSourcePreparation != null) facingEvidence.sourcePreparation = facingSourcePreparation;
    if (facingEncounterPreparation != null) facingEvidence.encounterPreparation = facingEncounterPreparation;
    if (requireFacingCollection) facingEvidence.freshCollectionRequired = true;
    const projectileOwnerEvidence = assertBotProjectileOwnersVisible(
      projectilePresentationStates.flatMap(metricsTraceWithViewport),
      projectilePresentationStates.flatMap((state) => state.remoteSpriteTrace || []),
      observedBotIds,
      `${arena} Chrome bot projectile ownership`,
    );
    const objectiveEvidenceAfter = assertObjectivesStayDisabled(finalState, "post-combat");
    const hiddenPrizeEvidence = combatHiddenPrizeEvidence(server);
    const wallCollisionEvidence = botWallCollisionEvidence(server, observedBotIds);
    const combatViewGeometryEvidence = assertHumanProjectilePresentationGeometry(
      server,
      playerDamageToBots(server, playerId, damageOffset),
      `${arena} Chrome human-to-bot combat`,
    );
    if ((finalState.uiMetrics?.localBotCount ?? 0) !== 0) {
      throw new Error(`network client created private local bots: ${finalState.uiMetrics?.localBotCount}`);
    }
    let activeProjectileImpairmentEvidence = null;
    if (activeProjectileImpairment) {
      await waitUntil("active projectile game omission and delayed event recovery", async () => {
        const evidence = await actor.evaluate(
          "window.__zubspaceActiveProjectileImpairmentStateForTest?.() || null",
        );
        return evidence?.gameCopiesDropped > 0 &&
          evidence?.eventCopiesDelayed > 0 &&
          evidence?.eventCopiesReleased > 0;
      }, 10_000);
      activeProjectileImpairmentEvidence = await actor.evaluate(
        "window.__zubspaceActiveProjectileImpairmentStateForTest?.() || null",
      );
      if (
        activeProjectileImpairmentEvidence?.configured !== true ||
        activeProjectileImpairmentEvidence.gameCopiesDropped < 1 ||
        activeProjectileImpairmentEvidence.eventCopiesDelayed < 1 ||
        activeProjectileImpairmentEvidence.eventCopiesReleased < 1 ||
        activeProjectileImpairmentEvidence.eventCopiesExpired !== 0 ||
        activeProjectileImpairmentEvidence.eventCopiesDelayed !==
          activeProjectileImpairmentEvidence.eventCopiesReleased +
            activeProjectileImpairmentEvidence.eventCopiesPending
      ) {
        throw new Error(`active projectile impairment lacked exact realized recovery evidence: ${JSON.stringify(activeProjectileImpairmentEvidence)}`);
      }
    }
    const finalProvenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript, wasm: wasmPath, clientEntry });
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    await actor.keyDown("Space");
    await waitUntil("final held-Space player list remains complete after bot death and respawn", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return metrics.scoreboardVisible === true &&
        // Chaos owns two standalone lethal weapon cycles. Extreme permits
        // the stronger bullet-lethal branch, but its ordinary contract is a
        // visible bullet decrement followed by one bomb-owned death. Bind
        // this final persistence check to the exact score already proved by
        // those completed cycles instead of inventing a second Extreme win.
        metrics.selfWins >= (repeatedCombatCycle?.winsAfter ?? bombCycle.winsAfter) &&
        observedBotIds.every((id) => (metrics.scoreboardPlayerIds || []).includes(id));
    }, 5_000);
    const scoreboardAfterCombat = (await actor.metricsState()).uiMetrics || {};
    const scorePresentation = {
      scoreboardVisible: scoreboardAfterCombat.scoreboardVisible,
      playerId,
      winsBefore: winsBeforeCombat,
      winsAfter: scoreboardAfterCombat.selfWins,
      botIds: (scoreboardAfterCombat.scoreboardPlayerIds || [])
        .filter((id) => id >= serverBotIdBase),
    };
    // Freeze the gameplay stream before auditing process-wide security logs.
    // The child stdout pipe can lag the browser by a few turns under debug
    // sim-observe output; auditing while the actor is still emitting inputs
    // allowed a late door/input reject to arrive after an apparent clean
    // result. This bridge requests the ordinary application disconnect and
    // does not inject gameplay packets. The delivery audit owns that
    // disconnect because its ingress drain is disconnect cleanup evidence.
    if (networkProfile) {
      await actor.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest?.(); true");
      await waitUntil("Chrome measured impairment drains before disconnect", async () => {
        const evidence = await actor.evaluate("window.__zubspaceNetworkProfileStateForTest?.() || null");
        return evidence?.measurementEnded === true &&
          evidence.s2c.pending === 0 && evidence.c2s.pending === 0 &&
          evidence.s2c.drainPending === 0 && evidence.c2s.drainPending === 0;
      }, 10_000);
    }
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [chrome.logs],
      disconnect: async () => {
        await actor.evaluate("window.__zubspaceSendDisconnectForTest?.(); true");
      },
    });
    await waitUntil("Chrome bot actor disconnect cleanup", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) return false;
      const status = await response.json();
      // The application disconnect retires gameplay authority immediately;
      // the underlying RTC session intentionally remains alive until the
      // browser/ICE teardown in `finally`. Waiting on a nonexistent
      // `sessions` field both hid that distinction and always timed out.
      return status.clients === 0 && status.joinedPlayers === 0;
    }, 30_000);
    let networkProfileEvidence = null;
    if (networkProfile) {
      await waitUntil("deterministic network profile timers to drain", async () => {
        const evidence = await actor.evaluate(
          "window.__zubspaceNetworkProfileStateForTest?.() || null",
        );
        return evidence?.s2c?.pending === 0 && evidence?.c2s?.pending === 0;
      }, 10_000);
      networkProfileEvidence = await actor.evaluate(
        "window.__zubspaceNetworkProfileStateForTest?.() || null",
      );
      assertNetworkProfileEvidence(networkProfileEvidence, {
        expectedName: networkProfile,
        expectedSeed: networkProfileSeed,
        label: `deterministic ${networkProfile} profile`,
      });
    } else {
      await sleep(100);
    }
    const observerProfileResponse = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
    if (!observerProfileResponse.ok) {
      throw new Error(`server observer profile status failed: ${observerProfileResponse.status}`);
    }
    const serverObserverProfileEvidence = assertServerObserverProfile(
      await observerProfileResponse.json(),
      serverObserverProfile,
    );
    const securityAudit = assertSecurityRejectAudit(server);
    const successArtifacts = successArtifactDir
      ? await saveBrowserE2eArtifacts({
          artifactDir: successArtifactDir,
          server,
          staticServer,
          urls: [actorUrl],
          clients: [{ label: "bot-actor", client: actor }],
          extraLogs: { chrome: chrome.logs,
            "bot-facing-collection": JSON.stringify(facingCollectionStates) },
          provenance: finalProvenance,
        })
      : null;
    const result = {
      loadoutFixture: observedLoadoutFixture,
      botSpawnFixture: botSpawnFixture?.evidence || null,
      ok: true,
      diagnosticOnly: process.env.ZUBSPACE_E2E_PRESENTATION_READ_TIMING === "1",
      transport: "rtc",
      browserPair: "chrome-server-bots",
      arena,
      initialReadinessEvidence,
      setupNavigation: actor.rendezvousTrace || [],
      playerId,
      botIds: observedBotIds,
      snapshotBotIds: [...observedSnapshotBots].sort((left, right) => left - right),
      radarBotIds: [...observedRadarBots].sort((left, right) => left - right),
      scoreboardEvidence,
      botFeelEvidence,
      initialReadinessEncounter,
      feelEncounter,
      motionSoakEvidence,
      agedJoinEvidence,
      longHorizonBotSmoothness,
      botSpriteEvidence,
      snapshotOnlyPresentation,
      botRepelEvidence,
      projectileOwnerEvidence,
      facingEvidence,
      audioEvidence,
      wallCollisionEvidence,
      combatViewGeometryEvidence,
      botToPlayer,
      appliedBotHit,
      playerToBot,
      bulletLethalHit,
      bombLethalHit,
      bulletCycle,
      bombCycle,
      combatShipEvidence,
      bombShipEvidence,
      bombAttempts,
      earlyAuthoredBombEvidence,
      lateAuthoredBombEvidence,
      stationaryBotEvidence,
      passiveFireEvidence,
      lateAuthoredBombAttempts,
      movingBombTimelineEvidence,
      postRespawnBotDamage,
      repeatedBulletCycle,
      repeatedCombatCycle,
      botLethalBrake,
      incomingSourcePreparation,
      incomingDamageApproach,
      scorePresentation,
      lethalBotHit,
      counterfirePreparation: botCounterfire.preparation ?? null,
      requireFreshCounterfire,
      objectiveEvidenceBefore,
      objectiveEvidenceAfter,
      hiddenPrizeEvidence,
      securityAudit,
      dataChannelDeliveryAudit,
      activeProjectileImpairmentEvidence,
      networkProfileEvidence,
      serverObserverProfileEvidence,
      provenance: finalProvenance,
      final: compactMetrics(finalState.uiMetrics),
      successArtifacts,
    };
    if (successArtifactDir) {
      const resultPath = path.join(successArtifactDir, "result.json");
      fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
      validateArtifactPathReferences(result, resultPath);
      validateQaTraceArtifacts(result, resultPath);
    }
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifactDir = path.join("/tmp", `zubspace-bot-authority-e2e-${arena}-${Date.now()}-${process.pid}`);
    const artifacts = await saveBrowserE2eArtifacts({
      artifactDir,
      server,
      staticServer,
      urls: [actorUrl],
      clients: [{ label: "bot-actor", client: actor }],
      extraLogs: {
        chrome: chrome.logs,
        "setup-navigation": JSON.stringify(actor?.rendezvousTrace || []),
        "bot-spawn-fixture": JSON.stringify(botSpawnFixture?.evidence || null),
        "bot-presentation-snapshots": JSON.stringify(projectilePresentationStates),
        "bot-facing-collection": JSON.stringify(facingCollectionStates),
      },
    });
    const footer = browserE2eFailureFooter(artifacts);
    // AssertionError captures its stack before message augmentation. Retain
    // the artifact links in the printed stack too; the complete native log
    // is already saved in that directory.
    const originalStack = error.stack;
    error.message = `${error.message}\n${footer}\n\nzubspace_server logs:\n${server.logs.join("").trim()}`;
    if (originalStack) error.stack = `${originalStack}\n${footer}`;
    throw error;
  } finally {
    await actor?.close().catch(() => {});
    await Promise.all([
      stopProcess(chrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(chrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    if (botSpawnFixture) fs.rmSync(botSpawnFixture.root, { recursive: true, force: true });
    staticServer.server.close();
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}

export { main };
