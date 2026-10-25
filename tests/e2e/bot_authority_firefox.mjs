#!/usr/bin/env node
import { prepareKeyboardLifecycleOrigin } from "./keyboard_lifecycle_origin.mjs";
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import { createBotSpawnFixture } from "./bot_spawn_fixture.mjs";
import { agedServerJoinOptions, waitForAgedServerJoin } from "./aged_server_join.mjs";

import { selectPlayerShip } from "./player_ship_identity.mjs";
import { botAuthorityLoadout, assertBotAuthorityLoadout, stationaryWarbirdBombLaneMinimum, botBombSafetyProfile, botBombSafetyFloor } from "./bot_authority_loadout.mjs";
import fs from "node:fs";
import { movingBombAttackPlan, movingBombControlStep, movingBombNeedsVelocityControl } from "./moving_bomb_pilot.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { approachLiveBotEncounter } from "./bot_encounter_staging.mjs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { installBotMotionCapture } from "./bot_motion_capture.mjs";
import { proveRemoteMotionSoak } from "./remote_motion_soak.mjs";
import { prepareHostileCounterfireLife, alignCounterfireAssignment, prepareBotFacingSources, facingHunterTarget } from "./counterfire_life.mjs";
import { collectBotFacingEvidence } from "./bot_facing.mjs";
import { facingLaunchStep } from "./bot_facing_encounter.mjs";

import {
  createSession,
  deleteSession,
  execute,
  freezeQaTrace,
  firefoxArtifactClient,
  firefoxKeyDown,
  firefoxKeyUp,
  freePort,
  holdFirefoxKeys,
  holdThenChordFirefoxKeys,
  metricsState as firefoxMetricsState,
  openClient,
  pageState,
  releaseFirefoxKeys,
  repoRoot,
  screenshot,
  sleep,
  startGeckodriver,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForGeckodriver,
  waitForServerListening,
  waitUntil,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import { assertRendererConsoleClean } from "../../tools/browser_console_evidence.mjs";
import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  brakeClientVelocity,
  driveClientToWorldTarget,
  parseServerProjectileEvents,
  saveBrowserE2eArtifacts,
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
import {
  assertHumanProjectilePresentationGeometry,
  exactLifePrimaryBombTerminal,
  renewRetiredBombTarget,
  captureBotPresentationState,
  chaosBombStationaryLaneDisposition,
  explosiveContactOverlapsHull,
  lethalHitWithNonlethalPrecursor,
  metricsTraceWithViewport,
  observedActorSourceTick,
  ownedProjectileTailSettled,
  provePostRespawnBotDamage,
  provePresentedBotCombatCycle,
  proveAuthoritativeBotRepel,
  quantizedProjectileIntercept,
  remoteActorMotionAcrossStatesEvidence,
  botDoorRelocationEvidence,
  remoteActorMotionWithDoorEvidence,
  routedMapCounterfireWaypoint,
  counterfireRouteNeedsBrake,
  serverDamageBoundary,
  serverDamageEventsSince,
  stationaryChaosBombHoldingPoint,
  stationaryBotMotionEvidence,
  stationaryFixtureLifeSeqs,
  requiredStationaryFixtureLifeSeqs,
  stageExtremeSafeEdgeCombat,
  sustainedExactLifeAttackEvidence,
  timedMovementWindows,
  waitForPresentedLiveBotSuccessor,
  worldPointOnCenteredViewport,
  nearestAuthoredSafetyEdge,
  enterNearestAuthoredSafety,
  leaveAuthoredSafety,
  lureTargetOutOfSafety,
} from "./bot_combat_evidence.mjs";

const botIdBase = 0x0300;
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const expectedBotCount = Number(process.env.ZUBSPACE_SMOKE_BOT_COUNT || "0");
const successArtifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
  (process.env.ZUBSPACE_E2E_ARTIFACT_ROOT ? path.join(process.env.ZUBSPACE_E2E_ARTIFACT_ROOT,
    `zubspace-bots-firefox-${arena}-${Date.now()}-${process.pid}`) : "");
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
const e2eScript = "tests/e2e/bot_authority_firefox.mjs";
const runStartedAt = new Date().toISOString();
process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
const bulletWeaponTypes = new Set([1, 2]);
const loadoutFixture = botAuthorityLoadout(process.env, arena);
const bombSafetyProfile = botBombSafetyProfile(loadoutFixture, fs.readFileSync(
  path.join(repoRoot, "assets/zones", arena, "arena.conf"), "utf8"));
// Exact type, not a union: maximum entry grants proximity in both zones;
// the separately declared reference Extreme fixture still requires direct.
const bombWeaponTypes = new Set([loadoutFixture.bombWeaponType]);
const extremeRecoveryMaximumSpeed = 30;
// Production bots pursue to a 320px combat envelope. Leave one 20px
// snapshot/hull margin so both browser pilots can acquire that ordinary
// encounter. The authored Warbird bomb travels 1,075px over its lifetime;
// exact quantized intercept and server-owned lethal damage still decide pass.
const extremeBombFiringDistance = 340;
// The protected Extreme bomb staging point is 48 px behind the safe-edge
// tile center, and the first legal nonsafe sample is 12 px beyond it. With
// the authored Shift-thrust acceleration the ship reaches about 200 px/s at
// that boundary. Constant-acceleration travel time is 2d/v, so lead the bot
// through the real keyboard crossing instead of aiming at its predecessor
// position and pretending the fire edge is instantaneous.
const extremeBombProtectedStagingDistance = 48;
const extremeBombProtectedCrossingDistance = 60;
const extremeBombExpectedExitSpeed = 200;
// Extreme Warbird's authored InitialRotation=300 maps to 0.75 turns/s. A
// worst-case half-turn therefore needs 667 ms before WebDriver sampling; keep
// the exposed revalidation bound physically possible without extending it to
// a full combat-pursuit timeout.
const extremeBombExposedAimTimeoutMs = 750;
// A stationary target must remain ahead through the maximum turn admitted by
// the 0.9 outward-alignment rule. Reserve that authored turn distance plus the
// 12px legal crossing, the greater of direct contact and authored BombSafety,
// and one tile of sampling margin. Maximum-entry proximity must not inherit
// the tighter direct-bomb lane: held Z then fires only beyond the target.
const extremeWarbirdInitialRotationPerSecond = 300 / 400;
const extremeBombMinimumStationaryLaneDistance = arena === "extreme"
  ? stationaryWarbirdBombLaneMinimum({
      fixture: loadoutFixture,
      arenaConfig: fs.readFileSync(path.join(repoRoot, "assets/zones/extreme/arena.conf"), "utf8"),
      expectedExitSpeed: extremeBombExpectedExitSpeed,
      rotationPerSecond: extremeWarbirdInitialRotationPerSecond,
    })
  : 0;
// Extreme's Lancaster and Warbird both author BulletFireDelay=6 (60 ms). Hold
// the genuine opening chord across seven complete intervals so a moving-life
// cycle can outpace authored recharge instead of emitting two rounds and
// spending seconds reacquiring a 320px/s target.
const extremeOpeningBulletBurstMs = 7 * 60;
// Reciprocal lethality needs the map-backed clear lane owned by the shared
// 120/150/180px waypoint planner, not merely any point inside the bot's 320px
// pursuit envelope. At 250px a wire-quantized Extreme stream can land only
// sparse edge hits while authored recharge erases every decrement. The
// ordinary-key route and bounded brake below now make the 180px terminal
// physically reachable without changing damage, recharge, or bot authority.
const botCounterfireStagingDistance = 180;
const extremeBombProtectedCrossingSeconds =
  2 * extremeBombProtectedCrossingDistance / extremeBombExpectedExitSpeed;
const hiddenPrizeSeed = Number(process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED || "2017");
process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = String(hiddenPrizeSeed);

function signedOrientationDelta(current, desired) {
  return ((desired - current + 1.5) % 1) - 0.5;
}

function reportStage(stage, details = {}) {
  console.error(JSON.stringify({ type: "bot_authority_firefox_stage", stage, at: Date.now(), ...details }));
}

if (!Number.isInteger(expectedBotCount) || expectedBotCount < 1) {
  throw new Error("Firefox bot authority replay requires ZUBSPACE_SMOKE_BOT_COUNT>=1");
}
if (!Number.isInteger(hiddenPrizeSeed) || hiddenPrizeSeed === 0) {
  throw new Error("Firefox bot authority replay requires a nonzero integer hidden-prize seed");
}
if (!["chaos-zone", "extreme"].includes(arena)) {
  throw new Error("Firefox bot authority replay is scoped to chaos-zone and extreme");
}
if (
  arena === "extreme" &&
  (!stationaryBotAfterHumanDamage || stationaryBotFromLifeSeq !== 1 ||
    stationaryBotStaticLifeSeqs.join(",") !== "2,3")
) {
  throw new Error(
    "Firefox Extreme exact-life geometry requires " +
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
  throw new Error("Firefox stationary bot lives must be unique increasing positive integers");
}
if (
  passiveBotLifeSeqs.some((lifeSeq) => !Number.isInteger(lifeSeq) || lifeSeq <= 0) ||
  passiveBotLifeSeqs.some((lifeSeq, index) => index > 0 && lifeSeq <= passiveBotLifeSeqs[index - 1])
) {
  throw new Error("Firefox passive bot lives must be unique increasing positive integers");
}
const supportedPassiveCollisionFixture =
  passiveBotLifeSeqs.length === 0 ||
  (
    arena === "chaos-zone" && expectedBotCount === 2 && !networkProfile &&
    passiveBotLifeSeqs.join(",") === "1,2" &&
    !stationaryBotAfterHumanDamage && stationaryBotFromLifeSeq === 0 &&
    stationaryBotStaticLifeSeqs.length === 0
  ) ||
  (
    arena === "chaos-zone" &&
    stationaryBotAfterHumanDamage &&
    stationaryBotFromLifeSeq === 2 &&
    (
      (passiveBotLifeSeqs.length === 1 && passiveBotLifeSeq === 2) ||
      passiveBotLifeSeqs.join(",") === "2,3"
    )
  ) ||
  (
    arena === "extreme" &&
    stationaryBotAfterHumanDamage &&
    stationaryBotFromLifeSeq === 1 &&
    stationaryBotStaticLifeSeqs.join(",") === "2,3" &&
    passiveBotLifeSeq === 3 &&
    (
      passiveBotLifeSeqs.length === 1 ||
      passiveBotLifeSeqs.join(",") === "1,3"
    )
  );
if (
  !supportedPassiveCollisionFixture
) {
  throw new Error(
    "Firefox passive combat isolation requires moving clean two-bot Chaos lives 1,2 or stationary/passive Chaos life 2 " +
    "(optionally repeated-bomb life 3) or stationary/passive Extreme life 3 " +
    "(optionally isolated human-combat life 1)",
  );
}
if (!["", "mild", "slow"].includes(networkProfile)) {
  throw new Error("Firefox bot authority replay supports only mild/slow deterministic network profiles");
}
if (!Number.isInteger(networkProfileSeed) || networkProfileSeed < 1 || networkProfileSeed > 0xffff_ffff) {
  throw new Error("Firefox bot authority replay network profile seed must be a nonzero u32 integer");
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

function authoredStationaryBotAimIsClear(fromX, fromY, toX, toY) {
  const deltaX = toX - fromX;
  const deltaY = toY - fromY;
  const distance = Math.hypot(deltaX, deltaY);
  if (!Number.isFinite(distance) || distance <= 0.01) return false;

  // Match the server bot's zero-velocity collision-avoidance probe. A clear
  // center ray is insufficient beside a wall: the server correctly selects
  // a hull-safe avoidance heading and will never align a stationary fixture
  // to fire. Requiring the same 40-bin, radius-plus-40 corridor makes the
  // browser stage a genuine firing lane instead of waiting on impossible
  // fixture geometry.
  const orientation = Math.atan2(deltaX, -deltaY) / (Math.PI * 2);
  // Production bots select the nearest one of Continuum's 40 legal headings
  // before turning. Match that server-authored choice here; the ordinary
  // client wire adapter still deliberately truncates its already-discrete
  // orientation.
  const direction = Math.round(((orientation % 1) + 1) % 1 * 40) % 40;
  const angle = direction / 40 * Math.PI * 2;
  const headingX = Math.sin(angle);
  const headingY = -Math.cos(angle);
  const hullRadius = 14;
  const probeDistance = hullRadius + 40;
  const steps = Math.ceil(probeDistance / hullRadius);
  for (let step = 1; step <= steps; step += 1) {
    const progress = step / steps;
    if (!authoredHullIsOpen(
      fromX + headingX * probeDistance * progress,
      fromY + headingY * probeDistance * progress,
      hullRadius,
    )) {
      return false;
    }
  }
  return true;
}

function assertObjectivesDisabled(metrics) {
  const objective = metrics?.localGameplayState || {};
  if (
    objective.networkedPlay !== true ||
    objective.objectivePresentationAuthoritativeOnly !== true ||
    (objective.authoritativeFlagCount ?? 0) !== 0 ||
    (objective.authoritativePowerballCount ?? 0) !== 0 ||
    (objective.objectivePresentedFlagCount ?? 0) !== 0 ||
    (objective.objectivePresentedPowerballCount ?? 0) !== 0 ||
    objective.localFlagFallbackPresented === true ||
    objective.localPowerballFallbackPresented === true
  ) {
    throw new Error(`Firefox objective presentation did not stay disabled: ${JSON.stringify(objective)}`);
  }
  return objective;
}

function assertFirefoxBotAudio(state) {
  const unlock = state.audioUnlock;
  const probe = state.firefoxAudioProbe;
  if (
    !unlock?.installed ||
    !unlock.gestureSeen ||
    unlock.trackedContexts < 1 ||
    unlock.resumeFailures !== 0 ||
    !probe?.available ||
    probe.starts < 1 ||
    probe.runningStarts < 1 ||
    probe.sourceConnections < 1 ||
    !probe.durations?.some(Number.isFinite) ||
    (probe.errors || []).length > 0
  ) {
    throw new Error(`Firefox bot audio was not unlocked and played: ${JSON.stringify({ unlock, probe })}`);
  }
  return { unlock, probe };
}

function assertSnapshotOnlyBotPresentation(state) {
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
    throw new Error(`Firefox did not use bounded header-only packet evidence: ${JSON.stringify(packetAudit)}`);
  }
  if (legacyPositionRelayCount > 0) {
    throw new Error(`Firefox received retired legacy position relays: ${JSON.stringify({
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

const minimumFirefoxTraceFrames = 120;
const minimumFirefoxTraceSpanMs = 8_500;
const minimumFirefoxAuthoritativeFrames = 100;

function assertFirefoxTraceWindow(frames, label) {
  const spanMs = frames.length < 2 ? 0 : frames.at(-1).at - frames[0].at;
  if (
    frames.length < minimumFirefoxTraceFrames ||
    spanMs < minimumFirefoxTraceSpanMs
  ) {
    throw new Error(`Firefox retained an incomplete ${label} window: ${JSON.stringify({
      frames: frames.length,
      spanMs,
      minimumFrames: minimumFirefoxTraceFrames,
      minimumSpanMs: minimumFirefoxTraceSpanMs,
    })}`);
  }
  return spanMs;
}

function assertFirefoxBotSpriteTrace(spriteTrace, botIds, startedAt) {
  const frames = (spriteTrace || []).filter((frame) => frame.at >= startedAt);
  const spanMs = assertFirefoxTraceWindow(frames, "actual sprite");
  const bots = botIds.map((botId) => {
    let authoritativeFrames = 0;
    let drawnFrames = 0;
    let onCanvasFrames = 0;
    const shipFrames = new Set();
    for (const frame of frames) {
      const entry = (frame.entries || []).find((candidate) => candidate.id === botId);
      if (!entry?.authoritativeMember) continue;
      authoritativeFrames += 1;
      if (
        entry.drawn !== true ||
        entry.suppression !== "none" ||
        !(entry.alpha > 0) ||
        entry.positionPresent !== true ||
        entry.renderPositionPresent !== true
      ) {
        throw new Error(`Firefox bot ${botId} suppressed an authoritative sprite: ${JSON.stringify({ frame, entry })}`);
      }
      drawnFrames += 1;
      shipFrames.add(entry.shipFrame);
      if (
        Number.isFinite(entry.screenX) &&
        Number.isFinite(entry.screenY) &&
        entry.screenX >= -24 && entry.screenX <= 1_464 &&
        entry.screenY >= -24 && entry.screenY <= 1_024
      ) {
        onCanvasFrames += 1;
      }
    }
    if (
      authoritativeFrames < minimumFirefoxAuthoritativeFrames ||
      drawnFrames !== authoritativeFrames ||
      onCanvasFrames < 30
    ) {
      throw new Error(`Firefox bot ${botId} lacked continuous actual sprite draws: ${JSON.stringify({
        authoritativeFrames,
        drawnFrames,
        onCanvasFrames,
        shipFrames: [...shipFrames],
      })}`);
    }
    return {
      botId,
      authoritativeFrames,
      drawnFrames,
      onCanvasFrames,
      shipFrames: [...shipFrames].sort((left, right) => left - right),
    };
  });
  return {
    startedAt,
    endedAt: frames.at(-1).at,
    spanMs,
    frameCount: frames.length,
    bots,
    frames: frames.map((frame) => ({
      at: frame.at,
      snapshotId: frame.snapshotId,
      entries: (frame.entries || []).filter((entry) => botIds.includes(entry.id)),
    })),
  };
}

function u32IsStrictlyNewer(candidate, predecessor) {
  if (!Number.isInteger(candidate) || !Number.isInteger(predecessor)) return false;
  const distance = (candidate - predecessor + 0x1_0000_0000) % 0x1_0000_0000;
  return distance > 0 && distance < 0x8000_0000;
}

function assertFirefoxBotTrace(metricsTrace, botIds, startedAt, botDamageEvents, doorRelocations = []) {
  const frames = (metricsTrace || []).filter((frame) => frame.at >= startedAt);
  const spanMs = assertFirefoxTraceWindow(frames, "bot authority");
  const bots = botIds.map((botId) => {
    const lifeEvidence = new Map();
    let presentFrames = 0;
    let authoritativeLiveFrames = 0;
    let authoritativeRenderedFrames = 0;
    let overtakenDeathFrames = 0;
    for (const frame of frames) {
      const authoritativeActor = (frame.authoritativeSnapshotActors || [])
        .find((actor) => actor.id === botId);
      const presentation = (frame.remoteActorPresentation || [])
        .find((actor) => actor.id === botId);
      const player = (frame.remotePlayers || []).find((actor) => actor.id === botId);
      // A reliable death can overtake one final unordered snapshot from the
      // predecessor life. Suppressing that dead life is correct; every
      // matching-life authoritative AOI frame must still render exactly.
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
          throw new Error(`Firefox bot ${botId} was omitted while alive in the authoritative AOI: ${JSON.stringify({
            at: frame.at,
            authoritativeActor,
            presentation: presentation || null,
            player: player || null,
          })}`);
        }
        authoritativeRenderedFrames += 1;
      }
      if (!player || !Number.isInteger(player.lifeSeq)) continue;
      presentFrames += 1;
      const life = lifeEvidence.get(player.lifeSeq) || {
        lifeSeq: player.lifeSeq,
        samples: [],
      };
      life.samples.push({
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
      lifeEvidence.set(player.lifeSeq, life);
    }
    const lives = [...lifeEvidence.values()].map((life) => {
      const finite = life.samples.filter((sample) =>
        [sample.x, sample.y, sample.displayX, sample.displayY].every(Number.isFinite)
      );
      const xs = finite.map((sample) => sample.x);
      const ys = finite.map((sample) => sample.y);
      const displacement = finite.length === 0 ? 0 : Math.hypot(
        Math.max(...xs) - Math.min(...xs),
        Math.max(...ys) - Math.min(...ys),
      );
      const rollingMovement = timedMovementWindows(finite);
      const smoothness = remoteActorMotionWithDoorEvidence(finite, botId, doorRelocations);
      return {
        lifeSeq: life.lifeSeq,
        sampleCount: life.samples.length,
        displacement,
        rollingMovement,
        smoothness,
      };
    });
    const maximumLifeDisplacement = Math.max(0, ...lives.map((life) => life.displacement));
    const totalLifeDisplacement = lives.reduce(
      (total, life) => total + life.displacement,
      0,
    );
    const rollingMovement = lives.flatMap((life) =>
      life.rollingMovement.map((window) => ({ lifeSeq: life.lifeSeq, ...window }))
    );
    const stalledWindows = rollingMovement.filter((window) => window.displacement < 16);
    const failedSmoothness = lives.filter((life) => !life.smoothness.ok);
    const stoppedAttackEvidence = stalledWindows.length === 0
      ? null
      : sustainedExactLifeAttackEvidence(botDamageEvents, {
          botId,
          lifeSeqs: lives.map((life) => life.lifeSeq),
        });
    const presenceRatio = authoritativeLiveFrames === 0
      ? 0
      : authoritativeRenderedFrames / authoritativeLiveFrames;
    const overallPresenceRatio = presentFrames / frames.length;
    // Four authored tiles is unambiguously material on-screen translation,
    // while remaining stable when the ten-second sample begins partway
    // through a bot's deliberate aim/braking phase. The server wall trace
    // below independently requires collision contacts and a continuously
    // clear hull, so this is not satisfied by interpolation jitter.
    if (
      authoritativeRenderedFrames !== authoritativeLiveFrames ||
      authoritativeLiveFrames < minimumFirefoxAuthoritativeFrames ||
      totalLifeDisplacement < 64 ||
      rollingMovement.length === 0 ||
      (stalledWindows.length > 0 && stoppedAttackEvidence == null) ||
      failedSmoothness.length > 0
    ) {
      throw new Error(`Firefox bot ${botId} lacked material movement or exact-life attack activity: ${JSON.stringify({
        presenceRatio,
        overallPresenceRatio,
        authoritativeLiveFrames,
        authoritativeRenderedFrames,
        overtakenDeathFrames,
        maximumLifeDisplacement,
        totalLifeDisplacement,
        rollingMovement,
        stalledWindows,
        failedSmoothness,
        stoppedAttackEvidence,
        lives,
      })}`);
    }
    return {
      botId,
      presentFrames,
      presenceRatio,
      overallPresenceRatio,
      authoritativeLiveFrames,
      authoritativeRenderedFrames,
      overtakenDeathFrames,
      maximumLifeDisplacement,
      totalLifeDisplacement,
      rollingMovement,
      failedSmoothness,
      stoppedAttackEvidence,
      lives,
    };
  });
  return {
    startedAt,
    endedAt: frames.at(-1).at,
    spanMs,
    frameCount: frames.length,
    bots,
    frames: frames.map((frame) => ({
      at: frame.at,
      playerAlive: frame.playerAlive,
      serverPlayerAlive: frame.serverPlayerAlive,
      playerRespawnSeq: frame.playerRespawnSeq,
      authoritativeSnapshotId: frame.authoritativeSnapshotId,
      latestSnapshotId: frame.latestSnapshotId,
      latestSnapshotServerTick: frame.latestSnapshotServerTick,
      latestServerTick: frame.latestServerTick,
      combatViewTimeline: frame.combatViewTimeline || null,
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

function assertFirefoxBotProjectileOwnersVisible(
  metricsTrace,
  spriteTrace,
  botIds,
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
      if (!worldPointOnCenteredViewport(frame, projectile.x, projectile.y)) continue;
      const key = `${projectile.playerId}:${projectile.lifeSeq}:${projectile.eventId}`;
      if (!observed.has(key)) observed.set(key, { frame, projectile });
    }
  }
  if (observed.size < 1) {
    if (!required) return null;
    throw new Error("Firefox observed no fresh bot projectile presentation source");
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
        throw new Error(`Firefox installed exact active-projectile state without a matching texture draw: ${JSON.stringify({
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
      throw new Error(`Firefox presented a fresh projectile without its live on-canvas owner: ${JSON.stringify({
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
    throw new Error("Firefox never proved a self-contained active-projectile texture draw");
  }
  return {
    freshProjectileGroups: evidence.length,
    exactActiveProjectileDraws: evidence.filter((entry) =>
      entry.presentationPath === "exact-active-projectile"
    ).length,
    evidence: evidence.slice(0, 64),
  };
}

function playerDamageToBots(server, playerId, boundary) {
  return serverDamageEventsSince(server, boundary).filter((event) =>
    event.shooterId === playerId && event.victimId >= botIdBase
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
  // Preserve potentially disruptive spawns without treating an unclaimed
  // green elsewhere in the arena as a target-side reset. The bomb proof below
  // binds preparation and lethal ownership to one exact server bot life.
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
    throw new Error(`Firefox server bot wall trace was incomplete or unsafe: ${JSON.stringify({
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

async function proveFirefoxHeldFireRotation(firefox, server, playerId) {
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const before = (await firefoxMetricsState(firefox)).uiMetrics || {};
    if (before.playerAlive !== true || before.serverPlayerAlive !== true) {
      await sleep(250);
      continue;
    }
    const traceOffset = traceRecords(server, "human authority input trace").length;
    await holdThenChordFirefoxKeys(firefox, "Control", "\uE014", 220, 520);
    const after = (await firefoxMetricsState(firefox)).uiMetrics || {};
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
  throw new Error("Firefox holding fire before right rotation produced no accepted input/turn");
}

function botDamageToPlayer(server, playerId, boundary) {
  return serverDamageEventsSince(server, boundary).filter((event) =>
    event.shooterId >= botIdBase && event.victimId === playerId
  );
}

async function stageFirefoxForBotLethalCounterfire(
  actor,
  server,
  playerId,
  damageOffset,
  shooterLifeSeqExclusive = null,
  assignedSource = null,
) {
  const eligibleLethal = () => botDamageToPlayer(server, playerId, damageOffset)
    .find((event) =>
      event.lethal === true &&
      (assignedSource == null || (event.shooterId === assignedSource.id &&
        event.shooterLifeSeq === assignedSource.lifeSeq)) &&
      (shooterLifeSeqExclusive == null ||
        u32IsStrictlyNewer(event.shooterLifeSeq, shooterLifeSeqExclusive))
    ) || null;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const lethal = eligibleLethal();
    if (lethal) return lethal;

    const metrics = (await actor.metricsState()).uiMetrics || {};
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) {
      await sleep(100);
      continue;
    }
    const target = (metrics.remotePlayers || [])
      .filter((player) =>
        player.id >= botIdBase &&
        (assignedSource == null || (player.id === assignedSource.id &&
          player.lifeSeq === assignedSource.lifeSeq)) &&
        Number.isFinite(player.positionX) &&
        Number.isFinite(player.positionY)
      )
      .sort((left, right) =>
        Math.hypot(left.positionX - metrics.playerX, left.positionY - metrics.playerY) -
        Math.hypot(right.positionX - metrics.playerX, right.positionY - metrics.playerY)
      )[0];
    if (!target) {
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
    const botAimIsClear = authoredStationaryBotAimIsClear(
      target.positionX,
      target.positionY,
      metrics.playerX,
      metrics.playerY,
    );
    if (metrics.inSafeZone !== true && lineIsClear && botAimIsClear && distance <= botCounterfireStagingDistance) {
      const braking = await brakeClientVelocity(actor);
      if (!braking.braked) continue;
      await sleep(250);
      continue;
    }
    const waypoint = routedMapCounterfireWaypoint({
      metrics,
      target,
      authoredHullIsTraversable,
      authoredHullIsOpen,
      authoredLineIsClear,
      authoredTargetAimIsClear: authoredStationaryBotAimIsClear,
    });
    if (waypoint == null) {
      await brakeClientVelocity(actor, 1_500);
      await sleep(100);
      continue;
    }

    const route = await driveClientToWorldTarget(
      actor,
      waypoint,
      1_200,
      false,
      24,
      async () => eligibleLethal(),
      250,
      0.8,
      1_200,
    );
    const afterRouteLethal = eligibleLethal();
    if (afterRouteLethal) return afterRouteLethal;
    const needsBrake = counterfireRouteNeedsBrake(route);
    reportStage("counterfire_route_slice", {
      playerLifeSeq: metrics.playerLifeSeq,
      targetId: target.id,
      targetLifeSeq: target.lifeSeq,
      from: { x: metrics.playerX, y: metrics.playerY },
      waypoint,
      reached: route.reached,
      reason: route.reason ?? null,
      pulses: route.pulses,
      bestDistance: route.bestDistance ?? null,
      final: route.final ? {
        x: route.final.playerX, y: route.final.playerY,
        velocityX: route.final.playerVelocityX, velocityY: route.final.playerVelocityY,
      } : null,
      needsBrake,
    });
    if (needsBrake) await brakeClientVelocity(actor, 1_500);
  }
  return null;
}

function playerWeaponDamageToBots(server, playerId, offset, weaponTypes) {
  return playerDamageToBots(server, playerId, offset).filter((event) =>
    weaponTypes.has(event.weapon & 0x1f)
  );
}

function interceptAimVector(
  relativeX,
  relativeY,
  relativeVelocityX,
  relativeVelocityY,
  speed,
  maxTime = 0.75,
) {
  const a = relativeVelocityX * relativeVelocityX +
    relativeVelocityY * relativeVelocityY - speed * speed;
  const b = 2 * (relativeX * relativeVelocityX + relativeY * relativeVelocityY);
  const c = relativeX * relativeX + relativeY * relativeY;
  let time = Math.sqrt(c) / Math.max(speed, 1);
  let feasible = false;
  if (Math.abs(a) > 1e-6) {
    const discriminant = b * b - 4 * a * c;
    if (discriminant >= 0) {
      const root = Math.sqrt(discriminant);
      const candidates = [(-b - root) / (2 * a), (-b + root) / (2 * a)]
        .filter((candidate) => candidate > 0 && Number.isFinite(candidate));
      if (candidates.length > 0) {
        time = Math.min(...candidates);
        feasible = true;
      }
    }
  } else if (Math.abs(b) > 1e-6) {
    const candidate = -c / b;
    if (candidate > 0 && Number.isFinite(candidate)) {
      time = candidate;
      feasible = true;
    }
  } else if (c <= 1e-6) {
    time = 0;
    feasible = true;
  }
  feasible = feasible && time <= maxTime;
  time = Math.min(maxTime, Math.max(0, time));
  return {
    x: relativeX + relativeVelocityX * time,
    y: relativeY + relativeVelocityY * time,
    feasible,
  };
}

const extremeFirefoxSafetyGeometry = {
  authoredTiles,
  positionIsOnAuthoredSafeTile,
  authoredTileIsSolid,
  authoredHullIsOpen,
  authoredLineIsClear,
};

async function leaveExtremeSafetyWithFirefoxInput(actor) {
  return leaveAuthoredSafety(actor);
}

function nearestExtremeFirefoxSafeEntry(metrics) {
  return nearestAuthoredSafetyEdge(extremeFirefoxSafetyGeometry, metrics);
}

async function enterNearestExtremeFirefoxSafety(actor, expectedLifeSeq) {
  return enterNearestAuthoredSafety(extremeFirefoxSafetyGeometry, actor, expectedLifeSeq);
}

async function lureExtremeFirefoxTargetOutOfSafety(
  actor,
  preferredTargetId,
  preferredTargetLifeSeq,
) {
  return lureTargetOutOfSafety({
    safety: extremeFirefoxSafetyGeometry,
    actor,
    preferredTargetId,
    preferredTargetLifeSeq,
    label: "Extreme Firefox",
    leaveSafety: leaveExtremeSafetyWithFirefoxInput,
  });
}

// Maintain a real hostile firing encounter while the unchanged exact-tick
// oracle collects. Merely leaving the bot outside safety does not establish range.
async function provokeFirefoxBotLaunches(actor, server, botIds, cycle = 0) {
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

async function returnExtremeFirefoxPilotToSafety(actor) {
  const center = { x: 8_200, y: 8_200 };
  if (!positionIsOnAuthoredSafeTile(center.x, center.y)) {
    throw new Error("Doomsy8's retained central recovery point is not authored safety");
  }
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await waitUntil("Extreme Firefox pilot is live before returning to safety", async () => {
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
        timeoutMs: 90_000, arrivalRadiusPx: 24, maximumDesiredSpeed: 100, desiredSpeedScale: 0.25,
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

let lastExtremeFirefoxStageDiagnostics = [];

async function stageExtremeFirefoxCombatEncounter(
  actor,
  crossingFireKey,
  preferredTargetId = null,
  preferredEdge = null,
) {
  lastExtremeFirefoxStageDiagnostics = [];
  await waitUntil("Extreme Firefox pilot is live before encounter staging", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
  }, 10_000);
  const initialMetrics = (await actor.metricsState()).uiMetrics || {};
  const initialTarget = (initialMetrics.remotePlayers || []).find((player) =>
    player.id === preferredTargetId && Number.isInteger(player.lifeSeq)
  );
  const stationaryDirectBombTarget = crossingFireKey === "z" &&
    initialTarget != null &&
    stationaryFixtureLifeSeqs(
      [initialTarget.lifeSeq],
      stationaryBotFromLifeSeq,
      stationaryBotStaticLifeSeqs,
    ).length === 1;
  if (
    initialMetrics.inSafeZone !== true &&
    !await returnExtremeFirefoxPilotToSafety(actor)
  ) {
    return false;
  }

  const deadline = Date.now() + 12_000;
  while (Date.now() < deadline) {
    // Chrome and Firefox deliberately share one map-backed interception
    // driver. The browser adapter supplies only genuine key events; exact
    // server damage remains the acceptance boundary. Extreme's authored
    // bullet envelope is 234px, while its direct-contact bomb needs the
    // tighter protected rendezvous below.
    const staged = await stageExtremeSafeEdgeCombat({
      actor,
      botIdBase,
      authoredTiles,
      positionIsOnAuthoredSafeTile,
      authoredTileIsSolid,
      authoredHullIsOpen,
      authoredLineIsClear,
      preferredTargetId,
      preferredEdge,
      crossingFireKey,
      crossingFireHoldMs: crossingFireKey === "z"
        ? 420
        : extremeOpeningBulletBurstMs,
      crossingFireReleaseSettleMs: crossingFireKey === "z" ? 200 : 40,
      crossingProjectileSpeed: crossingFireKey === "z"
        ? 430
        : (crossingFireKey === "Control" ? 360 : null),
      crossingProjectileMaxTime: crossingFireKey === "z" ? 2 : 0.75,
      // Acquire the map-backed edge before Firefox's route-and-brake latency,
      // then require the exact bot life inside the conservative direct-bomb range
      // immediately before the real safe->unsafe crossing.
      crossingApproachDistance: extremeBombFiringDistance,
      crossingMinimumApproachDistance: stationaryDirectBombTarget
        ? extremeBombMinimumStationaryLaneDistance
        : 0,
      crossingFinalApproachDistance: extremeBombFiringDistance,
      crossingMinimumApproachAlignment:
        crossingFireKey == null || crossingFireKey === "z" ? 0.9 : -1,
      crossingProtectedAimTimeout: crossingFireKey == null ? 3_000 : 12_000,
      crossingMaximumDistance: crossingFireKey === "z"
        ? extremeBombFiringDistance
        : (crossingFireKey === "Control" ? 234 : extremeBombFiringDistance),
      crossingExposedMaximumDistance: crossingFireKey === "z"
        ? extremeBombFiringDistance
        : null,
      crossingMaximumRecedingSpeed: crossingFireKey === "z"
        ? 80
        : (crossingFireKey === "Control" ? Number.POSITIVE_INFINITY : 80),
      crossingOrientationTolerance: crossingFireKey === "z"
        // Firefox samples the moving target less frequently than the server's
        // 40-bin weapon heading. Permit one adjacent render/wire bin; the
        // exact human-owned lethal server event still rejects every miss.
        ? 0.04
        : (crossingFireKey === "Control" ? 0.020 : 0.018),
      crossingThrustModifierKey: crossingFireKey == null ? null : "Shift",
      crossingExposedSettleDistance: 0,
      crossingRequiredExposedRunwayDistance: crossingFireKey === "z" ? 48 : 0,
      crossingProtectedStagingDistance: crossingFireKey === "z"
        ? extremeBombProtectedStagingDistance
        : 32,
      // A stationary exact-life collision target has no curved protected
      // orbit to revalidate. Bind its intercept while protected and submit the
      // real key on the first nonsafe sample; otherwise the inherited crossing
      // velocity can carry the pilot past the target while it turns. Moving
      // targets retain post-crossing revalidation.
      crossingPreserveProtectedAim: crossingFireKey === "z",
      crossingRevalidateProtectedAim:
        crossingFireKey === "z" && !stationaryDirectBombTarget,
      crossingReleaseThrustBeforeProtectedFire: crossingFireKey === "z",
      // The full hull corridor is checked along the quantized aim vector.
      // Permit a genuinely outward 26-degree shot rather than requiring the
      // moving intercept to remain within 14 degrees of the tile normal.
      crossingMinimumExitAimAlignment: crossingFireKey === "z" ? 0.9 : 0.97,
      crossingPreFireDelaySeconds: crossingFireKey === "z"
        ? extremeBombProtectedCrossingSeconds
        : 0,
      crossingExpectedExitSpeed: crossingFireKey === "z"
        ? extremeBombExpectedExitSpeed
        : 0,
      crossingExpectedExitSpeedAlongAim: crossingFireKey === "z"
        ? extremeBombExpectedExitSpeed
        : null,
      crossingExposedAimTimeout: crossingFireKey === "z"
        ? extremeBombExposedAimTimeoutMs
        : null,
      diagnostics: lastExtremeFirefoxStageDiagnostics,
    });
    if (staged != null && staged !== false) {
      reportStage("extreme_encounter_staged", {
        edge: staged.edge?.direction?.name ?? null,
        edgeInside: staged.edge?.inside ?? null,
        edgeOutside: staged.edge?.outside ?? null,
        botId: staged.botId,
        botLifeSeq: staged.botLifeSeq,
        aim: staged.aim,
        protectedShotAim: staged.protectedShotAim,
      });
      return staged;
    }
    const metrics = (await actor.metricsState()).uiMetrics || {};
    if (
      metrics.playerAlive !== true ||
      metrics.serverPlayerAlive !== true ||
      metrics.inSafeZone !== true
    ) {
      return false;
    }
    await sleep(200);
  }
  return false;
}

async function prepareExtremeFirefoxSafeReserve(actor, requiredEnergy) {
  // The bullet proof deliberately keeps the pilot exposed until one complete
  // visible bot life is retired. Starting the following bomb proof from the
  // wounded remainder makes its accepted projectile disappear with the
  // pilot's death before the ordinary bomb can traverse its firing lane.
  // Return to Doomsy8's authored central safety and recover through the real
  // server/client recharge path; do not inject energy, life, or position.
  for (let attempt = 0; attempt < 4; attempt += 1) {
    await waitUntil("Extreme Firefox pilot is live before bomb preparation", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
    }, 15_000);
    let metrics = (await actor.metricsState()).uiMetrics || {};
    if (metrics.inSafeZone !== true) {
      if (await returnExtremeFirefoxPilotToSafety(actor)) {
        metrics = (await actor.metricsState()).uiMetrics || {};
      }
    }
    if (metrics.inSafeZone !== true) {
      const pilotTileX = Math.floor((metrics.playerX ?? 0) / 16);
      const pilotTileY = Math.floor((metrics.playerY ?? 0) / 16);
      const directions = [
        { dx: -1, dy: 0 },
        { dx: 1, dy: 0 },
        { dx: 0, dy: -1 },
        { dx: 0, dy: 1 },
      ];
      const entries = [];
      for (let tileY = Math.max(0, pilotTileY - 40); tileY <= Math.min(1_023, pilotTileY + 40); tileY += 1) {
        for (let tileX = Math.max(0, pilotTileX - 40); tileX <= Math.min(1_023, pilotTileX + 40); tileX += 1) {
          if (authoredTiles[tileY * 1_024 + tileX] !== 171) continue;
          for (const direction of directions) {
            const outsideTileX = tileX + direction.dx;
            const outsideTileY = tileY + direction.dy;
            if (
              outsideTileX < 0 || outsideTileX >= 1_024 ||
              outsideTileY < 0 || outsideTileY >= 1_024
            ) continue;
            const outsideTile = authoredTiles[outsideTileY * 1_024 + outsideTileX];
            if (outsideTile === 171 || authoredTileIsSolid(outsideTile)) continue;
            const inside = { x: tileX * 16 + 8, y: tileY * 16 + 8 };
            const outside = { x: outsideTileX * 16 + 8, y: outsideTileY * 16 + 8 };
            if (!authoredLineIsClear(
              metrics.playerX,
              metrics.playerY,
              outside.x,
              outside.y,
            )) continue;
            entries.push({ inside, outside });
          }
        }
      }
      const entry = entries.sort((left, right) =>
        Math.hypot(metrics.playerX - left.outside.x, metrics.playerY - left.outside.y) -
        Math.hypot(metrics.playerX - right.outside.x, metrics.playerY - right.outside.y)
      )[0];
      if (!entry) continue;
      const approached = await driveClientToWorldTarget(
        actor,
        entry.outside,
        20_000,
        false,
        10,
        null,
        100,
      );
      if (!approached.reached) continue;
      const returned = await driveClientToWorldTarget(
        actor,
        entry.inside,
        10_000,
        false,
        10,
        null,
        70,
      );
      metrics = (await actor.metricsState()).uiMetrics || {};
      if (!returned.reached || metrics.inSafeZone !== true) continue;
    }
    try {
      await waitUntil("Extreme Firefox pilot recovers authored full energy in safety", async () => {
        const current = (await actor.metricsState()).uiMetrics || {};
        const maximum = current.playerMaxEnergy ?? 0;
        return current.playerAlive === true && current.serverPlayerAlive === true &&
          current.inSafeZone === true && maximum >= requiredEnergy &&
          (current.playerEnergy ?? 0) >= maximum - 1 &&
          (current.serverPlayerEnergy ?? 0) >= maximum - 1;
      }, 15_000);
      const recovered = (await actor.metricsState()).uiMetrics || {};
      return {
        lifeSeq: recovered.playerLifeSeq,
        energy: recovered.playerEnergy,
        serverEnergy: recovered.serverPlayerEnergy,
        maxEnergy: recovered.playerMaxEnergy,
        x: recovered.playerX,
        y: recovered.playerY,
        inSafeZone: recovered.inSafeZone,
      };
    } catch {
      // A lifecycle transition can overtake the real route. Retry only after
      // the next exact live authority scope instead of weakening the reserve.
    }
  }
  throw new Error("Extreme Firefox pilot could not establish full authoritative energy reserve");
}

async function attackBotsWithFirefoxInput(
  firefox,
  actor,
  server,
  playerId,
  damageOffset,
  preferredTargetId = null,
  requireLethalTargetHit = false,
  weaponMode = "bullet",
  timeoutMs = 60_000,
  preferredTargetLifeSeq = null,
) {
  const weaponTypes = weaponMode === "bomb" ? bombWeaponTypes : bulletWeaponTypes;
  // Bullet bursts use the game's held Ctrl path; Space owns the player list.
  const fireKey = weaponMode === "bomb" ? "z" : "Control";
  let targetMotion = null;
  let targetMotionReady = false;
  let stationaryBombLane = null;
  const attackStartedAt = Date.now();
  const attackDeadline = attackStartedAt + timeoutMs;
  const waitForReleasedBombSample = async () => {
    const released = (await firefoxMetricsState(firefox)).uiMetrics || {};
    if (!Number.isInteger(released.clientInputSeq)) return;
    const releasedSequence = released.clientInputSeq;
    const deadline = Date.now() + 1_500;
    while (Date.now() < deadline) {
      const current = (await firefoxMetricsState(firefox)).uiMetrics || {};
      if (current.playerAlive !== true || current.serverPlayerAlive !== true) return;
      if (Number.isInteger(current.clientInputSeq) &&
          current.clientInputSeq !== releasedSequence) {
        return;
      }
      await sleep(20);
    }
    throw new Error("Firefox bomb key-up was not sampled by a later gameplay input");
  };
  // The real wall-clock deadline is the acceptance bound. A separate pass
  // ceiling made fast Firefox/CDP sampling terminate a nominal 90-second
  // controller after roughly half that time, before a moving target could
  // finish an authored wall route and expose a clear bomb lane.
  for (let pass = 0; Date.now() < attackDeadline; pass += 1) {
    const metrics = (await firefoxMetricsState(firefox)).uiMetrics || {};
    if (Number.isInteger(metrics.selfPlayerId) && metrics.selfPlayerId !== playerId) {
      throw new Error(`Firefox combat session changed player identity: ${JSON.stringify({
        expectedPlayerId: playerId, observedPlayerId: metrics.selfPlayerId,
        lifeSeq: metrics.playerLifeSeq, weaponMode,
      })}`);
    }
    const hits = playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
    const targetHits = hits.filter((event) =>
      (preferredTargetId == null || event.victimId === preferredTargetId) &&
      (preferredTargetLifeSeq == null ||
        event.victimLifeSeq === preferredTargetLifeSeq)
    );
    const targetSatisfied = !requireLethalTargetHit
        ? targetHits.length > 0
      : weaponMode === "bullet"
        ? lethalHitWithNonlethalPrecursor(targetHits, preferredTargetId) != null
        : targetHits.some((event) => event.lethal === true);
    if (targetSatisfied) {
      return hits;
    }
    if (weaponMode === "bomb" && requireLethalTargetHit &&
        Number.isInteger(preferredTargetId) && Number.isInteger(preferredTargetLifeSeq) &&
        exactLifePrimaryBombTerminal(serverDamageEventsSince(server, 0), playerId,
          { victimId: preferredTargetId, victimLifeSeq: preferredTargetLifeSeq }).kind !== "pending") return hits;
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) {
      await sleep(100);
      continue;
    }
    if (arena === "extreme" && metrics.inSafeZone === true) {
      // This is ordinary pursuit, including the fallback after protected
      // edge staging fails. Re-entering that same edge solver can leave the
      // pilot in safety forever without one real fire edge. Leave through
      // map-aware keyboard flight and re-read the nonsafe state before fire.
      const protectedTarget = (metrics.remotePlayers || []).find((player) =>
        player.id >= botIdBase &&
        (preferredTargetId == null || player.id === preferredTargetId) &&
        (preferredTargetLifeSeq == null || player.lifeSeq === preferredTargetLifeSeq) &&
        Number.isFinite(player.positionX) && Number.isFinite(player.positionY)
      );
      if (protectedTarget == null) {
        await sleep(80);
        continue;
      }
      const targetPosition = {
        x: Number.isFinite(protectedTarget.displayX) ? protectedTarget.displayX : protectedTarget.positionX,
        y: Number.isFinite(protectedTarget.displayY) ? protectedTarget.displayY : protectedTarget.positionY,
      };
      const directLane = authoredLineIsClear(
        metrics.playerX, metrics.playerY, targetPosition.x, targetPosition.y,
      );
      const routed = directLane ? null : routedMapCounterfireWaypoint({
        metrics,
        target: { ...protectedTarget, positionX: targetPosition.x, positionY: targetPosition.y },
        authoredHullIsTraversable,
        authoredHullIsOpen,
        authoredLineIsClear,
      });
      await driveClientToWorldTarget(
        actor, directLane ? targetPosition : (routed ?? targetPosition),
        1_200, false, 24, null, 250, 0.8, 1_200,
      );
      continue;
    }
    const visibleBots = (metrics.remotePlayers || [])
      .filter((player) => player.id >= botIdBase &&
        Number.isFinite(player.positionX) && Number.isFinite(player.positionY) &&
        (preferredTargetLifeSeq == null ||
          player.lifeSeq === preferredTargetLifeSeq) &&
        !positionIsOnAuthoredSafeTile(player.positionX, player.positionY));
    // Bot-v-bot fire can wound a selected target before the human projectile
    // lands. Lock only after a human nonlethal hit, and bind the lock to that
    // exact life; an incidental first-hit kill then cannot strand the driver
    // on a dead predecessor instead of proving a visible decrement cycle.
    const openCycle = [...hits].reverse().find((hit) =>
      hit.lethal !== true &&
      (preferredTargetId == null || hit.victimId === preferredTargetId) &&
      (preferredTargetLifeSeq == null ||
        hit.victimLifeSeq === preferredTargetLifeSeq) &&
      visibleBots.some((player) =>
        player.id === hit.victimId && player.lifeSeq === hit.victimLifeSeq
      )
    );
    const target = preferredTargetId != null
      ? visibleBots.find((player) => player.id === preferredTargetId)
      : (openCycle == null ? null : visibleBots.find((player) =>
          player.id === openCycle.victimId && player.lifeSeq === openCycle.victimLifeSeq
        )) || visibleBots.sort((left, right) => weaponMode === "bullet"
          ? (right.energy ?? Number.MIN_SAFE_INTEGER) -
            (left.energy ?? Number.MIN_SAFE_INTEGER)
          : (left.energy ?? Number.MAX_SAFE_INTEGER) -
            (right.energy ?? Number.MAX_SAFE_INTEGER)
        )[0];
    if (!target || !Number.isFinite(metrics.playerX) || !Number.isFinite(metrics.playerY)) {
      if (pass % 15 === 0) {
        reportStage("fire_progress", {
          arena,
          weaponMode,
          pass,
          elapsedMs: Date.now() - attackStartedAt,
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
    const sampledAt = Date.now();
    let targetVelocityX = 0;
    let targetVelocityY = 0;
    const targetServerTick = target.lastServerPositionTick;
    const sameTargetSample = targetMotion?.id === target.id &&
      targetMotion.lifeSeq === target.lifeSeq;
    let measuredTargetVelocity = false;
    const canonicalTargetSpeed = Math.hypot(
      target.velocityX ?? Number.POSITIVE_INFINITY,
      target.velocityY ?? Number.POSITIVE_INFINITY,
    );
    const canonicalTargetVelocityReady =
      Number.isFinite(target.velocityX) && Number.isFinite(target.velocityY) &&
      canonicalTargetSpeed <= 500;
    if (canonicalTargetVelocityReady) {
      // The remote actor metric carries the exact velocity from the same
      // authoritative snapshot as position. Prefer that source-time pair to
      // wall-clock differencing: a headed debug Firefox frame can span many
      // server ticks and make a valid 320px/s bot look substantially faster.
      targetVelocityX = target.velocityX;
      targetVelocityY = target.velocityY;
      measuredTargetVelocity = true;
      targetMotionReady = true;
    } else if (sameTargetSample && targetMotion.serverTick !== targetServerTick) {
      const elapsed = (sampledAt - targetMotion.sampledAt) / 1_000;
      if (elapsed >= 0.025 && elapsed <= 1) {
        const measuredX = (target.positionX - targetMotion.x) / elapsed;
        const measuredY = (target.positionY - targetMotion.y) / elapsed;
        if (Math.hypot(measuredX, measuredY) <= 500) {
          targetVelocityX = targetMotion.velocityX * 0.45 + measuredX * 0.55;
          targetVelocityY = targetMotion.velocityY * 0.45 + measuredY * 0.55;
          measuredTargetVelocity = true;
        }
      }
    }
    if (!sameTargetSample || targetMotion.serverTick !== targetServerTick) {
      if (!sameTargetSample) targetMotionReady = false;
      if (measuredTargetVelocity) targetMotionReady = true;
      targetMotion = {
        id: target.id,
        lifeSeq: target.lifeSeq,
        serverTick: targetServerTick,
        x: target.positionX,
        y: target.positionY,
        sampledAt,
        velocityX: targetVelocityX,
        velocityY: targetVelocityY,
      };
    } else if (!canonicalTargetVelocityReady) {
      // Browser render polling is faster than authoritative remote snapshots.
      // Re-sampling an unchanged snapshot as zero velocity erases the lead
      // solution just before the key press. Preserve the last real server-tick
      // sample until a newer authoritative position arrives.
      targetVelocityX = targetMotion.velocityX;
      targetVelocityY = targetMotion.velocityY;
    }
    const dx = target.positionX - metrics.playerX;
    const dy = target.positionY - metrics.playerY;
    const distance = Math.hypot(dx, dy);
    // Settings and spawn-log velocities use tenths of pixels per second;
    // the production registry divides by ten when advancing world positions.
    // Match target/ship metrics in pixels per second, for the exact authored
    // Lancaster bullet and Warbird bomb fixtures selected by this driver.
    const projectileSpeed = arena === "extreme"
      ? (weaponMode === "bomb" ? 430 : 360)
      : 200;
    // Use the same intercept geometry as Chrome. Extreme bots can cross most
    // of the close fire lane during one projectile flight; direct aiming at
    // their current snapshot position produced accepted shots that reliably
    // passed behind them. The bounded/smoothed velocity sample above rejects
    // implausible snapshot jitter before it participates in this lead.
    const maximumFlightSeconds = weaponMode === "bomb"
      ? (arena === "extreme" ? 2.5 : 2)
      : (arena === "extreme" ? 0.65 : 5.5);
    const lead = interceptAimVector(
      dx,
      dy,
      targetVelocityX - metrics.playerVelocityX,
      targetVelocityY - metrics.playerVelocityY,
      projectileSpeed,
      maximumFlightSeconds,
    );
    const movingChaosBomb = arena === "chaos-zone" && weaponMode === "bomb" && !stationaryBotAfterHumanDamage;
    const chaosBombPlan = movingChaosBomb ? movingBombAttackPlan({
      relativeX: dx, relativeY: dy, targetVelocityX, targetVelocityY,
      playerVelocityX: metrics.playerVelocityX ?? 0, playerVelocityY: metrics.playerVelocityY ?? 0,
      projectileSpeed, maximumFlightSeconds,
      explosionRadius: bombSafetyProfile.explosionPixels * metrics.localGameplayState?.bombLevel,
      // Match the18px contact sensor, not the16px fire-only safety tile.
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
    const quantizedAim = arena === "extreme" || weaponMode === "bullet"
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
    const currentProtocolDirection = Number.isInteger(metrics.playerProtocolDirection)
      ? metrics.playerProtocolDirection
      : Math.floor(Math.fround(metrics.playerOrientation ?? 0) * 40) % 40;
    const currentDirectionAim = arena === "extreme" || weaponMode === "bullet"
      ? quantizedProjectileIntercept({
          relativeX: dx,
          relativeY: dy,
          targetVelocityX,
          targetVelocityY,
          playerVelocityX: metrics.playerVelocityX ?? 0,
          playerVelocityY: metrics.playerVelocityY ?? 0,
          projectileSpeed,
          maxTime: maximumFlightSeconds,
          fixedDirection: currentProtocolDirection,
        })
      : null;
    // The reference game spawns the bomb at the pilot position and performs an exact
    // pixel-truncated 4px weapon-box/14px ship-box contact check. If that box
    // already overlaps, the current sample is collision-safe regardless of
    // heading; do not spend a slow Firefox frame turning while the bot exits
    // contact. Every more distant shot still requires the measured intercept.
    const pointBlankBombContact = arena === "extreme" && weaponMode === "bomb" &&
      explosiveContactOverlapsHull({
        weaponX: metrics.playerX,
        weaponY: metrics.playerY,
        shipX: target.positionX,
        shipY: target.positionY,
      });
    const aim = chaosBombPlan?.candidate ?? (pointBlankBombContact
      ? { x: dx, y: dy }
      : quantizedAim ?? lead);
    const desired = Number.isFinite(aim?.orientation)
      ? aim.orientation
      : Math.atan2(aim.x, -aim.y) / (Math.PI * 2);
    const wrappedDesired = ((desired % 1) + 1) % 1;
    const current = (((metrics.playerOrientation ?? 0) % 1) + 1) % 1;
    const delta = ((wrappedDesired - current + 1.5) % 1) - 0.5;
    const aimTolerance = 0.013;
    // Moving Chaos uses the existing two-second forecast plus explicit owner
    // clearance. The stationary fixture keeps its140px lane; all profiles
    // still require actual exact-life server-owned bomb damage and death.
    const fireRange = weaponMode === "bomb"
      ? (chaosBombPlan?.fireRange ?? (arena === "extreme" ? extremeBombFiringDistance : 140))
      // Chaos BulletAliveTime=550 ticks at 200px/s is ~1,100px of
      // authored travel. Extreme's 360px/s bullet travels 234px in its
      // authored 650ms life before accounting for inherited velocity. The
      // quantized solver also checks the moving target's contact envelope;
      // range alone never authorizes the shot. This avoids the same-speed
      // 130px chase that made
      // Firefox's real-key pilot orbit instead of exercising repeated
      // authoritative hits.
      : (arena === "extreme" ? 234 : 400);
    // Chaos' level-2 proximity bomb is intentionally suppressed while an
    // enemy is within its authored 64-pixel safety radius. Keep this real-key
    // pilot in a narrow valid lane rather than repeatedly issuing Z at a
    // distance where the game must reject it.
    const bombSafetyFloor = weaponMode === "bomb"
      ? botBombSafetyFloor(bombSafetyProfile, metrics.localGameplayState?.bombLevel)
      : 0;
    const targetSpeed = Math.hypot(targetVelocityX, targetVelocityY);
    const lineIsClear = authoredLineIsClear(
      metrics.playerX,
      metrics.playerY,
      target.positionX,
      target.positionY,
    );
    // Stationarity is armed by the first exact-life human hit and persists
    // across later attack/retry boundaries. Restricting this witness to the
    // current damage offset forgot the preceding bullet phase, so the pilot
    // treated a server-stationary bot as a moving pursuit target and orbited
    // it instead of entering the deterministic bomb lane.
    const humanDamageObservedForTarget = playerDamageToBots(server, playerId, 0).some((event) =>
      event.victimId === target.id && event.victimLifeSeq === target.lifeSeq
    );
    const stationaryDamagedChaosBomb = stationaryBotAfterHumanDamage &&
      arena === "chaos-zone" &&
      weaponMode === "bomb" &&
      lineIsClear &&
      humanDamageObservedForTarget &&
      targetMotionReady &&
      targetSpeed <= 3.5;
    const stationaryDamagedExtremeBullet = stationaryBotAfterHumanDamage &&
      arena === "extreme" &&
      weaponMode === "bullet" &&
      lineIsClear &&
      humanDamageObservedForTarget &&
      targetMotionReady &&
      targetSpeed <= 3.5;
    const stationaryBombHoldingPointCandidate = stationaryChaosBombHoldingPoint({
      fixtureActive: stationaryBotAfterHumanDamage,
      arena,
      weaponMode,
      lineIsClear,
      humanDamageObserved: humanDamageObservedForTarget,
      targetMotionReady,
      targetSpeed,
      playerX: metrics.playerX,
      playerY: metrics.playerY,
      targetX: target.positionX,
      targetY: target.positionY,
      hullIsOpen: authoredHullIsOpen,
      firingLineIsClear: authoredLineIsClear,
    });
    if (stationaryBombHoldingPointCandidate == null) {
      stationaryBombLane = null;
    } else if (
      stationaryBombLane == null ||
      stationaryBombLane.targetId !== target.id ||
      stationaryBombLane.targetLifeSeq !== target.lifeSeq ||
      Math.hypot(
        stationaryBombLane.targetX - target.positionX,
        stationaryBombLane.targetY - target.positionY,
      ) > 4
    ) {
      stationaryBombLane = {
        targetId: target.id,
        targetLifeSeq: target.lifeSeq,
        targetX: target.positionX,
        targetY: target.positionY,
        point: stationaryBombHoldingPointCandidate,
      };
    }
    const stationaryBombHoldingPoint = stationaryBombLane?.point ?? null;
    if (stationaryBombHoldingPoint != null) {
      // Retain one controlled 73..79px point for the exact target life while
      // it remains stationary. Recomputing the radial point after every
      // crossing moves it to the opposite side of the bot and creates an
      // artificial orbit.
      const stationaryLaneRoute = await driveClientToWorldTarget(
        actor,
        stationaryBombHoldingPoint,
        6_000,
        true,
        3,
        null,
        80,
        0.3,
        3_000,
      );
      if (!stationaryLaneRoute.reached) {
        const finalMetrics = stationaryLaneRoute.final || {};
        reportStage("stationary_bomb_lane_route", {
          reached: false,
          reason: stationaryLaneRoute.reason,
          bestDistance: stationaryLaneRoute.bestDistance,
          pulses: stationaryLaneRoute.pulses,
          target: stationaryLaneRoute.target,
          playerLifeSeq: finalMetrics.playerLifeSeq,
          playerAlive: finalMetrics.playerAlive,
          serverPlayerAlive: finalMetrics.serverPlayerAlive,
          playerX: finalMetrics.playerX,
          playerY: finalMetrics.playerY,
          pilotSpeed: Math.hypot(
            finalMetrics.playerVelocityX ?? Number.NaN,
            finalMetrics.playerVelocityY ?? Number.NaN,
          ),
        });
      }
      targetMotion = null;
      targetMotionReady = false;
      continue;
    }
    const quantizedShotIsFeasible =
      (arena !== "extreme" && weaponMode === "bomb") ||
      pointBlankBombContact || (
        quantizedAim?.feasible === true &&
        quantizedAim.missDistance <= (weaponMode === "bullet" ? 12 : 6)
      );
    const currentExtremeHeadingCanHit = arena !== "extreme" ||
      pointBlankBombContact || (
        currentDirectionAim?.feasible === true &&
        currentDirectionAim.missDistance <= 6
      );
    const targetInFireRange = metrics.inSafeZone !== true &&
      distance <= fireRange &&
      distance >= bombSafetyFloor &&
      lineIsClear &&
      // Extreme BulletAliveTime is only 650ms. Do not count a direct shot
      // against a faster relative target as a valid keyboard attempt; use
      // the ordinary movement controller below until the exact intercept
      // equation has a positive solution inside that lifetime.
      quantizedShotIsFeasible &&
      (chaosBombPlan ? (targetMotionReady && chaosBombPlan.candidate != null) :
        (weaponMode !== "bomb" ||
          (targetMotionReady && lead.feasible) ||
          (arena === "chaos-zone" && distance <= 80) ||
          pointBlankBombContact));
    const localGameplay = metrics.localGameplayState || {};
    const bombCooldown = Number(localGameplay.bombCooldown ?? metrics.bombCooldown);
    const bombCooldownReady = weaponMode !== "bomb" ||
      (Number.isFinite(bombCooldown) && bombCooldown <= 0);
    if (pass % 15 === 0) {
      reportStage("fire_progress", {
        arena,
        weaponMode,
        pass,
        elapsedMs: Date.now() - attackStartedAt,
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        inSafeZone: metrics.inSafeZone,
        targetId: target.id,
        targetLifeSeq: target.lifeSeq,
        distance,
        orientationDelta: delta,
        lineIsClear,
        targetSpeed,
        targetMotionReady,
        interceptFeasible: lead.feasible,
        quantizedDirection: quantizedAim?.direction ?? null,
        quantizedMissDistance: quantizedAim?.missDistance ?? null,
        currentProtocolDirection,
        currentDirectionMissDistance: currentDirectionAim?.missDistance ?? null,
        currentExtremeHeadingCanHit,
        targetInFireRange,
        stationaryDamagedChaosBomb,
        stationaryDamagedExtremeBullet,
        bombCooldown,
        bombCooldownReady,
        pilotSpeed: Math.hypot(
          metrics.playerVelocityX ?? 0,
          metrics.playerVelocityY ?? 0,
        ),
        damageEvents: hits.length,
        semanticInputCounts: metrics.semanticInputCounts,
      });
    }
    const pilotSpeed = Math.hypot(
      metrics.playerVelocityX ?? 0,
      metrics.playerVelocityY ?? 0,
    );
    // Bombs inherit the pilot's canonical velocity. Preserve that authored
    // velocity when the measured intercept is feasible; braking a Lancaster
    // below a faster receding bot made a valid solution impossible. Chaos
    // retains its narrower controlled lane, while Extreme admits the ship's
    // complete authored speed envelope just like the Chrome pilot.
    const brakingThreshold = stationaryDamagedChaosBomb || stationaryDamagedExtremeBullet
      ? 3.5
      : weaponMode === "bomb" && arena === "extreme"
        ? 160
        : arena === "extreme"
          ? 360
          : 160;
    if (!chaosBombPlan && targetInFireRange && pilotSpeed > brakingThreshold) {
      // Enter the firing lane under control. Genuine counter-thrust makes an
      // inherited-velocity bomb assertion about the weapon path instead of a
      // fly-by or self-splash. After a human hit freezes the Extreme fixture
      // target, it also keeps the real-key bullet pilot from crossing that
      // stationary target during a short burst and turning later rounds into
      // misses before the same exact life is lethally finished.
      await brakeClientVelocity(actor, 2_500);
      targetMotion = null;
      targetMotionReady = false;
      continue;
    }
    if (
      arena === "extreme" && weaponMode === "bullet" &&
      distance <= fireRange && lineIsClear && !lead.feasible
    ) {
      // Lancaster can enter its complete 234px authored bullet envelope with
      // enough lateral ship velocity that no interception exists within the
      // 650ms BulletAliveTime. Chasing the already-close target accelerates
      // through the lane and repeats the same impossible shot. Use genuine
      // counter-thrust to establish a solvable canonical velocity, then
      // re-sample and aim; the server-owned hit remains the acceptance gate.
      await brakeClientVelocity(actor, 1_500);
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
      // This leg has one authoritative bot, which already navigates toward
      // the human. Stop the Firefox ship with genuine counter-thrust and let
      // server AI close to its authored 96px aim brake instead of chasing the
      // same moving target and creating an unbounded orbit.
      await brakeClientVelocity(actor, 1_500);
      targetMotion = null;
      targetMotionReady = false;
      await sleep(200);
      continue;
    }
    if (stationaryChaosBombLane.action === "wait") {
      // Preserve the preceding authoritative sample so the next bot snapshot
      // can establish a real lead while the human remains stopped.
      await sleep(80);
      continue;
    }
    if (!lineIsClear || chaosBombPlan?.pathBlocked) {
      const routeWaypoint = routedMapCounterfireWaypoint({
        metrics,
        target,
        authoredHullIsTraversable,
        authoredHullIsOpen,
        authoredLineIsClear,
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
      if (step.keys.length) await holdFirefoxKeys(firefox, step.keys, step.holdMs);
      else await sleep(step.holdMs);
      continue;
    }
    if (bombSafetyFloor > 0 && distance < bombSafetyFloor) {
      const retreatWaypoint = routedMapCounterfireWaypoint({
        metrics,
        target,
        authoredHullIsTraversable,
        authoredHullIsOpen,
        authoredLineIsClear,
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
    // Proximity owns radial contact after launch; it does not make a bomb
    // fired as much as a quarter-turn away into aimed combat evidence.
    // Require the same half-wire-bin heading tolerance in both arenas.
    const bombFireTolerance = aimTolerance;
    if (
      weaponMode === "bomb" && targetInFireRange && bombCooldownReady &&
      (chaosBombPlan ? (chaosBombPlan.candidate != null &&
        chaosBombPlan.current.direction === chaosBombPlan.candidate.direction && chaosBombPlan.current.safeToAttempt) :
        (pointBlankBombContact || (
          Math.abs(delta) <= bombFireTolerance && currentExtremeHeadingCanHit
        )))
    ) {
      // Debug Firefox can render below 10 Hz while the server emits complete
      // simulation traces. A 100ms Z+turn chord can therefore fall entirely
      // between two WASM input samples. Hold only Z across several render
      // samples once the radial proximity weapon is inside a clear authored
      // safety lane. The exact server-owned hit/death assertion below still
      // rejects a miss. Extreme retains Chrome's narrow measured-lead
      // threshold; the former quarter-turn allowance launched accepted bombs
      // up to 90 degrees off-axis. The bomb remains an ordinary edge-triggered
      // browser action. Chaos
      // deliberately stays outside the authored proximity BombSafety radius
      // below, so a real key edge cannot be silently suppressed by design.
      if (chaosBombPlan) reportStage("moving_bomb_key_attempt", {
        playerLifeSeq: metrics.playerLifeSeq, targetId: target.id, targetLifeSeq: target.lifeSeq,
        playerEnergy: metrics.playerEnergy, plan: chaosBombPlan,
        meaning: "physical key attempt; authoritative acceptance and damage checked separately",
      });
      await holdFirefoxKeys(firefox, [fireKey], 420);
      await waitForReleasedBombSample();
    } else if (
      weaponMode === "bomb" && targetInFireRange &&
      Math.abs(delta) <= bombFireTolerance && !bombCooldownReady
    ) {
      // An edge emitted during the authored cooldown is intentionally inert.
      // Hold the already-proved geometry without thrust and wait for the
      // ordinary client clock before producing the next genuine Z edge.
      await sleep(40);
    } else if (targetInFireRange && (Math.abs(delta) > aimTolerance ||
      (chaosBombPlan && chaosBombPlan.current.direction !== chaosBombPlan.candidate?.direction))) {
      const turnMs = Math.min(240, Math.max(25, Math.abs(delta) / 0.625 * 700));
      const keys = [delta > 0 ? "\uE014" : "\uE012"];
      if (
        targetInFireRange &&
        weaponMode === "bullet" && Math.abs(delta) <= 0.08
      ) {
        keys.unshift(fireKey);
      }
      await holdFirefoxKeys(
        firefox,
        keys,
        Math.min(turnMs, keys.length > 1 ? 100 : turnMs),
      );
    } else if (!targetInFireRange) {
      // Match the Chrome pilot's velocity-aware approach. Blindly thrusting
      // until the distance threshold left Firefox moving near ship max speed,
      // which made otherwise valid bomb launches miss or circle into self.
      const desiredRange = weaponMode === "bomb"
        ? (arena === "extreme" ? 160 : 76)
        : (arena === "extreme" ? 110 : 96);
      const retreatingFromBombSafety = bombSafetyFloor > 0 &&
        distance < bombSafetyFloor;
      if (arena === "extreme") {
        if (weaponMode === "bomb" && distance <= fireRange && lineIsClear) {
          // Preserve the last authoritative sample until a newer server tick
          // establishes a real intercept. Resetting it on every close-range
          // poll left the pilot permanently velocity-blind and launched slow
          // bombs behind a faster receding bot.
          await sleep(50);
          continue;
        }
        // Reuse the checked-in real-key feedback controller against a short
        // lived authoritative target sample. The prior hand-rolled velocity
        // controller ignored the bot's lateral motion and could orbit away
        // for the entire retry budget without ever emitting a fire edge.
        await driveClientToWorldTarget(
          actor,
          { x: target.positionX, y: target.positionY },
          900,
          false,
          desiredRange,
          null,
          350,
          1,
        );
        targetMotion = null;
        targetMotionReady = false;
        continue;
      }
      const closingDistance = retreatingFromBombSafety
        ? bombSafetyFloor + 8 - distance
        : Math.max(metrics.inSafeZone === true ? 80 : 0, distance - desiredRange);
      const closingSpeed = Math.sqrt(2 * 180 * closingDistance);
      // Match the actual Extreme chase envelope: bots can reach 320 px/s,
      // so the old 130 px/s cap could never close a receding target inside
      // Lancaster's 210 px firing lane. The lane branch still uses genuine
      // movement/fire keys rather than moving authority directly.
      const desiredSpeed = arena === "extreme"
        ? Math.min(350, Math.max(300, closingSpeed))
        : retreatingFromBombSafety
          ? Math.min(110, Math.max(60, closingSpeed * 0.28))
          : Math.min(300, Math.max(
              closingDistance > 96 ? 220 : 80,
              closingSpeed,
            ));
      const approachDirection = retreatingFromBombSafety ? -1 : 1;
      let desiredVelocityX = distance > 0.01
        ? dx / distance * desiredSpeed * approachDirection
        : 0;
      let desiredVelocityY = distance > 0.01
        ? dy / distance * desiredSpeed * approachDirection
        : 0;
      if (weaponMode === "bomb" && !retreatingFromBombSafety) {
        // Close relative to the moving target. Point-only pursuit discarded
        // its lateral velocity and settled into a mutual orbit outside the
        // firing lane. These are already-observed snapshot velocities, not
        // predicted packet/state injection. Keep the existing 300 px/s cap.
        desiredVelocityX += targetVelocityX;
        desiredVelocityY += targetVelocityY;
        const desiredMagnitude = Math.hypot(desiredVelocityX, desiredVelocityY);
        if (desiredMagnitude > 300) {
          desiredVelocityX *= 300 / desiredMagnitude;
          desiredVelocityY *= 300 / desiredMagnitude;
        }
      }
      const accelX = desiredVelocityX - (metrics.playerVelocityX ?? 0);
      const accelY = desiredVelocityY - (metrics.playerVelocityY ?? 0);
      const closeOrientation = ((Math.atan2(accelX, -accelY) / (Math.PI * 2)) + 1) % 1;
      const closeDelta = signedOrientationDelta(current, closeOrientation);
      // Chaos Warbird's ordinary starting speed is about the same as the
      // authoritative bot's pursuit speed. A keyboard pilot that uses only
      // ArrowUp can therefore follow a receding bot forever without entering
      // the authored bomb lane. Use the real Shift+thrust Afterburner while
      // closing that lane, exactly as a human pilot would. This remains an
      // ordinary WebDriver chord: the client/server still own its energy
      // cost, speed cap, position admission, bomb edge, and eventual damage.
      const useAfterburner = weaponMode === "bomb" &&
        !retreatingFromBombSafety && distance > fireRange;
      if (pass % 15 === 0) {
        reportStage("pursuit_control", {
          targetId: target.id, targetLifeSeq: target.lifeSeq,
          distance, desiredSpeed, desiredVelocityX, desiredVelocityY,
          targetVelocityX, targetVelocityY,
          velocityErrorX: accelX, velocityErrorY: accelY, closeDelta,
          action: Math.abs(closeDelta) > 0.025 ? "turn" : "thrust",
          afterburner: Math.abs(closeDelta) <= 0.025 && useAfterburner,
          playerLifeSeq: metrics.playerLifeSeq,
        });
      }
      if (Math.abs(closeDelta) > 0.025) {
        const turnMs = Math.min(220, Math.max(25, Math.abs(closeDelta) / 0.625 * 650));
        // Align with the required velocity change before applying thrust.
        // Combining turn and thrust can spend acceleration across or away
        // from that change instead of establishing the requested velocity.
        // Match the shared route driver's rotate-then-thrust policy; keep
        // genuine afterburner on the aligned approach below.
        await holdFirefoxKeys(
          firefox,
          [closeDelta > 0 ? "\uE014" : "\uE012"],
          Math.min(turnMs, 100),
        );
      } else {
        await holdFirefoxKeys(
          firefox,
          [...(useAfterburner ? ["\uE008"] : []), "\uE013"],
          distance < 320 ? 55 : 100,
        );
      }
    } else {
      // Hold only the requested weapon once the ordinary-keyboard pilot is
      // aligned in a short unobstructed lane. The previous Extreme driver
      // added thrust to every burst, crossed the target at speed, and landed
      // only one round before the bot killed the pilot and regenerated. This
      // remains genuine WebDriver input; position, energy, and damage are all
      // still authored by the normal client/server path.
      // Re-solve the lead between bounded genuine-key bursts so later rounds
      // are aimed at the current authoritative target. The old 180ms Extreme
      // tap admitted only one bullet per approach; authored 300 energy/s
      // recharge could erase two otherwise-valid hits while the pilot spent
      // several seconds reacquiring a wall-crossing target. The Extreme
      // exact-life fixture deliberately stops translating shortly after its
      // first real human hit. Release after seven authored 60ms fire intervals
      // so Firefox can observe that authoritative motion boundary and solve a
      // fresh intercept instead of spending 2.2 seconds firing along the
      // predecessor trajectory. Chaos retains its complete longer burst.
      // Bombs are edge-triggered. A long Z hold emits only one bomb and gives
      // a regenerating target an idle second before the next real key edge.
      // Release promptly and re-solve the authoritative lane; bullets retain
      // their complete authored burst windows.
      const resolvedFireHoldMs = weaponMode === "bomb"
        ? 420
        : arena === "extreme"
          ? extremeOpeningBulletBurstMs
          : 1_100;
      await holdFirefoxKeys(firefox, [fireKey], resolvedFireHoldMs);
      if (weaponMode === "bomb") await waitForReleasedBombSample();
    }
    // WebDriver returns as soon as the key-up action is dispatched. Leave at
    // least two 60 Hz render samples before the next action so the WASM input
    // edge detector observes the released state; otherwise a tight Firefox
    // command loop can merge every later Control press into the first hold.
    await sleep(40);
  }
  return playerWeaponDamageToBots(server, playerId, damageOffset, weaponTypes);
}

async function main() {
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const serverPort = await freePort();
  const geckodriverPort = await freePort();
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
  const firefox = startGeckodriver(geckodriverPort, "bot-authority-firefox");
  const networkProfileQuery = networkProfile
    ? `&network_profile=${encodeURIComponent(networkProfile)}&network_profile_seed=${networkProfileSeed}`
    : "";
  const actorUrl = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${serverPort}/v1/rtc/offer`)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&packetlog_bytes=1&trace=1&name=ffbotactor${networkProfileQuery}`;
  const actor = {
    freezeQaTrace: (options) => freezeQaTrace(firefox, options),
    pageState: () => pageState(firefox),
    metricsState: () => firefoxMetricsState(firefox),
    evaluate: (expression) => execute(firefox, `return ${expression};`),
    screenshot: () => screenshot(firefox),
    keyDown: (key) => firefoxKeyDown(firefox, key),
    keyUp: (key) => firefoxKeyUp(firefox, key),
    releaseKeys: async () => {
      // Gecko's DELETE /actions can clear the remote input source without
      // always delivering a DOM keyup for a key held across a lifecycle
      // boundary. Emit explicit genuine WebDriver key-up actions first so
      // the game observes a neutral sample instead of retaining (for example)
      // ArrowRight in inputBits after respawn.
      for (const key of ["Control", "Space", "z", "\uE013", "\uE012", "\uE014"]) {
        await firefoxKeyUp(firefox, key).catch(() => {});
      }
      await releaseFirefoxKeys(firefox);
    },
    press: (key, durationMs = 45) => holdFirefoxKeys(firefox, [key], durationMs),
    hold: (key, durationMs = 45) => holdFirefoxKeys(firefox, [key], durationMs),
    performChord: (first, second, firstOnlyMs, chordMs, releaseOrder) =>
      holdThenChordFirefoxKeys(
        firefox,
        first,
        second,
        firstOnlyMs,
        chordMs,
        releaseOrder,
      ),
  };

  const projectilePresentationStates = [];
  const facingCollectionStates = [];

  try {
    await Promise.all([
      waitForServerListening(serverPort, server.child, server.logs),
      waitForGeckodriver(firefox),
    ]);
    const observedLoadoutFixture = await assertBotAuthorityLoadout(serverPort, loadoutFixture, joinOptions.initialZone);
    const agedJoinEvidence = await waitForAgedServerJoin(server, serverPort, joinOptions);
    await createSession(firefox);
    // Bind the session's collector before navigation. Keep the genuine-key
    // adapter's evaluator: it also accepts statement sequences, unlike the
    // artifact-only evaluator's parenthesized expression contract.
    const artifacts = firefoxArtifactClient(firefox);
    actor.events = artifacts.events;
    actor.consoleEvidence = artifacts.consoleEvidence;
    await openClient(firefox, actorUrl);
    const zonePublicationTimeoutMs = networkProfile === "slow"
      ? 420_000
      : networkProfile === "mild"
        ? 240_000
        : 60_000;
    await waitUntil("Firefox bot actor authoritative lifecycle ready", async () => {
      const metrics = (await firefoxMetricsState(firefox)).uiMetrics || {};
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
    // The authoritative join already installs the default playable ship.
    // Do not reuse the shallow smoke helper here: it presses `2` four times
    // before map/security readiness, and a delayed edge can become a real
    // Javelin lifecycle request just as the Extreme scenario selects its
    // explicit Lancaster. Correctness evidence must own exactly the key
    // transitions it claims.
    const combatShipEvidence = arena === "extreme" ? await selectPlayerShip({
      read: async () => (await firefoxMetricsState(firefox)).uiMetrics || {},
      press: (key, ms) => holdFirefoxKeys(firefox, [key === "Escape" ? "\uE00C" : key], ms),
      waitUntil, ship: 6, label: "Extreme Firefox combat Lancaster",
    }) : null;
    reportStage("joined");
    const initial = await firefoxMetricsState(firefox);
    const playerId = initial.uiMetrics?.selfPlayerId;
    const initialRespawnSeq = initial.uiMetrics?.playerRespawnSeq ?? 0;
    // Random joins can begin outside published/radar membership. Establish
    // those real witnesses first; the original20sec assertion remains below.
    // The later sprite stage separately retains its300px encounter geometry.
    const initialReadinessEncounter = arena === "chaos-zone" && expectedBotCount > 0
      ? await approachLiveBotEncounter({
        actor, server, botIds: Array.from({ length: expectedBotCount }, (_, index) => botIdBase + index),
        requireEveryBot: true, allowSafeSource: true,
        readiness: "radar",
        label: "Chaos Firefox initial radar readiness",
      }) : null;
    const botIds = new Set();
    const snapshotBotIds = new Set();
    const radarBotIds = new Set();
    await waitUntil("Firefox sees both server bots through snapshots and radar", async () => {
      const metrics = (await firefoxMetricsState(firefox)).uiMetrics || {};
      for (const bot of metrics.remotePlayers || []) {
        if (bot.id >= botIdBase && Number.isInteger(bot.lifeSeq)) botIds.add(bot.id);
      }
      for (const actor of metrics.authoritativeSnapshotActors || []) {
        if (actor.id >= botIdBase && Number.isInteger(actor.lifeSeq)) snapshotBotIds.add(actor.id);
      }
      for (const id of metrics.radarPlayerIds || []) {
        if (id >= botIdBase) radarBotIds.add(id);
      }
      return botIds.size >= expectedBotCount &&
        snapshotBotIds.size >= expectedBotCount && radarBotIds.size >= expectedBotCount;
    }, 20_000);
    const orderedBotIds = [...botIds].sort((left, right) => left - right);
    await installBotMotionCapture(actor, orderedBotIds);
    actor.botRespawnSearchPoint = botSpawnFixture ? { x: 8200, y: 8200 } : null;
    actor.prepareBotObservationOrigin = botSpawnFixture ? async (deadline) => {
      const m = (await actor.metricsState()).uiMetrics;
      return prepareKeyboardLifecycleOrigin({ actor, server, playerId: m.selfPlayerId,
        initialLifeSeq: m.playerLifeSeq, geometry: { authoredHullIsTraversable }, deadline });
    } : null;
    actor.presentationState = (startedAt = 0, includePacketAudit = false) =>
      captureBotPresentationState(actor, orderedBotIds, startedAt, includePacketAudit);
    await firefoxKeyDown(firefox, "Space");
    await waitUntil("Firefox held-Space roster contains both bots", async () => {
      const metrics = (await firefoxMetricsState(firefox)).uiMetrics || {};
      return metrics.scoreboardVisible === true && orderedBotIds.every((id) =>
        (metrics.scoreboardPlayerIds || []).includes(id)
      );
    }, 5_000);
    await firefoxKeyUp(firefox, "Space");

    const feelEncounter = arena === "chaos-zone" ? await approachLiveBotEncounter({
      actor, server, botIds: orderedBotIds, requireEveryBot: true, allowSafeSource: true,
      label: "Chaos Firefox sprite observation",
    }) : null;
    const traceStartedAt = Date.now();
    const damageOffset = serverDamageBoundary(server);
    await sleep(arena === "extreme" ? 18_000 : 10_000);
    const traceState = await actor.presentationState(traceStartedAt, true);
    const traceBotDamage = botDamageToPlayer(server, playerId, damageOffset);
    const botFeelEvidence = assertFirefoxBotTrace(
      traceState.metricsTrace,
      orderedBotIds,
      traceStartedAt,
      traceBotDamage,
      botDoorRelocationEvidence(server.logs)
    );
    const botSpriteEvidence = assertFirefoxBotSpriteTrace(
      traceState.remoteSpriteTrace,
      orderedBotIds,
      traceStartedAt,
    );
    // Retain the bounded render/metrics windows around real bot fire. Firefox
    // can spend long enough in the human weapon and respawn cycles for its
    // final ring buffer to evict the earlier projectile/owner presentation.
    projectilePresentationStates.push(traceState);
    const snapshotOnlyPresentation = assertSnapshotOnlyBotPresentation(traceState);
    const objectiveEvidence = assertObjectivesDisabled(traceState.uiMetrics);
    const botRepelEvidence = arena === "chaos-zone"
      ? await proveAuthoritativeBotRepel({
          actor,
          server,
          playerId,
          botIds: orderedBotIds,
          positionIsOnAuthoredSafeTile,
          authoredHullIsOpen,
          label: "Chaos Firefox",
        })
      : null;
    if (botRepelEvidence != null) {
      projectilePresentationStates.push(await actor.presentationState());
    }
    reportStage("presentation_trace_complete", { botIds: orderedBotIds });
    const motionSoakEvidence = process.env.ZUBSPACE_E2E_MOTION_SOAK_MS
      ? await proveRemoteMotionSoak(actor, orderedBotIds,
          Number(process.env.ZUBSPACE_E2E_MOTION_SOAK_MS), reportStage,
          { server, turnRight: () => holdFirefoxKeys(firefox, ["\uE014"], 150) })
      : null;

    // Aim-lock bots can kill the deliberately stationary test pilot during the
    // ten-second presentation trace. Bind the movement proof to one live life
    // and retry if combat crosses a death boundary while WebDriver holds the
    // keys; a single key gesture issued while dead is intentionally ignored by
    // the client and is not evidence that Firefox input failed.
    let movementBefore = null;
    let movementAfter = null;
    await waitUntil("Firefox keyboard thrust produces authoritative movement", async () => {
      const before = await firefoxMetricsState(firefox);
      const beforeMetrics = before.uiMetrics || {};
      if (beforeMetrics.playerAlive !== true || beforeMetrics.serverPlayerAlive !== true) {
        return false;
      }
      await holdFirefoxKeys(firefox, ["\uE013", "\uE014"], 550);
      const after = await firefoxMetricsState(firefox);
      const afterMetrics = after.uiMetrics || {};
      const stayedInLife = afterMetrics.playerAlive === true &&
        afterMetrics.serverPlayerAlive === true &&
        afterMetrics.playerRespawnSeq === beforeMetrics.playerRespawnSeq;
      const displacement = Math.hypot(
        (afterMetrics.playerX ?? 0) - (beforeMetrics.playerX ?? 0),
        (afterMetrics.playerY ?? 0) - (beforeMetrics.playerY ?? 0),
      );
      if (!stayedInLife || displacement < 12) return false;
      movementBefore = before;
      movementAfter = after;
      return true;
    }, 15_000);
    const firefoxMovement = {
      from: {
        x: movementBefore.uiMetrics?.playerX,
        y: movementBefore.uiMetrics?.playerY,
      },
      to: {
        x: movementAfter.uiMetrics?.playerX,
        y: movementAfter.uiMetrics?.playerY,
      },
    };
    const winsBeforeCombat = movementAfter.uiMetrics?.selfWins ?? 0;
    // Keep Extreme's moving-target bullet proof and stationary direct-bomb
    // proof as distinct exact-life cycles. Stopping after the first bullet
    // decrement makes the bomb driver chase the moving predecessor and no
    // longer proves the plan's controlled stationary geometry scenario.
    const requireStandaloneBulletCycle = true;
    // The two-bot soak retains both ordinary roles. Bind the controlled
    // Extreme combat cycle to the human pursuer, as Chaos already does for
    // bombs: the roaming bot can respawn behind walls outside every authored
    // safe-edge firing route. Choosing whichever bot happens to pass first
    // made the subsequent stationary geometry fixture unreachable, with no
    // bomb ever fired. Do not change either bot's gameplay or the hit checks.
    const combatTargetId = arena === "extreme" ? botIdBase : null;
    reportStage("movement_complete", { winsBeforeCombat });
    let bulletHits = [];
    let humanBulletLethalHit = null;
    let completeBulletLethalHit = null;
    const bulletProjectileOffset = parseServerProjectileEvents(server.logs).length;
    for (let attempt = 0; attempt < 4 && humanBulletLethalHit == null; attempt += 1) {
      let stagedTargetId = combatTargetId;
      if (arena === "extreme") {
        const staged = await stageExtremeFirefoxCombatEncounter(actor, "Control", combatTargetId);
        // The authored safe edge is a useful bounded opening, but a moving
        // bot does not owe the browser a solvable intercept at one chosen
        // boundary. If that setup cannot rotate/fire in time, continue with
        // the same genuine-key map-aware pursuit used by Chrome instead of
        // spending every attempt without emitting a projectile.
        stagedTargetId = staged ? staged.botId : combatTargetId;
      }
      await attackBotsWithFirefoxInput(
        firefox,
        actor,
        server,
        playerId,
        damageOffset,
        stagedTargetId,
        requireStandaloneBulletCycle,
        "bullet",
      );
      bulletHits = playerWeaponDamageToBots(
        server,
        playerId,
        damageOffset,
        bulletWeaponTypes,
      ).filter((hit) => combatTargetId == null || hit.victimId === combatTargetId);
      completeBulletLethalHit = lethalHitWithNonlethalPrecursor(bulletHits);
      humanBulletLethalHit = requireStandaloneBulletCycle
        ? completeBulletLethalHit
        : completeBulletLethalHit || bulletHits.find((event) =>
            (stagedTargetId == null || event.victimId === stagedTargetId) &&
            event.lethal !== true && event.energyAfter < event.energyBefore
          ) || null;
    }
    const ownedBulletProjectiles = parseServerProjectileEvents(server.logs)
      .slice(bulletProjectileOffset)
      .filter((event) =>
        event.type === "server_projectile_spawned" &&
        event.ownerId === playerId &&
        bulletWeaponTypes.has(event.weapon & 0x1f)
      );
    if (ownedBulletProjectiles.length === 0) {
      throw new Error(`Firefox keyboard bullet staging produced no authoritative projectile: ${JSON.stringify({
        arena,
        diagnostics: lastExtremeFirefoxStageDiagnostics,
      })}`);
    }
    await waitUntil("Firefox browser-owned bullet tail reaches hit or authored expiry", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return ownedProjectileTailSettled({
        projectileEvents: parseServerProjectileEvents(server.logs)
          .slice(bulletProjectileOffset),
        ownerId: playerId,
        latestServerTick: metrics.latestServerTick,
        weaponTypes: bulletWeaponTypes,
      });
    }, 10_000);
    // The real-key attack loop can return on Extreme's first visible energy
    // decrement while later rounds from the same accepted burst are still in
    // flight. Classify the cycle only after the exact server-owned projectile
    // tail is resolved, otherwise the bomb phase can bind a dead predecessor
    // life and fail before the authoritative respawn exists.
    bulletHits = playerWeaponDamageToBots(
      server,
      playerId,
      damageOffset,
      bulletWeaponTypes,
    ).filter((hit) => combatTargetId == null || hit.victimId === combatTargetId);
    completeBulletLethalHit = lethalHitWithNonlethalPrecursor(bulletHits);
    humanBulletLethalHit = requireStandaloneBulletCycle
      ? completeBulletLethalHit
      : completeBulletLethalHit || bulletHits.find((event) =>
          event.lethal !== true && event.energyAfter < event.energyBefore
        ) || null;
    if (bulletHits.length === 0) {
      throw new Error("Firefox keyboard fire produced no server-authoritative bot damage");
    }
    if (!humanBulletLethalHit) {
      throw new Error(requireStandaloneBulletCycle
        ? "Firefox human bullet fire did not visibly damage then lethally kill one bot life"
        : "Firefox human bullet fire did not visibly reduce the Extreme bot life before the bomb cycle");
    }
    // Extreme ordinarily needs only a browser-visible bullet decrement before
    // the bomb-owned terminal. A real multifire burst can nevertheless own
    // the complete death before the driver releases Control. Treat that
    // stronger outcome as a full exact-life cycle and bind the later bomb to
    // the proved successor instead of continuing to target the dead life.
    const bulletOwnsCompleteCycle = requireStandaloneBulletCycle ||
      completeBulletLethalHit != null;
    const bulletCycle = bulletOwnsCompleteCycle
      ? await provePresentedBotCombatCycle({
          actor,
          server,
          playerId,
          playerName: new URL(actorUrl).searchParams.get("name"),
          hits: bulletHits,
          lethalHit: completeBulletLethalHit || humanBulletLethalHit,
          winsBefore: winsBeforeCombat,
          label: `${arena} Firefox bullet cycle`,
          presentationBefore: movementAfter.uiMetrics || {},
        })
      : {
          label: `${arena} Firefox bullet decrement before combined bomb cycle`,
          victimId: humanBulletLethalHit.victimId,
          victimLifeSeq: humanBulletLethalHit.victimLifeSeq,
          nextLifeSeq: null,
          winsBefore: winsBeforeCombat,
          winsAfter: winsBeforeCombat,
          damagePresentation: await provePostRespawnBotDamage({
            actor,
            hits: bulletHits,
            playerId,
            victimId: humanBulletLethalHit.victimId,
            victimLifeSeq: humanBulletLethalHit.victimLifeSeq,
            label: `${arena} Firefox bullet decrement before combined bomb cycle`,
          }),
          combinedBombCycle: true,
        };
    reportStage("bullet_cycle_complete", {
      victimId: bulletCycle.victimId,
      victimLifeSeq: bulletCycle.victimLifeSeq,
      nextLifeSeq: bulletCycle.nextLifeSeq,
    });

    // Lancaster owns the bullet leg; Warbird's authored faster bomb owns
    // the separate bomb leg. Identify ships directly, keeping maximum entry.
    // Establish that full-health menu selection in authored safety: a fresh
    // hostile successor can otherwise hit during Escape's real 200ms hold,
    // correctly reject the one ship request, and invalidate this setup.
    const bombShipSelectionSafety = arena === "extreme"
      ? await prepareExtremeFirefoxSafeReserve(actor, 1_650)
      : null;
    const bombShipEvidence = arena === "extreme" ? await selectPlayerShip({
      read: async () => (await actor.metricsState()).uiMetrics || {},
      press: (key, ms) => holdFirefoxKeys(firefox, [key === "Escape" ? "\uE00C" : key], ms),
      waitUntil, ship: 0, fromShip: 6,
      label: "Extreme Firefox bomb Warbird",
    }) : null;
    const bombPreparationEvidence = arena === "extreme"
      ? await prepareExtremeFirefoxSafeReserve(actor, 1_650)
      : null;

    // The bomb owns this entire second combat cycle. Target the exact life
    // established by the bullet cycle and require bomb damage itself to
    // provide the nonlethal decrement and death, rather than staging the bot
    // with later bullets that can accidentally own the terminal hit.
    const chaosBombActor = arena === "extreme"
      ? null
      : ((await actor.metricsState()).uiMetrics?.remotePlayers || [])
          .find((player) => player.id === botIdBase);
    const bombTarget = arena === "extreme"
      ? {
          victimId: bulletCycle.victimId,
          victimLifeSeq: bulletCycle.nextLifeSeq ?? bulletCycle.victimLifeSeq,
        }
      : {
          // Bot 768 is the human pursuer; bot 769 does not attack it. Bind
          // the distinct bomb-owned cycle to bot 768's exact current life so
          // autonomous bot combat cannot steal Firefox's terminal hit.
          victimId: botIdBase,
          victimLifeSeq: chaosBombActor?.lifeSeq,
        };
    if (!Number.isInteger(bombTarget.victimLifeSeq)) {
      throw new Error(`Firefox bullet cycle did not establish an exact bomb target: ${JSON.stringify(bulletCycle)}`);
    }

    let bombOffset = serverDamageBoundary(server);
    let bombPresentationBefore = (await actor.metricsState()).uiMetrics || {};
    let bombWinsBefore = bulletCycle.winsAfter;
    let bombHits = [];
    let humanBombLethalHit = null;
    const bombStageAttempts = [];
    if (arena === "extreme") {
      const pursuitStartedAt = Date.now();
      await attackBotsWithFirefoxInput(
        firefox,
        actor,
        server,
        playerId,
        bombOffset,
        bombTarget.victimId,
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
      humanBombLethalHit = bombHits.find((event) =>
        event.lethal === true &&
        event.victimId === bombTarget.victimId &&
        event.victimLifeSeq === bombTarget.victimLifeSeq
      ) || null;
      bombStageAttempts.push({
        mode: "genuine_keyboard_existing_encounter",
        victimId: bombTarget.victimId,
        victimLifeSeq: bombTarget.victimLifeSeq,
        elapsedMs: Date.now() - pursuitStartedAt,
        hits: bombHits,
      });
    }
    const chaosBombDeadline = Date.now() + 360_000;
    for (let attempt = 0; attempt < 6 && humanBombLethalHit == null; attempt += 1) {
      if (arena !== "extreme") {
        const remaining = chaosBombDeadline - Date.now();
        if (remaining <= 0) break;
        const renewed = await renewRetiredBombTarget({ actor, server, playerId, target: bombTarget,
          damageBoundary: bombOffset, label: "Firefox primary-bomb retry", timeoutMs: Math.min(10_000, remaining) });
        if (renewed) {
          reportStage("primary_bomb_target_retired", {
            previousLifeSeq: renewed.previousTarget.victimLifeSeq, nextLifeSeq: renewed.target.victimLifeSeq,
            terminalDamageSeq: renewed.retirement.damageSeq, terminalWeapon: renewed.retirement.weapon,
            primaryBombProofStillRequired: true,
          });
          bombStageAttempts.push({ mode: "retired_exact_life", ...renewed });
          Object.assign(bombTarget, renewed.target);
          bombOffset = renewed.damageBoundary;
          bombPresentationBefore = renewed.presentationBefore;
          bombWinsBefore = renewed.winsBefore;
        }
      }
      if (arena === "extreme") {
        const staged = await stageExtremeFirefoxCombatEncounter(
          actor,
          "z",
          bombTarget.victimId,
        );
        bombStageAttempts.push({
          attempt,
          staged: Boolean(staged),
          diagnostics: lastExtremeFirefoxStageDiagnostics.slice(-8),
        });
        // The safe-edge driver can submit the genuine Z edge itself after it
        // crosses out of safety. Consume that exact server-owned terminal
        // before starting the generic pursuit loop. Inspect it even if bot
        // counterfire kills the pilot just after submission and makes the
        // remaining stage return false; the exact projectile/death identity,
        // not pilot survival, owns this transaction.
        bombHits = playerWeaponDamageToBots(
          server,
          playerId,
          bombOffset,
          bombWeaponTypes,
        );
        humanBombLethalHit = bombHits.find((event) =>
          event.lethal === true &&
          event.victimId === bombTarget.victimId &&
          event.victimLifeSeq === bombTarget.victimLifeSeq
        ) || null;
        if (humanBombLethalHit != null) break;
        if (!staged) continue;
      }
      const attemptOffset = serverDamageBoundary(server);
      await attackBotsWithFirefoxInput(
        firefox,
        actor,
        server,
        playerId,
        attemptOffset,
        bombTarget.victimId,
        true,
        "bomb",
        arena === "extreme" ? 20_000 : Math.max(1, Math.min(60_000, chaosBombDeadline - Date.now())),
        bombTarget.victimLifeSeq,
      );
      const attemptBombHits = playerWeaponDamageToBots(
        server,
        playerId,
        attemptOffset,
        bombWeaponTypes,
      );
      bombHits = playerWeaponDamageToBots(server, playerId, bombOffset, bombWeaponTypes);
      humanBombLethalHit = bombHits.find((event) =>
        event.lethal === true &&
        event.victimId === bombTarget.victimId &&
        event.victimLifeSeq === bombTarget.victimLifeSeq
      ) || null;
      // A miss may require another real Z edge; no later bullet is allowed to
      // satisfy or restage this distinct bomb-owned death.
      void attemptBombHits;
    }
    if (!humanBombLethalHit) {
      throw new Error(`Firefox human bomb fire did not lethally damage the respawned server bot: ${JSON.stringify({
        bombTarget,
        bombStageAttempts,
        bombHits,
      })}`);
    }
    if (humanBombLethalHit.victimLifeSeq !== bombTarget.victimLifeSeq) {
      throw new Error(`Firefox bomb target life changed during the bomb-only cycle: ${JSON.stringify({
        bombTarget,
        humanBombLethalHit,
      })}`);
    }
    const earlyAuthoredBombEvidence = {
      damageSeq: humanBombLethalHit.damageSeq,
      projectileId: humanBombLethalHit.projectileId,
      serverTick: humanBombLethalHit.serverTick,
      victimId: humanBombLethalHit.victimId,
      victimLifeSeq: humanBombLethalHit.victimLifeSeq,
      weapon: humanBombLethalHit.weapon,
      damage: humanBombLethalHit.damage,
      energyBefore: humanBombLethalHit.energyBefore,
      energyAfter: humanBombLethalHit.energyAfter,
      lethal: humanBombLethalHit.lethal,
    };
    const bombCycle = await provePresentedBotCombatCycle({
      actor,
      server,
      playerId,
      playerName: new URL(actorUrl).searchParams.get("name"),
      hits: bombHits,
      lethalHit: humanBombLethalHit,
      winsBefore: bombWinsBefore,
      label: `${arena} Firefox bomb cycle`,
      presentationBefore: bombPresentationBefore,
      // The bullet cycle above owns the visible decrement assertion. A fresh
      // bot life can have less energy than one authored bomb, so this distinct
      // bomb cycle proves exact-life bomb ownership and terminal presentation.
      requireNonlethal: false,
    });
    reportStage("bomb_cycle_complete", {
      victimId: bombCycle.victimId,
      victimLifeSeq: bombCycle.victimLifeSeq,
      nextLifeSeq: bombCycle.nextLifeSeq,
    });

    const postRespawnOffset = serverDamageBoundary(server);
    const postRespawnProjectileOffset = parseServerProjectileEvents(server.logs).length;
    const postRespawnPresentationBefore = (await actor.metricsState()).uiMetrics || {};
    let postRespawnHits = [];
    const postRespawnAttempts = [];
    for (let attempt = 0; attempt < 4; attempt += 1) {
      const staged = arena === "extreme"
        ? await stageExtremeFirefoxCombatEncounter(actor, "Control", bombCycle.victimId)
        : null;
      await attackBotsWithFirefoxInput(
        firefox,
        actor,
        server,
        playerId,
        postRespawnOffset,
        bombCycle.victimId,
        false,
        "bullet",
        90_000,
        bombCycle.nextLifeSeq,
      );
      postRespawnHits = playerWeaponDamageToBots(
        server,
        playerId,
        postRespawnOffset,
        bulletWeaponTypes,
      );
      postRespawnAttempts.push({
        attempt,
        stagedExactLane: arena !== "extreme" || Boolean(staged),
        botId: staged && staged !== false ? staged.botId : null,
        botLifeSeq: staged && staged !== false ? staged.botLifeSeq : null,
        aim: staged && staged !== false ? staged.aim : null,
        hits: postRespawnHits,
      });
      if (postRespawnHits.some((event) =>
        event.victimId === bombCycle.victimId &&
        u32IsStrictlyNewer(event.victimLifeSeq, bombCycle.victimLifeSeq)
      )) break;
    }
    const postRespawnOwnedProjectiles = parseServerProjectileEvents(server.logs)
      .slice(postRespawnProjectileOffset)
      .filter((event) =>
        event.type === "server_projectile_spawned" &&
        event.ownerId === playerId &&
        bulletWeaponTypes.has(event.weapon & 0x1f)
      );
    if (postRespawnOwnedProjectiles.length === 0) {
      throw new Error("Firefox post-respawn keyboard fire produced no authoritative projectile");
    }
    await waitUntil("Firefox post-respawn bullet tail reaches hit or authored expiry", async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return ownedProjectileTailSettled({
        projectileEvents: parseServerProjectileEvents(server.logs)
          .slice(postRespawnProjectileOffset),
        ownerId: playerId,
        latestServerTick: metrics.latestServerTick,
        weaponTypes: bulletWeaponTypes,
      });
    }, 10_000);
    // The attack loop returns on the first accepted decrement. Resolve the
    // complete immutable burst before choosing the next bot life; otherwise
    // a later round can retire that life while the repeated bomb remains
    // incorrectly bound to its predecessor.
    postRespawnHits = playerWeaponDamageToBots(
      server,
      playerId,
      postRespawnOffset,
      bulletWeaponTypes,
    );
    const postRespawnBotDamage = await provePostRespawnBotDamage({
      actor,
      hits: postRespawnHits,
      playerId,
      victimId: bombCycle.victimId,
      victimLifeSeq: bombCycle.nextLifeSeq,
      label: `${arena} Firefox post-respawn bullet`,
    });
    postRespawnBotDamage.attempts = postRespawnAttempts;
    reportStage("post_respawn_damage_complete", {
      victimId: postRespawnBotDamage.event.victimId,
      victimLifeSeq: postRespawnBotDamage.event.victimLifeSeq,
      damageSeq: postRespawnBotDamage.event.damageSeq,
    });
    // Capture the bounded browser trace while ordinary bot counterfire from
    // this live encounter is still fresh. Waiting until the later repeated
    // human-owned cycle and respawn can legitimately evict the consumed
    // projectile source from the 1,200-frame ring even though its exact
    // server damage/death evidence remains present.
    projectilePresentationStates.push(await actor.presentationState());

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
          label: "extreme Firefox repeated bullet-owned cycle",
          presentationBefore: postRespawnPresentationBefore,
        });

    let repeatedCombatCycle = null;
    let lateAuthoredBombEvidence = null;
    const lateAuthoredBombAttempts = [];
    if (arena === "extreme") {
      const repeatedTarget = {
        victimId: bombCycle.victimId,
        victimLifeSeq: repeatedBulletCycle?.nextLifeSeq ??
          postRespawnBotDamage.event.victimLifeSeq,
      };
      const repeatedOffset = serverDamageBoundary(server);
      const repeatedPresentationBefore = (await actor.metricsState()).uiMetrics || {};
      const repeatedAttempts = [];
      let repeatedHits = [];
      let repeatedLethalHit = null;
      for (let attempt = 0; attempt < 6 && repeatedLethalHit == null; attempt += 1) {
        await prepareExtremeFirefoxSafeReserve(actor, 1_650);
        const targetLuredOutOfSafety = await lureExtremeFirefoxTargetOutOfSafety(
          actor,
          repeatedTarget.victimId,
          repeatedTarget.victimLifeSeq,
        );
        const attemptOffset = serverDamageBoundary(server);
        const staged = targetLuredOutOfSafety
          ? await stageExtremeFirefoxCombatEncounter(
              actor,
              "z",
              repeatedTarget.victimId,
            )
          : false;
        const stagedExactLane = staged != null && staged !== false &&
          Number.isInteger(staged.botId) && Number.isInteger(staged.botLifeSeq);
        let attemptHits = playerWeaponDamageToBots(
          server,
          playerId,
          attemptOffset,
          bombWeaponTypes,
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
        if (repeatedLethalHit == null) {
          await attackBotsWithFirefoxInput(
            firefox,
            actor,
            server,
            playerId,
            attemptOffset,
            repeatedTarget.victimId,
            true,
            "bomb",
            30_000,
            repeatedTarget.victimLifeSeq,
          );
          attemptHits = playerWeaponDamageToBots(
            server,
            playerId,
            attemptOffset,
            bombWeaponTypes,
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
          attempt,
          targetLuredOutOfSafety,
          staged: stagedExactLane,
          fallbackKeyboardPursuit: !stagedExactLane,
          botId: stagedExactLane ? staged.botId : null,
          botLifeSeq: stagedExactLane ? staged.botLifeSeq : null,
          aim: stagedExactLane ? staged.aim : null,
          stagingDiagnostics: lastExtremeFirefoxStageDiagnostics.slice(-8),
          hits: attemptHits,
        });
      }
      if (repeatedLethalHit == null) {
        throw new Error(`repeated Extreme Firefox bomb cycle did not lethally damage the exact post-respawn bot life: ${JSON.stringify({
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
          ? "extreme Firefox repeated bullet-decrement/bomb-death cycle"
          : "extreme Firefox bomb cycle after repeated bullet-owned death",
        presentationBefore: repeatedPresentationBefore,
        requireNonlethal: false,
      });
      repeatedCombatCycle.attempts = repeatedAttempts;
      repeatedCombatCycle.priorBulletDamage = postRespawnBotDamage;
      repeatedCombatCycle.priorBulletCycle = repeatedBulletCycle;
      lateAuthoredBombEvidence = repeatedLethalHit;
    } else {
      // Prove the reported long-session failure cannot recur after the first
      // successful explosion. Use a fresh exact bot life and another real Z
      // edge after the complete death/respawn and post-respawn bullet phases.
      for (let attempt = 0; attempt < 4 && lateAuthoredBombEvidence == null; attempt += 1) {
        // The preceding real-key bullet phase may itself kill the first
        // post-bomb life. Synchronize on the browser admitting an exact live
        // successor instead of polling for less than the authored respawn
        // deadline and then reporting a false combat failure.
        const currentTarget = await waitForPresentedLiveBotSuccessor({
          actor,
          playerId: bombCycle.victimId,
          predecessorLifeSeq: bombCycle.victimLifeSeq,
          label: `late Chaos Firefox proximity-bomb attempt ${attempt}`,
        });
        const attemptOffset = serverDamageBoundary(server);
        // This leg requires a fresh later-life bomb hit. The distinct
        // bomb-owned death is proved above; do not chase another unnecessary
        // lethal while the required nonlethal transaction is already present.
        await attackBotsWithFirefoxInput(
          firefox,
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
        throw new Error(`late Chaos Firefox proximity-bomb cycle produced no authoritative bot damage: ${JSON.stringify({
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
      throw new Error(`Firefox authored-bomb replay did not cross an exact bot life: ${JSON.stringify({
        earlyAuthoredBombEvidence,
        lateAuthoredBombEvidence,
      })}`);
    }
    const requireStationaryBotEvidence = arena === "extreme" ||
      stationaryBotFromLifeSeq > 0;
    const stationaryBombLifeSeqs = requireStationaryBotEvidence
      ? requiredStationaryFixtureLifeSeqs(
          [
            earlyAuthoredBombEvidence.victimLifeSeq,
            lateAuthoredBombEvidence.victimLifeSeq,
          ],
          stationaryBotFromLifeSeq,
          arena === "extreme" ? stationaryBotStaticLifeSeqs : null,
        )
      : [];
    const stationaryPassiveBombLifeSeqs = stationaryBombLifeSeqs.filter((lifeSeq) =>
      passiveBotLifeSeqs.includes(lifeSeq)
    );
    const stationaryBotEvidence = requireStationaryBotEvidence
      ? stationaryBotMotionEvidence(server.logs, {
          botId: bombCycle.victimId,
          // Match Chrome's finite fixture contract: both configured static
          // lives need proof, even if bullets retire the intervening life and
          // the repeated bomb hits a moving successor.
          requiredLifeSeqs: stationaryBombLifeSeqs,
          requiredPassiveFireLifeSeqs: stationaryPassiveBombLifeSeqs,
        })
      : null;
    const passiveBotProjectileSpawns = passiveBotLifeSeqs.length === 0
      ? []
      : parseServerProjectileEvents(server.logs).filter((event) =>
          event.type === "server_projectile_spawned" &&
          event.ownerId === bombCycle.victimId &&
          passiveBotLifeSeqs.includes(event.ownerLifeSeq)
        );
    if (passiveBotProjectileSpawns.length !== 0) {
      throw new Error(`passive Firefox collision life emitted a projectile: ${JSON.stringify({
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
          successorHostileLethal: null,
        };

    // As in Chrome, isolate the moving human-owned collision proofs from
    // unrelated bot volleys, then prove actual hostile successors. Retire
    // every remaining controlled source before observing incoming damage.
    const incomingSourcePreparation = arena === "chaos-zone" && expectedBotCount === 2 &&
      passiveBotLifeSeqs.join(",") === "1,2"
      ? await prepareBotFacingSources({
          botIds: orderedBotIds, passiveLifeSeqs: passiveBotLifeSeqs,
          readTarget: (id, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: id, predecessorLifeSeq: 0,
            label: "Firefox incoming-fire source", timeoutMs,
          }),
          retire: async (target, timeoutMs) => {
            const offset = serverDamageBoundary(server);
            const hits = await attackBotsWithFirefoxInput(firefox, actor, server, playerId,
              offset, target.id, true, "bullet", timeoutMs, target.lifeSeq);
            return hits.find(hit => hit.victimId === target.id &&
              hit.victimLifeSeq === target.lifeSeq && hit.lethal === true);
          },
          successor: (target, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: target.id, predecessorLifeSeq: target.lifeSeq,
            label: "Firefox incoming-fire hostile successor", timeoutMs,
          }),
        })
      : null;
    reportStage("incoming_hostile_sources", incomingSourcePreparation);
    const botLethalBrake = await brakeClientVelocity(actor, 5_000);
    const counterfireOffset = serverDamageBoundary(server);
    const beforeCounterfire = (await firefoxMetricsState(firefox)).uiMetrics || {};
    const retainedProjectileOwnerEvidence = assertFirefoxBotProjectileOwnersVisible(
      projectilePresentationStates.flatMap(metricsTraceWithViewport),
      projectilePresentationStates.flatMap((state) => state.remoteSpriteTrace || []),
      orderedBotIds,
      { required: false },
    );
    const requireFreshCounterfire = process.env.ZUBSPACE_E2E_REQUIRE_FRESH_COUNTERFIRE === "1";
    const observedLethalBotHit = requireFreshCounterfire || retainedProjectileOwnerEvidence == null
      ? null
      : ([...botDamageToPlayer(server, playerId, damageOffset)]
          .reverse()
          .find((event) =>
            event.lethal === true &&
            (passiveBotLifeSeq === 0 ||
              u32IsStrictlyNewer(event.shooterLifeSeq, passiveBotLifeSeq)) &&
            Number.isInteger(beforeCounterfire.playerLifeSeq) &&
            u32IsStrictlyNewer(beforeCounterfire.playerLifeSeq, event.victimLifeSeq) &&
            (beforeCounterfire.playerRespawnSeq ?? 0) > initialRespawnSeq
          ) || null);
    // Retain an earlier bot-owned lethal only when its exact on-screen
    // projectile/draw proof is still present in the bounded browser windows.
    // Otherwise stage fresh stationary-hostile counterfire below: server-only
    // damage cannot stand in for the client presentation requirement.
    const counterfirePreparation = observedLethalBotHit == null
      ? await prepareHostileCounterfireLife({
          target: await waitForPresentedLiveBotSuccessor({
            actor, playerId: botIdBase, predecessorLifeSeq: 0,
            label: "Firefox counterfire source",
          }),
          passiveLifeSeqs: passiveBotLifeSeqs,
          retire: async target => {
            const offset = serverDamageBoundary(server);
            const hits = await attackBotsWithFirefoxInput(firefox, actor, server,
              playerId, offset, target.id, true, "bomb", 90_000, target.lifeSeq);
            return hits.find(hit => hit.victimId === target.id &&
              hit.victimLifeSeq === target.lifeSeq && hit.lethal === true);
          },
          successor: target => waitForPresentedLiveBotSuccessor({
            actor, playerId: target.id, predecessorLifeSeq: target.lifeSeq,
            label: "Firefox counterfire hostile successor",
          }),
        })
      : null;
    if (counterfirePreparation != null) {
      counterfirePreparation.assignment = await alignCounterfireAssignment({
        read: async () => (await actor.metricsState()).uiMetrics || {}, server,
        botIds: orderedBotIds, desiredBotId: counterfirePreparation.target.id, waitUntil,
        changeShip: (before, ship, boundedWait) => selectPlayerShip({
          read: async () => (await actor.metricsState()).uiMetrics || {},
          press: (key, ms) => holdFirefoxKeys(firefox, [key === "Escape" ? "\uE00C" : key], ms),
          waitUntil: boundedWait, ship, fromShip: before.playerShip,
          label: "Firefox controlled counterfire assignment",
        }),
      });
      reportStage("counterfire_hostile_source", counterfirePreparation);
      if (passiveBotLifeSeq !== 0 &&
          !u32IsStrictlyNewer(counterfirePreparation.target.lifeSeq, passiveBotLifeSeq)) {
        throw new Error("Firefox counterfire source has not passed the required fixture life");
      }
    }
    const lethalBotHit = observedLethalBotHit ??
      await stageFirefoxForBotLethalCounterfire(
        actor,
        server,
        playerId,
        counterfireOffset,
        passiveBotLifeSeq === 0 ? null : passiveBotLifeSeq,
        counterfirePreparation.target,
      );
    if (!lethalBotHit) {
      throw new Error("server bots did not lethally damage the staged active Firefox pilot");
    }
    if (
      passiveFireEvidence != null &&
      !u32IsStrictlyNewer(lethalBotHit.shooterLifeSeq, passiveFireEvidence.lifeSeq)
    ) {
      throw new Error(`Firefox passive collision life did not resume hostile successor fire: ${JSON.stringify({
        passiveFireEvidence,
        lethalBotHit,
      })}`);
    }
    if (passiveFireEvidence != null) {
      passiveFireEvidence.successorHostileLethal = {
        damageSeq: lethalBotHit.damageSeq,
        projectileId: lethalBotHit.projectileId,
        shooterLifeSeq: lethalBotHit.shooterLifeSeq,
        victimLifeSeq: lethalBotHit.victimLifeSeq,
      };
    }
    projectilePresentationStates.push(await actor.presentationState());
    await waitUntil("Firefox applies authoritative death and respawns", async () => {
      const metrics = (await firefoxMetricsState(firefox)).uiMetrics || {};
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
        (metrics.playerRespawnSeq ?? 0) > initialRespawnSeq;
    }, 15_000);
    const finalState = await actor.presentationState();
    projectilePresentationStates.push(finalState);
    const longHorizonBotSmoothness = remoteActorMotionAcrossStatesEvidence(
      projectilePresentationStates,
      orderedBotIds,
      { doorRelocations: botDoorRelocationEvidence(server.logs) },
    );
    if (!longHorizonBotSmoothness.ok) {
      throw new Error(`${arena} Firefox bot presentation crossed an unexpected same-life discontinuity: ${JSON.stringify(longHorizonBotSmoothness)}`);
    }
    const audioEvidence = assertFirefoxBotAudio(finalState);
    // See `bot_authority.mjs`: the exact-tick facing witness is not decidable on an impaired presentation
    // timeline, so it is skipped and recorded rather than asserted. The clean cells keep it unmodified.
    const facingSourcePreparation = networkProfile || passiveBotLifeSeqs.length === 0
      ? null
      : await prepareBotFacingSources({
          botIds: orderedBotIds, passiveLifeSeqs: passiveBotLifeSeqs,
          readTarget: (id, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: id, predecessorLifeSeq: 0,
            label: "Firefox facing source", timeoutMs,
          }),
          retire: async (target, timeoutMs) => {
            const offset = serverDamageBoundary(server);
            const hits = await attackBotsWithFirefoxInput(firefox, actor, server, playerId,
              offset, target.id, true, "bomb", timeoutMs, target.lifeSeq);
            return hits.find(hit => hit.victimId === target.id &&
              hit.victimLifeSeq === target.lifeSeq && hit.lethal === true);
          },
          successor: (target, timeoutMs) => waitForPresentedLiveBotSuccessor({
            actor, playerId: target.id, predecessorLifeSeq: target.lifeSeq,
            label: "Firefox facing hostile successor", timeoutMs,
          }),
        });
    reportStage("facing_hostile_sources", facingSourcePreparation);
    const facingEncounterPreparation = networkProfile ? null : await approachLiveBotEncounter({
      actor, server, botIds: orderedBotIds, positionIsOnAuthoredSafeTile,
      searchPoint: { x: 8_200, y: 8_200 },
      label: `${arena} Firefox facing collection`,
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
          botIds: orderedBotIds,
          timeoutMs: 90_000,
          provoke: (cycle) => provokeFirefoxBotLaunches(actor, server, orderedBotIds, cycle),
        });
    if (facingSourcePreparation != null) facingEvidence.sourcePreparation = facingSourcePreparation;
    if (facingEncounterPreparation != null) facingEvidence.encounterPreparation = facingEncounterPreparation;
    if (requireFacingCollection) facingEvidence.freshCollectionRequired = true;
    const projectileOwnerEvidence = assertFirefoxBotProjectileOwnersVisible(
      projectilePresentationStates.flatMap(metricsTraceWithViewport),
      projectilePresentationStates.flatMap((state) => state.remoteSpriteTrace || []),
      orderedBotIds,
    );
    const wallCollisionEvidence = botWallCollisionEvidence(server, orderedBotIds);
    const combatViewGeometryEvidence = assertHumanProjectilePresentationGeometry(
      server,
      playerDamageToBots(server, playerId, damageOffset),
      `${arena} Firefox human-to-bot combat`,
    );
    const botToFirefox = serverDamageEventsSince(server, damageOffset).filter((event) =>
      event.shooterId >= botIdBase && event.victimId === playerId
    );
    if (botToFirefox.length === 0) {
      throw new Error("Firefox received no server-authoritative bot damage");
    }
    await firefoxKeyDown(firefox, "Space");
    await waitUntil("Firefox scoreboard presents every proved bot kill", async () => {
      const metrics = (await firefoxMetricsState(firefox)).uiMetrics || {};
      return metrics.scoreboardVisible === true &&
        metrics.selfWins >= (repeatedCombatCycle?.winsAfter ?? bombCycle.winsAfter) &&
        orderedBotIds.every((id) => (metrics.scoreboardPlayerIds || []).includes(id));
    }, 5_000);
    const scoreboardAfterCombat = (await firefoxMetricsState(firefox)).uiMetrics || {};
    const scorePresentation = {
      scoreboardVisible: scoreboardAfterCombat.scoreboardVisible,
      playerId,
      winsBefore: winsBeforeCombat,
      winsAfter: scoreboardAfterCombat.selfWins,
      botIds: (scoreboardAfterCombat.scoreboardPlayerIds || [])
        .filter((id) => id >= botIdBase),
    };
    const hiddenPrizeEvidence = combatHiddenPrizeEvidence(server);
    if (networkProfile) {
      await execute(firefox, "window.__zubspaceEndNetworkProfileMeasurementForTest?.(); return true;");
      await waitUntil("Firefox measured impairment drains before disconnect", async () => {
        const evidence = await execute(firefox, "return window.__zubspaceNetworkProfileStateForTest?.() || null;");
        return evidence?.measurementEnded === true &&
          evidence.s2c.pending === 0 && evidence.c2s.pending === 0 &&
          evidence.s2c.drainPending === 0 && evidence.c2s.drainPending === 0;
      }, 10_000);
    }
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [firefox.logs],
      disconnect: async () => {
        await execute(firefox, "window.__zubspaceSendDisconnectForTest?.(); return true;");
      },
    });
    const finalProvenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    await waitUntil("Firefox bot actor disconnect cleanup", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) return false;
      const status = await response.json();
      // Gameplay cleanup is the boundary needed before the late security
      // audit. RTC teardown is owned by the browser `finally` block and is
      // intentionally not required here.
      return status.clients === 0 && status.joinedPlayers === 0;
    }, 30_000);
    let networkProfileEvidence = null;
    if (networkProfile) {
      await waitUntil("Firefox deterministic network profile timers to drain", async () => {
        const evidence = await execute(
          firefox,
          "return window.__zubspaceNetworkProfileStateForTest?.() || null;",
        );
        return evidence?.s2c?.pending === 0 && evidence?.c2s?.pending === 0;
      }, 10_000);
      networkProfileEvidence = await execute(
        firefox,
        "return window.__zubspaceNetworkProfileStateForTest?.() || null;",
      );
      assertNetworkProfileEvidence(networkProfileEvidence, {
        expectedName: networkProfile,
        expectedSeed: networkProfileSeed,
        label: `Firefox deterministic ${networkProfile} profile`,
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
    assertRendererConsoleClean(actor.consoleEvidence());
    const successArtifacts = successArtifactDir
      ? await saveBrowserE2eArtifacts({
          artifactDir: successArtifactDir,
          server,
          staticServer,
          urls: [actorUrl],
          clients: [{ label: "firefox-bot-actor", client: actor }],
          extraLogs: { geckodriver: firefox.logs,
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
      browserPair: "firefox-server-bots",
      arena,
      playerId,
      botIds: orderedBotIds,
      snapshotBotIds: [...snapshotBotIds].sort((left, right) => left - right),
      radarBotIds: [...radarBotIds].sort((left, right) => left - right),
      botFeelEvidence,
      initialReadinessEncounter,
      feelEncounter,
      setupNavigation: actor.rendezvousTrace || [],
      motionSoakEvidence,
      agedJoinEvidence,
      longHorizonBotSmoothness,
      botSpriteEvidence,
      snapshotOnlyPresentation,
      botRepelEvidence,
      audioEvidence,
      projectileOwnerEvidence,
      facingEvidence,
      firefoxMovement,
      wallCollisionEvidence,
      combatViewGeometryEvidence,
      firefoxPlayerToBot: playerDamageToBots(server, playerId, damageOffset),
      humanBulletLethalHit,
      humanBombLethalHit,
      combatShipEvidence,
      bombShipEvidence,
      bombPreparationEvidence,
      bombShipSelectionSafety,
      bulletCycle,
      bombCycle,
      earlyAuthoredBombEvidence,
      lateAuthoredBombEvidence,
      stationaryBotEvidence,
      passiveFireEvidence,
      lateAuthoredBombAttempts,
      postRespawnBotDamage,
      repeatedBulletCycle,
      repeatedCombatCycle,
      botLethalBrake,
      scorePresentation,
      botToFirefox,
      lethalBotHit,
      incomingSourcePreparation,
      counterfirePreparation,
      requireFreshCounterfire,
      respawnSeq: finalState.uiMetrics?.playerRespawnSeq,
      objectiveEvidence,
      hiddenPrizeEvidence,
      securityAudit,
      dataChannelDeliveryAudit,
      networkProfileEvidence,
      serverObserverProfileEvidence,
      provenance: finalProvenance,
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
    const artifactDir = `/tmp/zubspace-bot-authority-firefox-${Date.now()}-${process.pid}`;
    const provenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
    const artifacts = await saveBrowserE2eArtifacts({
      artifactDir,
      server,
      staticServer,
      urls: [actorUrl],
      clients: [{ label: "firefox-bot-actor", client: actor }],
      extraLogs: {
        geckodriver: firefox.logs,
        "setup-navigation": JSON.stringify(actor?.rendezvousTrace || []),
        "bot-spawn-fixture": JSON.stringify(botSpawnFixture?.evidence || null),
        "bot-presentation-snapshots": JSON.stringify(projectilePresentationStates),
        "bot-facing-collection": JSON.stringify(facingCollectionStates),
      },
      provenance,
    }).catch(() => null);
    if (artifacts) console.error(`artifacts: ${artifactDir}`);
    throw error;
  } finally {
    await deleteSession(firefox).catch(() => {});
    await Promise.all([
      stopProcess(firefox.child),
      stopProcess(server.child),
    ]).catch(() => {});
    if (botSpawnFixture) fs.rmSync(botSpawnFixture.root, { recursive: true, force: true });
    staticServer.server.close();
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) main().catch((error) => {
  console.error(error.stack || error.message);
  process.exitCode = 1;
});

export { main, returnExtremeFirefoxPilotToSafety };
