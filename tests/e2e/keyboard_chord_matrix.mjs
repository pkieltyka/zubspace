#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  createSession,
  cycleFirefoxWindowFocus,
  deleteSession,
  execute,
  firefoxKeyDown,
  firefoxKeyUp,
  firefoxKeyBatch,
  firefoxArtifactClient,
  freePort,
  holdFirefoxKeys,
  metricsState as firefoxMetricsState,
  openClient,
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
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import { initialAuthorityReady, prepareKeyboardLifecycleOrigin } from "./keyboard_lifecycle_origin.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { holdThenChordFirefoxKeys } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { keyboardTraceReaderStats } from "./keyboard_trace_reader.mjs";
import { browserConsoleEvidence, assertRendererConsoleClean } from "../../tools/browser_console_evidence.mjs";
import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  brakeClientVelocity,
  brakeVelocityDisposition,
  compactMetrics,
  driveClientToWorldTarget,
  openChromeClient,
  orientClientForProtocolDirection,
  parseServerDamageEvents,
  rendezvousPlayers,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import {
  artifactFileManifest,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { writeQaTraceArtifact, writeQaTraceFailureResult } from "./qa_trace_artifacts.mjs";
import { assertNetworkProfileEvidence } from "./network_profile_evidence.mjs";
import { INPUT_LATENCY_CAPTURE, proveNextDrawPrediction } from "./input_latency_evidence.mjs";
import {
  authoredMapGeometry,
  authoredSafeTiles,
  authoredSafetyDistance,
  authoredSafetyMargin,
  authoredExitHeading,
  deepestStagingPoint,
  bestBoostHeading,
  runwayAlongHeading,
  hullOverlapsOnlyOpenDoors,
} from "./authored_map_geometry.mjs";
import { chaosSafeRouteToAuthoredSafety, CHAOS_WORMHOLE_FIELD_AVOID_PX } from "./chaos_safe_route.mjs";
import { driveRouteWithReplans } from "./chaos_route_replanner.mjs";
import { extremeChordStageRoute } from "./extreme_chord_stage.mjs";
import {
  authoredFiringRingWaypoint,
  lifecycleRouteBudgetMs,
  routedHopAccepted,
  routedMapCounterfireWaypoint,
  stageAdvanced,
} from "./bot_combat_evidence.mjs";
import { BATCH_CAPTURE, proveHeldKeyBatches, beginHeldDeath, finishHeldDeath } from "./held_key_batches.mjs";
import { assertServerObserverProfile, serverObserverProfileForNetworkProfile } from "./server_observer_profile.mjs";
import {
  proveAfterburnerMatrixWithLifecycleRetries,
  proveKeyboardChordMatrix,
  proveHeldChordSurvivesRepeatPause,
  proveFocusLossReleasesHeldChord,
  proveSafeZoneFireThrust,
  proveChaosSafeZoneHeldSets,
} from "./keyboard_chords.mjs";

const browser = process.env.ZUBSPACE_E2E_BROWSER || "chrome";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
// Observed uncapped all-software SwANGLE repeatedly blocks bufferSubData.
// High-refresh coverage uses Chrome's normal renderer choice;
// measured cadence, readiness and input contracts remain mandatory. Keep the
// explicit stress backend available for diagnostic reproduction.
const chromeGraphicsBackend = process.env.ZUBSPACE_E2E_KEYBOARD_GRAPHICS_BACKEND ||
  (process.env.ZUBSPACE_E2E_HIGH_REFRESH === "1" ? "native" : "swiftshader");
const networkProfile = process.env.ZUBSPACE_E2E_NETWORK_PROFILE || "";
const networkProfileSeed = Number(process.env.ZUBSPACE_E2E_NETWORK_PROFILE_SEED || "271828");
const serverObserverProfile = serverObserverProfileForNetworkProfile(networkProfile);
if (!Number.isInteger(networkProfileSeed) || networkProfileSeed < 1 || networkProfileSeed > 0xffff_ffff) {
  throw new Error("keyboard network profile seed must be a nonzero u32");
}
process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/keyboard_chord_matrix.mjs";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const target = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
  path.join("/tmp", `zubspace-keyboard-chords-${browser}-${arena}-${Date.now()}-${process.pid}`);

const extremeSafeTiles = arena === "extreme"
  ? authoredSafeTiles(fs.readFileSync(path.join(repoRoot, "assets/zones/extreme/doomsy8.lvl")))
  : [];
// Both bundled arena maps are authored geometry the harness can route against;
// only Extreme had it loaded, which is why the Chaos lane guessed at open
// columns and intermittently drove into a wall.
const authoredRouteMapFile = arena === "extreme"
  ? "assets/zones/extreme/doomsy8.lvl"
  : arena === "chaos-zone"
    ? "assets/zones/chaos-zone/bdhadar.lvl"
    : null;
const authoredRouteGeometry = authoredRouteMapFile
  ? authoredMapGeometry(fs.readFileSync(path.join(repoRoot, authoredRouteMapFile)), {
      // Keep the staging search and the leg preflight out of the wormhole gravity field as well, not just the
      // BFS. The planner avoids the field, but these predicates are what decide whether a *leg* is flyable,
      // and a leg that grazes the field is the measured relocation defect: without the radius here
      // a route that crosses the field is reported as flyable and then flown into it. A map with no wormhole
      // tiles simply produces an empty mask.
      wormholeAvoidRadiusPx: CHAOS_WORMHOLE_FIELD_AVOID_PX,
    })
  : null;

const distanceToAuthoredSafety = authoredSafetyDistance(extremeSafeTiles);
// Unlike Extreme, Chaos does not spawn inside authored safety: of the shipped map's 353
// safe tiles, only two are reachable by the hull from the canonical spawn, 195 corridor
// hops away. The safe-zone held sets therefore have to be *flown* to the green - from
// wherever the ordinary matrix left the pilot, which in the first real run was x=10440,
// two kilometres from the spawn. Extreme stays as it was, because there the green already
// is the spawn.
const chaosSafeMapBytes = arena === "chaos-zone"
  ? fs.readFileSync(path.join(repoRoot, authoredRouteMapFile))
  : null;
const chaosAuthoredSafe = (x, y) => authoredRouteGeometry?.tileAt(x, y) === 171;

function cannotReenterSafetyDuringChord(metrics) {
  const speed = Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY);
  const distance = distanceToAuthoredSafety(metrics.playerX, metrics.playerY);
  // One second covers the complete staggered chord. The additional 128px is
  // deliberately larger than the ship hull plus one second of authored
  // acceleration, so this can only exempt a ship materially separated from
  // every real safe tile in the shipped map.
  return Number.isFinite(speed) && Number.isFinite(distance) &&
    distance > speed + 128;
}

// Chaos runs with no drag and no stop key, so parking on a four-tile green has exactly one
// authored mechanism: point the ship against its velocity and reverse-thrust (`w`), which is what
// `brakeClientVelocity` does everywhere else in the harness. A hand-rolled burst driver was tried
// and measured wrong: sizing a burst as `sqrt(2D/180)` (the 180px/s^2 the brake helper itself
// assumes) covers the distance but leaves the ship at 288px/s at the burst end, and it then coasts
// at that speed for as long as it takes to turn 180 degrees before the reverse thrust can start.
// Attempt 7 showed exactly that shape - closest approach 57px, then 423px past the waypoint with
// the velocity reading zero, because the overshoot happened while the brake was still turning.
// `driveClientToWorldTarget` already solves that ordering: it steers by velocity error, so a pulse
// is only ever held while the ship is heading where it wants to go.
//
// What it lacked before was room. Its 42px/s cruise floor is not laziness, it is the physics: a
// short waypoint makes it spend the leg turning instead of accelerating, which is how a 144px leg
// burned 63 pulses and covered 300px. The route therefore hands it the 900px `drivePoints`
// polyline, and the per-point budget comes from that floor - a 900px point is 21.4s at 42px/s, so
// 60s allows three failed approaches before the driver's own no-progress watchdog has to be
// believed. Nothing here was relaxed to make a lane pass: the previous 20s budget was the number
// that failed, and this one is derived from the driver's own speed floor.
// Both failure shapes now have the same cause, and it is the driver's mode rather than its budget:
// the attempts that died did so on short legs (144px in 63 pulses, then 107px in 60), where the
// non-settle mode spends the leg turning and its no-progress watchdog - which only counts
// displacement while the last action was thrust - calls a turning loop a blocked corridor. The
// settle mode is the one written for a ship that cannot stop instantly: it also credits a *falling
// speed* near the target as progress, because braking has to overshoot before it can come back.
// Each waypoint is therefore a real stop, which is what the pilot needs anyway to be certified as
// resting on authored safety at the end.
// Chaos' route legs run inside corridors that are sometimes one or two tiles wide and the ship's hull is 28 px
// across, so where the pilot stops *along* the corridor axis matters as much as being near the waypoint.
//
// The retained leg 4 is why: the corridor's hull-clear band at x=12082 is y in [4238, 4288] while the
// waypoint sits at y=4248, and a 12 px arrival tolerance let the pilot stop at y=4235.97 - two pixels outside the
// band - after which the straight line to the next waypoint was hull-blocked from x=12090 and the leg spent its
// whole 60 s budget thrusting into terrain at a 0.003-turn heading error. Aim at the waypoint instead: the
// corridor centres the route is planned through are hull-clear by construction, so a tight tolerance re-centres
// the pilot on every leg and keeps the next line flyable.
const CHAOS_ROUTE_LEG_ARRIVAL_TOLERANCE_PX = 4;

async function flyChaosRouteLeg(actor, waypoint, { tolerance, timeoutMs, label, completion = null }) {
  // A leg whose straight line is not hull-clear cannot be flown, and thrusting into the terrain it clips is
  // indistinguishable from a broken control law - which is exactly how the retained leg 4 was first read. Name
  // it before driving, so a recurrence costs one run instead of a 60 s stall that has to be argued about.
  const preflight = (await actor.metricsState()).uiMetrics || {};
  if (authoredRouteGeometry !== null && Number.isFinite(preflight.playerX)) {
    const toX = waypoint.x - preflight.playerX;
    const toY = waypoint.y - preflight.playerY;
    const distance = Math.hypot(toX, toY);
    if (distance > tolerance) {
      const runway = runwayAlongHeading({
        // Terrain only. This check asks whether a wall blocks the leg, and the wormhole field is not a wall:
        // using the field-aware predicate here rejected the leg that flies *out* of a field the pilot was pulled
        // into, which is the lane 12 regression. The planner is what avoids the field.
        hullIsTraversable: (x, y) => authoredRouteGeometry.authoredTerrainHullIsTraversable(x, y),
        safetyDistance: distanceToAuthoredSafety,
        x: preflight.playerX,
        y: preflight.playerY,
        dx: toX / distance,
        dy: toY / distance,
        travelPx: Math.ceil(distance),
        // One-pixel steps: the blockages that matter are a handful of pixels wide at the leg's start under an
        // 8 px hull offset, and the default 16 px step would step straight over them.
        stepPx: 1,
      });
      if (runway && runway.runwayPx + tolerance < distance) {
        throw new Error(`${label} is not flyable: the straight line to `
          + `(${waypoint.x},${waypoint.y}) is hull-blocked after ${runway.runwayPx}px of `
          + `${Math.round(distance)}px from (${preflight.playerX.toFixed(1)},${preflight.playerY.toFixed(1)}); `
          + `the pilot is off the corridor centre and driving would thrust into terrain`);
      }
    }
  }
  const drive = () => driveClientToWorldTarget(
    actor, waypoint, timeoutMs, true, tolerance, completion, 90, 0.35, 8_000,
  );
  const shape = (result, metrics) => ({
    reason: result?.reason ?? "not reached",
    // Without this the driver's whole-attempt shape is computed, returned and discarded, because this payload
    // is assembled field by field - which is why four gate logs showed a route timeout with no attemptPathPx in
    // them and the instrument looked silent rather than dropped.
    ...(Object.entries(result ?? {}).filter(([key]) => key.startsWith("attempt"))
      .reduce((carry, [key, value]) => ({ ...carry, [key]: value }), {})),
    pulses: result?.pulses ?? null,
    // The two branches that are not a plain "the driver never converged": a refused (lifecycle-paused) pulse and
    // the timeout's authority evidence. Dropping either here would rebuild the same blind spot the field-by-field
    // assembly already cost once.
    ...(Object.entries(result ?? {})
      .filter(([key]) => key.endsWith("AtTimeout") || key === "undeliveredMs" || key.startsWith("authority")
        || key === "traceSamplesTotal" || key === "traceTail")
      .reduce((carry, [key, value]) => ({ ...carry, [key]: value }), {})),
    x: metrics.playerX, y: metrics.playerY,
    vx: metrics.playerVelocityX, vy: metrics.playerVelocityY,
  });

  let result = await drive();
  let firstFailure = null;
  if (result?.reached !== true) {
    firstFailure = shape(result, (await actor.metricsState()).uiMetrics || {});
    // One recorded, bounded retry. The historic route timeout is a driver limit cycle that the attempt-shape
    // instrument attributes (attemptPathPx >> attemptNetDisplacementPx) but which has not recurred since the
    // staging-margin fix. Re-drive once from the pilot's actual position so a single rare oscillation cannot
    // red an otherwise green lane; the first failure is retained and the leg is marked retried so a retry is
    // never reported as a clean pass.
    result = await drive();
    if (result?.reached !== true) {
      throw new Error(`${label} did not reach ${JSON.stringify(waypoint)} after one recorded retry: ` +
        JSON.stringify({
          firstFailure,
          retry: shape(result, (await actor.metricsState()).uiMetrics || {}),
        }));
    }
  }
  return {
    waypoint, reached: true,
    pulses: result.pulses,
    distance: result.completed ? null : Math.round(result.distance ?? 0),
    ...(result.completed ? { completed: result.completed } : {}),
    ...(firstFailure ? { retried: true, firstFailure } : {}),
  };
}

// Fly the computed corridor route with real keys and certify that the pilot is resting on
// authored safety. A leg that cannot be driven fails as a blocked leg, naming the waypoint,
// the pulse count, and the pilot's position and velocity.
async function stageChaosSafeZonePilot(actor) {
  // Stage from rest *before* the route is planned. The route is a corridor plan computed from the pilot's
  // position and the driver steers straight at each waypoint, so a brake applied mid-route turns the ship
  // retrograde and lets it coast off the corridor the plan was built on - measured: a leg whose
  // remaining distance was 417 px when it started had the pilot 966 px away and receding at the first sample.
  // Braking before the plan is the only placement that is safe, and it removes the entry that motivated it:
  // that occurrence entered leg 0 at 325 px/s pointed away from its waypoint and spent 7.3 s coasting out to
  // 2224 px before it could turn. A successful leg already settles on arrival, so no per-leg brake is needed.
  const stageBrake = await brakeClientVelocity(actor, 4_000);
  let parked = (await actor.metricsState()).uiMetrics || {};
  let openDoorOriginWait = null;
  if (!authoredRouteGeometry.authoredTerrainHullIsTraversable(parked.playerX, parked.playerY) &&
      hullOverlapsOnlyOpenDoors(authoredRouteGeometry, { ...parked, ...parked.localGameplayState })) {
    const before = { x: parked.playerX, y: parked.playerY, seed: parked.localGameplayState?.doorSeed, timer: parked.localGameplayState?.doorTimer };
    const started = Date.now();
    // Let natural authoritative door closure relocate the resting hull. Do not
    // route through a wall in the conservative static geometry model.
    await waitUntil("open-door origin becomes clear for static route", async () => {
      parked = (await actor.metricsState()).uiMetrics || {};
      return parked.localGameplayState?.authorityPhase === "Live" && parked.localGameplayState?.doorClockFresh === true &&
        authoredRouteGeometry.authoredTerrainHullIsTraversable(parked.playerX, parked.playerY);
    }, 10_000);
    openDoorOriginWait = { before, after: { x: parked.playerX, y: parked.playerY,
      seed: parked.localGameplayState?.doorSeed, timer: parked.localGameplayState?.doorTimer }, elapsedMs: Date.now() - started };
  }
  const route = chaosSafeRouteToAuthoredSafety(chaosSafeMapBytes, {
    from: { x: parked.playerX, y: parked.playerY },
  });
  // Already being on safety skips travel, not the common clearance proof.
  const alreadyInSafety = parked.inSafeZone === true && chaosAuthoredSafe(parked.playerX, parked.playerY);
  const legs = [];
  const startedAt = Date.now();
  // A relocation moves the ship without it flying there, so a plan built for the old position can be arbitrarily
  // wrong - including "the straight line to the next waypoint is blocked from here" - and the driver's own single
  // retry only re-drives the same waypoint from a start it never chose. Measured on this lane: leg 0
  // reached neither its waypoint nor its retry, with `attemptPathPx 5606.6` against `attemptNetDisplacementPx 4164.9`
  // and the target ~460 px from the Chaos wormhole, i.e. inside its 1225 px field. Re-planning from where the ship
  // actually is is the only answer that does not require knowing a teleport happened. Bounded at two re-plans, and a
  // re-planned leg is recorded as such so it can never be read as a clean pass.
  const routeRun = alreadyInSafety ? { legs: [], replans: 0 } : await driveRouteWithReplans({
    initialDrivePoints: route.drivePoints,
    maxReplans: 2,
    currentPosition: async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return { x: metrics.playerX, y: metrics.playerY };
    },
    planFrom: (from) => chaosSafeRouteToAuthoredSafety(chaosSafeMapBytes, { from }),
    driveLeg: async (waypoint, index) => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const remaining = Math.round(Math.hypot(waypoint.x - metrics.playerX, waypoint.y - metrics.playerY));
      return flyChaosRouteLeg(actor, waypoint, {
        tolerance: CHAOS_ROUTE_LEG_ARRIVAL_TOLERANCE_PX,
        // 42px/s is the driver's cruise floor, and a settle stop costs about 20s of braking on top of
        // the flight time; three failed approaches is the point where the driver's own verdict is the
        // answer rather than the budget's.
        timeoutMs: Math.max(60_000, Math.round(remaining / 42 * 1_000) + 20_000),
        label: `Chaos safe-zone route leg ${index} (${remaining}px)`,
      });
    },
  });
  legs.push(...routeRun.legs);
  // Stage at the deepest authored point, not at whatever entry edge the route first touched. The Chaos green
  // here is at most two tiles thick and the lane used to park 2-12 px from the boundary, which put every later
  // assertion about being inside authored safety inside boundary jitter: the same pilot position that let one
  // run pass sat 4 px on the other side of the predicate for another.
  const stagedAt = (await actor.metricsState()).uiMetrics || {};
  const entryAt = { x: stagedAt.playerX ?? 0, y: stagedAt.playerY ?? 0 };
  // Both surfaces, because they disagree here. Authored safety is tile 171; hull-traversable space is the same
  // map measured by the ship's 14 px radius, and a point can sit 23 px inside the first with a hull that overlaps
  // solid tiles of the second. The release case that kept failing was staged exactly like that: 23 px of safety
  // margin, and 2.9 px of travel in ten seconds with thrust held.
  const hullClearAt = (x, y) => authoredRouteGeometry?.authoredHullIsTraversable?.(x, y)
    ?? chaosAuthoredSafe(x, y);
  const safetyMarginAt = authoredSafetyMargin(chaosAuthoredSafe);
  const hullMarginAt = authoredSafetyMargin(hullClearAt);
  const entrySafetyPx = safetyMarginAt(entryAt.x, entryAt.y);
  const entryHullPx = hullMarginAt(entryAt.x, entryAt.y);
  const entryBindingPx = Math.min(entrySafetyPx ?? 0, entryHullPx ?? 0);
  const deepest = deepestStagingPoint({
    isSafe: chaosAuthoredSafe, isHullClear: hullClearAt, from: entryAt,
  });
  if (deepest !== null && (entryBindingPx < 16 || deepest.bindingMarginPx > entryBindingPx + 4)) {
    legs.push(await flyChaosRouteLeg(actor, { x: deepest.x, y: deepest.y }, {
      // The floor can occupy a subpixel-wide centre band of a thin strip.
      // Four-pixel corridor arrival is insufficient for the16px margin proof.
      tolerance: 0.5,
      timeoutMs: 90_000,
      completion: async () => {
        const m = (await actor.metricsState()).uiMetrics || {};
        const safetyPx = safetyMarginAt(m.playerX, m.playerY), hullPx = hullMarginAt(m.playerX, m.playerY);
        if (m.inSafeZone !== true || !brakeVelocityDisposition(m).braked || safetyPx < 16 || hullPx < 16 ||
            safetyPx === null || hullPx === null) return null;
        return { kind: "measured-safety-clearance", x: m.playerX, y: m.playerY, safetyPx, hullPx,
          speed: Math.hypot(m.playerVelocityX, m.playerVelocityY) };
      },
      label: `Chaos staging to the widest spot on both surfaces `
        + `(safety ${deepest.safetyMarginPx}px, hull ${deepest.hullMarginPx}px)`,
    }));
  }
  const braked = await brakeClientVelocity(actor, 10_000);
  const metrics = (await actor.metricsState()).uiMetrics || {};
  // One authored tile of clearance (16 px) is the floor this lane is willing to assert on, and the failure
  // names the deepest spot the region can supply so a future thinning of the green reads as a level change
  // rather than as another unexplained staging timeout.
  const stagedSafetyPx = safetyMarginAt(metrics.playerX ?? 0, metrics.playerY ?? 0);
  const stagedHullPx = hullMarginAt(metrics.playerX ?? 0, metrics.playerY ?? 0);
  // One authored tile on each surface. The safety floor keeps the assertions outside boundary jitter; the hull
  // floor is what keeps the pilot from being parked in a nook it cannot thrust out of.
  if (stagedSafetyPx === null || stagedSafetyPx < 16 || stagedHullPx === null || stagedHullPx < 16) {
    throw new Error("the Chaos staging point does not clear both margin floors: " + JSON.stringify({
      stagedSafetyPx, stagedHullPx, floorPx: 16, x: metrics.playerX, y: metrics.playerY,
      entrySafetyPx, entryHullPx, widestNearbySpot: deepest,
    }));
  }
  console.log(`Chaos staging margins: entry safety ${entrySafetyPx}px hull ${entryHullPx}px -> staged `
    + `safety ${stagedSafetyPx}px hull ${stagedHullPx}px at ${metrics.playerX},${metrics.playerY} `
    + `(floors 16px/16px, widest nearby safety ${deepest?.safetyMarginPx}px hull ${deepest?.hullMarginPx}px)`);
  // The escape heading the route chose was chosen at the BFS arrival tile, and this lane now parks the pilot
  // somewhere else, so the heading has to be recomputed where the pilot actually stands. That is not a tidy-up:
  // measured offline against the shipped Chaos tiles, both recorded failing release positions face north with
  // exitPx=null - thrusting north never leaves authored safety within 1024 px - while the same spots have a west
  // exit at 32-40 px with 72-80 px of hull clearance. A case that orients along a heading which cannot exit is
  // not slow, it is pointed at the wrong place, and no timeout or margin floor can help it.
  const exits = authoredExitHeading({
    isSafe: chaosAuthoredSafe, isHullClear: hullClearAt,
    from: { x: metrics.playerX ?? 0, y: metrics.playerY ?? 0 },
  });
  if (exits.chosen === null) {
    throw new Error("no authored exit heading is available from the Chaos staging point: " + JSON.stringify({
      x: metrics.playerX, y: metrics.playerY, stagedSafetyPx, stagedHullPx,
      candidates: exits.candidates,
    }));
  }
  const stagedEscapeHeading = exits.chosen.heading;
  console.log(`Chaos staging exit: heading ${stagedEscapeHeading.x},${stagedEscapeHeading.y} `
    + `leaves authored safety at ${exits.chosen.exitDistancePx}px `
    + `(blocked: ${exits.candidates.filter((candidate) => !candidate.usable)
      .map((candidate) => `${candidate.heading.x},${candidate.heading.y}`).join(" ") || "none"})`);
  if (metrics.inSafeZone !== true || !chaosAuthoredSafe(metrics.playerX, metrics.playerY)) {
    throw new Error("the Chaos safe-zone route did not deliver the pilot onto authored safety: " +
      JSON.stringify({
        x: metrics.playerX, y: metrics.playerY, inSafeZone: metrics.inSafeZone,
        stage: route.stage, stageTile: route.stageTile,
        braked: braked?.braked === true, brakeReason: braked?.reason ?? null,
      }));
  }
  return {
    navigated: !alreadyInSafety, from: { x: parked.playerX, y: parked.playerY },
    stage: route.stage, stageTile: route.stageTile, hops: route.hops,
    waypoints: route.waypoints.length, legLengths: route.legLengths,
    // Recomputed at the staged position, not inherited from the route's stage tile - see the exit probe above.
    escapeHeading: stagedEscapeHeading, safeRun: route.safeRun,
    drivePoints: route.drivePoints.length,
    stageSeconds: Math.round((Date.now() - startedAt) / 1000),
    hopCount: legs.reduce((total, leg) => total + (leg.hops ?? 0), 0),
    legs: legs.length, failedLeg: legs.at(-1) ?? null, braked: braked?.braked === true,
    // The pre-plan brake that decides whether the route was entered from rest at all. Reported beside the
    // post-route staging brake so a renewed lane can tell "entered stopped" from "entered at speed".
    openDoorOriginWait,
      stageBrake: { braked: stageBrake?.braked === true, reason: stageBrake?.reason ?? null,
      speed: stageBrake?.speed ?? null },
    // Read before trusting a clean route report: a leg that only passed after the recorded retry is not a clean
    // run, so a renewed lane can tell an unattributed oscillation from a route that closed on the first drive.
    routeRetries: legs.filter((leg) => leg.retried === true).length,
    routeRetryFirstFailures: legs.filter((leg) => leg.retried === true)
      .map((leg) => ({ waypoint: leg.waypoint, firstFailure: leg.firstFailure })),
    // A re-planned leg is not a clean pass either, and for the same reason: it reports that the plan the lane
    // started with was invalidated by something that moved the ship without flying it there.
    routeReplans: routeRun.replans,
    routeReplanFailures: legs.filter((leg) => leg.replanned === true)
      .map((leg) => ({ waypoint: leg.waypoint, from: leg.from, reason: leg.reason })),
  };
}

if (!new Set(["chrome", "firefox"]).has(browser)) {
  throw new Error("ZUBSPACE_E2E_BROWSER must be chrome or firefox");
}
if (!new Set(["chaos-zone", "extreme"]).has(arena)) {
  throw new Error("keyboard chord matrix is scoped to chaos-zone and extreme");
}
if (process.env.ZUBSPACE_SMOKE_BOT_COUNT !== "0") {
  throw new Error("pre-respawn keyboard matrix requires ZUBSPACE_SMOKE_BOT_COUNT=0");
}

async function keyboardGraphicsEvidence(chromeClient, firefox) {
  const expression = `(() => {
    if (typeof gl === 'undefined' || !gl) return { available: false };
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    return { available: true, version: gl.getParameter(gl.VERSION), attributes: gl.getContextAttributes(),
      renderer: ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER) };
  })()`;
  const actual = browser === "chrome" ? await chromeClient.evaluate(expression)
    : await execute(firefox, `return (${expression});`);
  return { requestedBackend: browser === "chrome" ?
    (chromeGraphicsBackend === "native" ? "automatic" : chromeGraphicsBackend) : "browser-default", ...actual };
}

async function firefoxKeyboardState(driver) {
  return execute(
    driver,
    `return {
      uiMetrics: window.__zubspaceUiMetrics || null,
      gameKeyBridge: window.__zubspaceGameKeyStateForTest?.() || null,
    };`,
  );
}

async function firefoxHeldWindowState(driver, startedAt, endedAt) {
  return execute(
    driver,
    `const [startedAt, endedAt] = arguments;
     const frames = (window.__zubspaceMetricsTrace || [])
       .filter((frame) => frame.at >= startedAt && frame.at <= endedAt)
       .map((frame) => ({
         at: frame.at,
         playerAlive: frame.playerAlive,
         serverPlayerAlive: frame.serverPlayerAlive,
         playerLifeSeq: frame.playerLifeSeq,
         playerRespawnSeq: frame.playerRespawnSeq,
         playerOrientation: frame.playerOrientation,
         clientInputSeq: frame.clientInputSeq,
         pendingWeapon: frame.pendingWeapon,
       }));
     return {
       uiMetrics: window.__zubspaceUiMetrics || null,
       metricsTrace: frames,
     };`,
    [startedAt, endedAt],
  );
}

async function fetchStatus(port) {
  const response = await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`);
  if (!response.ok) throw new Error(`status failed with HTTP ${response.status}`);
  return response.json();
}

async function waitForCleanup(port) {
  let status = null;
  await waitUntil("keyboard session cleanup", async () => {
    status = await fetchStatus(port);
    return status.clients === 0 && status.joinedPlayers === 0 && status.rtc_sessions === 0 &&
      status.rtc_outbound_queue_packets === 0 && status.rtc_outbound_queue_bytes === 0 &&
      Object.values(status.rtc_inbound_queue_packets || {}).every((count) => count === 0);
  }, 8_000);
  return status;
}

// The staged Extreme chord pilot has to be a place where a chord is
// measurable: outside the authored safe surface by the authored predicate, and
// facing a corridor with room for the whole measurement window. The retained
// Firefox/Extreme after-respawn lane early-returned from this staging with the
// pilot standing on authored safe tile 171 at (8047, 8368), because a
// `settled.braked` result alone satisfied the old guard while the browser's
// momentary safe-zone report said `false`. That lane sampled 3.68 px/s after a
// 1.08 s thrust+Afterburner hold and failed its own motion clause. The cause was
// not thrust damping: there is no such term in the server, where `in_safe`
// reaches only weapon handling. What this guard actually needs is the runway
// chosen below.
function extremeChordStageIsReady(metrics) {
  return metrics.inSafeZone === false &&
    Number.isFinite(metrics.playerX) && Number.isFinite(metrics.playerY) &&
    cannotReenterSafetyDuringChord(metrics);
}

// An Afterburner chord thrusts along whatever heading the pilot happens to
// hold. The retained stall held `input_direction: 30` (due west) from the
// staged waypoint back toward the central green, so the heading is chosen from
// authored geometry instead of inherited from whichever phase ran before.
function extremeChordBoostHeading(metrics) {
  return bestBoostHeading({
    hullIsTraversable: (x, y) => authoredRouteGeometry.authoredHullIsTraversable(x, y),
    safetyDistance: distanceToAuthoredSafety,
    x: metrics.playerX,
    y: metrics.playerY,
    // 180ms of stagger plus the 900ms hold travels under 380px at the authored
    // 350 px/s top speed; 512px absorbs braking and movement correction.
    travelPx: 512,
    clearancePx: 128,
  });
}

async function extremeChordStageReading(actor) {
  const metrics = (await actor.metricsState()).uiMetrics || {};
  return {
    metrics,
    ready: extremeChordStageIsReady(metrics),
    heading: extremeChordBoostHeading(metrics),
  };
}

// One 60 Hz key-pulse quantum, the same physical limit `brakeVelocityDisposition`
// (tests/e2e/combat_replay.mjs:4659) accepts as rest. Below it the pilot cannot coast a
// measurable distance before the chord is dispatched.
const STAGE_REST_SPEED_PX_S = 3.5;
const RUNWAY_PROBE_PX = 512;

// A runway is only certified for the position it was probed from and the stillness of the
// ship that probed it. Both were assumed here: the brake's disposition was discarded and
// nothing re-measured the corridor after the pilot settled, so a stage accepted at 100
// px/s coasted ~30 px before the chord ran. Chrome/Extreme: clearance was
// certified at (10476.4, 8298.6), the chord ran at (10451.1, 8329.1), and the boost went
// west into the authored wall face at x=10416 - the lane then reported the server's
// correct BounceFactor reflection as an Afterburner motion failure. Ask the question the
// measurement actually depends on.
function runwayClearFrom(heading, metrics) {
  const x = metrics?.playerX;
  const y = metrics?.playerY;
  if (heading == null || !Number.isFinite(x) || !Number.isFinite(y)) return false;
  return authoredRouteGeometry.authoredLineIsClear(
    x,
    y,
    x + heading.dx * RUNWAY_PROBE_PX,
    y + heading.dy * RUNWAY_PROBE_PX,
  );
}

async function prepareExtremeChordAttempt(actor, options = {}) {
  // Staging is navigation, not a chord, so it must not inherit a held key. holdThenChordFirefoxKeys takes a
  // releaseOrder that may deliberately leave a key down for a later step, and nothing in this file returned
  // the keyboard to idle between attempts. A pilot staged while still driven sits at a terminal thrust speed
  // that drag never decays, which is what the two signatures this gate kept producing looked like: a rest
  // ceiling that twelve seconds could not satisfy (measured: 4.52 px/s at waitedMs 12077 - a flat speed, not
  // a decay curve) and route blocked, whose check is displacement-only and therefore trips on a ship being
  // driven in whatever direction it happens to face. The speed below is reported before and after the release
  // so this is evidence rather than a guess: a pilot already moving when staging begins was being held.
  let inheritedSpeedPxS = null;
  try {
    const inherited = (await actor.metricsState()).uiMetrics || {};
    // The page publishes these as metrics.playerVelocityX ?? null, so absence must stay unknown rather than
    // becoming a confident zero: a staging report that claims the pilot was idle because a field was missing
    // is worse than one that says nothing.
    const vx = Number(inherited.playerVelocityX);
    const vy = Number(inherited.playerVelocityY);
    if (Number.isFinite(vx) && Number.isFinite(vy)) {
      inheritedSpeedPxS = Number(Math.hypot(vx, vy).toFixed(2));
    }
  } catch { }
  if (typeof actor.releaseKeys === "function") await actor.releaseKeys();
  if (inheritedSpeedPxS !== null) {
    console.log(`Extreme chord staging: released the inherited keyboard at ${inheritedSpeedPxS} px/s `
      + `(rest ceiling 3.5 px/s); a pilot already moving when staging begins was still being driven`);
  }
  // Admit chords only from authored nonsafe geometry. An afterburner
  // measurement additionally needs a full clear runway from rest. Find it
  // from the actual current position: a previous boost can leave the pilot
  // behind a wall that makes a direct return to a fixed waypoint impossible.
  const requiresRunway = options.afterburner === true;
  let stage = await extremeChordStageReading(actor);
  // A runway only means something from rest. This fast path used to skip braking
  // whenever the pilot already looked staged, so a ship still coasting at ~210 px/s
  // from the previous case certified a corridor and then travelled out of it before the
  // chord ran. An Afterburner attempt therefore always brakes first and carries the
  // disposition into the acceptance test below.
  if (requiresRunway) {
    const braked = await brakeClientVelocity(actor, 10_000);
    stage = await extremeChordStageReading(actor);
    stage.braked = braked?.braked === true;
    stage.brakeReason = braked?.braked ? null : (braked?.reason ?? "no disposition");
  }
  if (!stage.ready || (requiresRunway && (!stage.heading || !stage.braked))) {
    if (!requiresRunway && stage.metrics.inSafeZone === false) {
      await brakeClientVelocity(actor, 10_000);
      stage = await extremeChordStageReading(actor);
    }
    if (!stage.ready || (requiresRunway && (!stage.heading || !stage.braked))) {
      const stageRoutes = [];
      let anyRouteReached = false;
      const origin = (await actor.metricsState()).uiMetrics || {};
      const plan = extremeChordStageRoute({
        geometry: authoredRouteGeometry,
        safetyDistance: distanceToAuthoredSafety,
        from: { x: origin.playerX, y: origin.playerY },
        requiresRunway,
      });
      if (!plan) {
        throw new Error(`no reachable authored Extreme chord stage: ${JSON.stringify({
          from: { x: origin.playerX, y: origin.playerY }, requiresRunway,
        })}`);
      }
      for (const target of [plan.target]) {
        const from = (await actor.metricsState()).uiMetrics || {};
        const route = await driveClientToWorldTarget(
          actor,
          target,
          30_000,
          true,
          8,
          null,
          75,
          0.35,
          15_000,
        );
        const attempt = {
          target,
          from: { x: from.playerX, y: from.playerY, inSafeZone: from.inSafeZone },
          route,
          plan,
          stage: null,
        };
        stageRoutes.push(attempt);
        if (!route.reached) continue;
        anyRouteReached = true;
        const braked = await brakeClientVelocity(actor, 10_000);
        const staged = await extremeChordStageReading(actor);
        staged.braked = braked?.braked === true;
        staged.brakeReason = braked?.braked ? null : (braked?.reason ?? "no disposition");
        attempt.stage = {
          ready: staged.ready,
          runway: staged.heading ?? null,
          braked: staged.braked,
          brakeReason: staged.brakeReason,
          position: {
            x: staged.metrics.playerX,
            y: staged.metrics.playerY,
            speed: Math.hypot(
              staged.metrics.playerVelocityX ?? 0,
              staged.metrics.playerVelocityY ?? 0,
            ),
            inSafeZone: staged.metrics.inSafeZone,
            playerAlive: staged.metrics.playerAlive,
            serverPlayerAlive: staged.metrics.serverPlayerAlive,
          },
        };
        if (staged.ready && (!requiresRunway || (staged.braked && staged.heading))) {
          stage = staged;
          break;
        }
      }
      // Retain every candidate's own route plus the stage it produced, and where
      // the pilot stood when it was attempted. A direct route from the west side
      // of the central walls is correctly reported as blocked, and a reachable
      // lane can still lack the Afterburner runway; those two look identical
      // unless the evidence names the ship's position.
      console.log(`Extreme chord staging: ${JSON.stringify({
        requiresRunway,
        attempts: stageRoutes.map(({ target, from, route, plan, stage: staged }) => ({
          target,
          from,
          plan,
          reached: route.reached,
          reason: route.reason ?? null,
          stage: staged,
        })),
      })}`);
      if (!anyRouteReached) {
        throw new Error(`Extreme chord safety route did not converge: ${JSON.stringify({ routes: stageRoutes })}`);
      }
    }
  }
  await waitUntil("Extreme chord pilot holds an authored nonsafe stage", async () => {
    const next = await extremeChordStageReading(actor);
    if (next.heading) stage = next;
    return next.metrics.playerAlive === true && next.metrics.serverPlayerAlive === true &&
      next.ready && (!requiresRunway || next.heading != null);
  }, 5_000);
  if (!requiresRunway) return null;
  const heading = stage.heading ?? (await extremeChordStageReading(actor)).heading;
  if (!heading) {
    throw new Error(`no authored Extreme chord runway at the staged pilot: ${JSON.stringify({
      metrics: compactMetrics(stage.metrics),
      distanceToAuthoredSafety: distanceToAuthoredSafety(
        stage.metrics.playerX, stage.metrics.playerY,
      ),
    })}`);
  }
  const oriented = await orientClientForProtocolDirection(actor, heading.dx, heading.dy, 3_000);
  if (!oriented) {
    throw new Error(`Extreme chord pilot could not face the authored runway: ${JSON.stringify({
      heading,
      metrics: compactMetrics(stage.metrics),
    })}`);
  }
  // Turning is a real manoeuvre, so the pilot can drift while it orients and the chord
  // fires wherever it ends up. Re-measure both stillness and corridor from that final
  // position: the certified runway must be the one the boost actually travels.
  // The pilot stops when the continuum's drag takes it there, not when the harness gets impatient: a
  // Firefox/Extreme staging attempt failed here with the corridor certified and the pilot still drifting at
  // 5.72 px/s against a 3.5 px/s ceiling, which is a deadline that was too short rather than a pilot that
  // would not settle. Three times the window, and the ceiling is reported in the failure so a future
  // reading says which bound it broke instead of leaving the reader to find the constant.
  // Brake to rest; do not wait for rest. src/game/player/ship.rs has no damping, friction or drag term, so a
  // coasting pilot keeps its velocity indefinitely - a measured failing case held 4.46 px/s for twelve seconds
  // with inputBits 0 on every sample. A pilot already at or below the threshold reports braked on the first
  // call, so this costs nothing when there is nothing to fix; what it removes is a twelve-second wait for a
  // decay the engine cannot produce. The disposition and its reason are carried into the certification failure
  // below, so a broken brake says so where it broke instead of surfacing as a symptom at the deadline.
  let stageBrake = null;
  for (let brakeAttempt = 0; brakeAttempt < 3; brakeAttempt += 1) {
    stageBrake = await brakeClientVelocity(actor, 10_000);
    if (stageBrake?.braked) break;
  }
  const restDeadline = Date.now() + 12_000;
  const restStartedAt = Date.now();
  let resting = null;
  let lastRestSpeed = null;
  let lastRestSampleLoggedAt = 0;
  let restMetricKeysLogged = false;
  while (Date.now() < restDeadline) {
    const candidate = await extremeChordStageReading(actor);
    if (!restMetricKeysLogged) {
      // Presence reported separately from value, because a ?? "?" print cannot tell a missing key from a real
      // null and that ambiguity cost two turns. One line, once: what this lane can actually read.
      restMetricKeysLogged = true;
      console.log(`Extreme chord staging: rest reading keys are ${Object.keys(candidate.metrics || {}).sort().join(",")}; `
        + `localRenderPrediction present=${candidate.metrics?.localRenderPrediction === undefined ? "no" : "yes"} `
        + `(${JSON.stringify(candidate.metrics?.localRenderPrediction ?? null)})`);
    }
    const candidateSpeed = Math.hypot(
      candidate.metrics.playerVelocityX ?? 0,
      candidate.metrics.playerVelocityY ?? 0,
    );
    // One sample per second, so a failure leaves a curve rather than a last value. Three shapes mean three
    // different things and only one of them is a window that is too short: a speed marching toward the ceiling
    // is drag taking longer than the deadline; a speed that plateaus above it is something nudging the ship;
    // a speed that never changes at all is a pilot still being driven. Note the scale while reading the
    // numbers - this same pair of fields has been observed in the hundreds on a ship at full thrust, so the
    // 3.5 ceiling is a small fraction of what this metric can report and "nearly stopped" is relative.
    if (Date.now() - lastRestSampleLoggedAt >= 1000) {
      lastRestSampleLoggedAt = Date.now();
      // latestServerTick and clientInputSeq decide what a constant speed means. If the tick advances while
      // speed and position repeat to the digit, the ship really is being held at that speed by something.
      // If the tick is frozen too, the snapshot itself stopped updating and every number read from it - this
      // ceiling check and the route planner's displacement check alike - is stale rather than true.
      console.log(`Extreme chord staging: rest poll ${Date.now() - restStartedAt} ms, speed `
        + `${candidateSpeed.toFixed(2)} px/s, ceiling ${STAGE_REST_SPEED_PX_S} px/s, `
        + `position ${candidate.metrics.playerX ?? "?"},${candidate.metrics.playerY ?? "?"}, `
        + `serverTick ${candidate.metrics.latestServerTick ?? "?"}, `
        + `inputSeq ${candidate.metrics.clientInputSeq ?? "?"}, `
        // The decisive field. A settle that stalls while inputBits still has thrust (1), reverse (2) or
        // afterburner (16) set is a ship being driven, whatever the harness did to the keyboard beforehand.
        + `inputBits ${JSON.stringify(candidate.metrics.localRenderPrediction?.inputBits ?? null)}`);
    }
    if (candidate.ready &&
      candidate.metrics.playerAlive === true &&
      candidate.metrics.serverPlayerAlive === true &&
      candidateSpeed <= STAGE_REST_SPEED_PX_S &&
      runwayClearFrom(heading, candidate.metrics)) {
      resting = candidate;
      // The window is reported because a deadline nobody can see is a number nobody can defend. Measured
      // here, Chrome/Extreme settles in 11-18 ms, so 12 s is a very wide window for that
      // configuration and is not justified by it. The Firefox/Extreme attempt that motivated the change
      // had polled for the full 4 s and was still at 5.72 px/s - a pilot that was not settling at all on
      // that pass - so the longer window is an observation that it eventually settles, not an explanation
      // of why it did not settle sooner. Whoever sees a Firefox/Extreme settle above a second or two
      // should treat that as the finding, not as a reason to raise the deadline again.
      console.log(`Extreme chord staging: pilot settled after ${Date.now() - restStartedAt} ms at `
        + `${candidateSpeed.toFixed(2)} px/s against the ${STAGE_REST_SPEED_PX_S} px/s ceiling`);
      break;
    }
    lastRestSpeed = candidateSpeed;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (!resting) {
    const last = await extremeChordStageReading(actor);
    throw new Error(`Extreme Afterburner runway is not certified at the resting pilot: ${JSON.stringify({
      heading,
      restSpeedCeilingPxS: STAGE_REST_SPEED_PX_S,
      stagingBrake: { braked: stageBrake?.braked ?? null, reason: stageBrake?.reason ?? null },
      waitedMs: Date.now() - restStartedAt,
      lastSeenSpeedBeforeGivingUp: lastRestSpeed,
      position: { x: last.metrics.playerX, y: last.metrics.playerY },
      speed: Math.hypot(last.metrics.playerVelocityX ?? 0, last.metrics.playerVelocityY ?? 0),
      inSafeZone: last.metrics.inSafeZone,
      runwayClear: runwayClearFrom(heading, last.metrics),
      distanceToAuthoredSafety: distanceToAuthoredSafety(
        last.metrics.playerX, last.metrics.playerY,
      ),
    })}`);
  }
  return heading;
}

// Chaos' Afterburner matrix needs the same thing the Extreme staging exists for, and was missing it: a chord
// thrusts along whatever heading the pilot holds, so a pilot resting against terrain produces the boost's sound
// and its energy cost with no travel at all. Measured at (8200, 9090): the tile centre is open while
// the hull is blocked 17-of-17 samples to the south, the chord thrust due south, and the case reported
// `maxDisplacementDuring 0.86 px` with `afterburnerActive true` and the cost charged.
//
// Random respawns can be in narrow branches. Keep the no-input fast path for
// an already valid runway, with bounded ordinary-key recovery when needed.
// Authored safety does not damp Chaos thrust; the full corridor is still required.
async function prepareChaosChordAttempt(actor, options = {}) {
  // Only the Afterburner matrix asserts ship motion, so only it needs a runway. The same `prepareAttempt` is
  // handed to matrices that assert no motion at all; staging them would be unrequested travel.
  if (options.afterburner !== true || authoredRouteGeometry === null) return null;
  const hullIsTraversable = (x, y) => authoredRouteGeometry.authoredHullIsTraversable(x, y);
  // Fast path: if the pilot already holds rest *and* a runway, nothing needs to touch the keyboard. This
  // staging runs immediately before the lane's 90 Hz draw-cadence probe, and eleven runs without an
  // unconditional staging never tripped that probe while the two runs that staged every time did (measuredHz
  // 89.89 against a 90 floor, plus one ring-eviction reading). The rest half is not optional: Extreme's fast
  // path once skipped braking for a ship that merely looked staged, and it then coasted at ~210 px/s out of the
  // corridor it had just certified.
  const held = (await actor.metricsState()).uiMetrics || {};
  if (brakeVelocityDisposition(held).braked) {
    const orientation = ((held.playerOrientation % 1) + 1) % 1;
    const currentRunway = runwayAlongHeading({
      hullIsTraversable,
      safetyDistance: distanceToAuthoredSafety,
      x: held.playerX,
      y: held.playerY,
      // The engine's heading for an orientation: `orientation_to_heading` -> `(sin, -cos)` in world space, which
      // is the same space `bestBoostHeading` and `orientClientForProtocolDirection` use.
      dx: Math.sin(orientation * Math.PI * 2),
      dy: -Math.cos(orientation * Math.PI * 2),
      travelPx: 512,
    });
    if (currentRunway && currentRunway.runwayPx >= 512) {
      const stage = {
        staged: false,
        runway: currentRunway,
        braked: true,
        brakeReason: null,
        from: { x: held.playerX, y: held.playerY },
        position: {
          x: held.playerX,
          y: held.playerY,
          speed: Number(Math.hypot(held.playerVelocityX ?? 0, held.playerVelocityY ?? 0).toFixed(2)),
          inSafeZone: held.inSafeZone,
          playerAlive: held.playerAlive,
          serverPlayerAlive: held.serverPlayerAlive,
        },
      };
      console.log(`Chaos chord staging: reused the pilot's existing rest and runway ${JSON.stringify(stage)}`);
      return stage;
    }
  }
  if (typeof actor.releaseKeys === "function") await actor.releaseKeys();
  await brakeClientVelocity(actor, 10_000);
  let before = (await actor.metricsState()).uiMetrics || {};
  const findHeading = metrics => bestBoostHeading({
    hullIsTraversable,
    safetyDistance: distanceToAuthoredSafety,
    x: metrics.playerX,
    y: metrics.playerY,
    // The Extreme runway figure: 180 ms of stagger plus the 900 ms hold travels under 380 px at Chaos' 325 px/s
    // top speed, and 512 px absorbs braking and movement correction. Clearance is deliberately not required
    // here - see the header comment.
    travelPx: 512,
    clearancePx: 0,
  });
  let heading = findHeading(before), originRecovery = null, runwayRoute = null;
  if (!heading) {
    // A genuine random respawn can land in a narrow branch. Restore a valid
    // route origin through the ordinary Warp protocol if needed, then fly to
    // the authored central runway. All of this precedes the measured chord.
    if (!options.server) throw new Error("Chaos runway recovery requires its real server context");
    const deadline = Date.now() + 90_000;
    originRecovery = await prepareKeyboardLifecycleOrigin({ actor, server: options.server,
      playerId: before.selfPlayerId, initialLifeSeq: before.playerLifeSeq,
      geometry: authoredRouteGeometry, deadline });
    before = (await actor.metricsState()).uiMetrics || {};
    heading = findHeading(before);
    if (!heading) {
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new Error("Chaos runway recovery exceeded its setup budget");
      runwayRoute = await navigateAuthoredTarget(actor, { x: 8200, y: 8200 }, options.server, {
        timeoutMs: remaining, arrivalRadiusPx: 8, maximumDesiredSpeed: 250, desiredSpeedScale: 0.8,
        // The centre is a fallback destination, not the chord precondition.
        // Stop at an earlier resting pilot with the same full512px runway;
        // the actual heading/clearance is certified again after turning below.
        complete: async () => {
          const candidate = (await actor.metricsState()).uiMetrics || {};
          return candidate.playerAlive === true && candidate.serverPlayerAlive === true &&
            brakeVelocityDisposition(candidate).braked && findHeading(candidate) !== null;
        },
      });
      before = (await actor.metricsState()).uiMetrics || {};
      heading = findHeading(before);
    }
  }
  if (!heading) {
    throw new Error(`no authored Chaos chord runway at the pilot: ${JSON.stringify({
      metrics: compactMetrics(before),
      distanceToAuthoredSafety: distanceToAuthoredSafety(before.playerX, before.playerY),
    })}`);
  }
  const oriented = await orientClientForProtocolDirection(actor, heading.dx, heading.dy, 3_000);
  if (!oriented?.oriented) {
    throw new Error(`the Chaos chord pilot would not turn onto its runway ${JSON.stringify(heading)}: `
      + JSON.stringify({ metrics: compactMetrics((await actor.metricsState()).uiMetrics || {}) }));
  }
  // Turning is a coast for a ship with no drag, so re-brake and certify from where it actually ended up.
  const settled = await brakeClientVelocity(actor, 10_000);
  await waitUntil("Chaos chord pilot holds a certified runway at rest", async () => {
    const next = (await actor.metricsState()).uiMetrics || {};
    return next.playerAlive === true && next.serverPlayerAlive === true &&
      brakeVelocityDisposition(next).braked;
  }, 6_000);
  const after = (await actor.metricsState()).uiMetrics || {};
  const actualOrientation = after.playerOrientation * Math.PI * 2;
  const actualRunway = runwayAlongHeading({ hullIsTraversable, safetyDistance: distanceToAuthoredSafety,
    x: after.playerX, y: after.playerY, dx: Math.sin(actualOrientation),
    dy: -Math.cos(actualOrientation), travelPx: 512 });
  if (!actualRunway || actualRunway.runwayPx < 512) {
    throw new Error("Chaos chord pilot's actual settled heading lacks its512px runway");
  }
  const stage = {
    staged: true,
    originRecovery,
    runwayRoute,
    runway: actualRunway,
    braked: settled?.braked === true,
    brakeReason: settled?.braked ? null : (settled?.reason ?? "no disposition"),
    from: { x: before.playerX, y: before.playerY },
    position: {
      x: after.playerX,
      y: after.playerY,
      speed: Number(Math.hypot(after.playerVelocityX ?? 0, after.playerVelocityY ?? 0).toFixed(2)),
      inSafeZone: after.inSafeZone,
      playerAlive: after.playerAlive,
      serverPlayerAlive: after.serverPlayerAlive,
    },
  };
  console.log(`Chaos chord staging: ${JSON.stringify(stage)}`);
  return stage;
}

async function settleLifecycleClient(actor, timeoutMs = 30_000) {
  const quickBrakeMs = Math.min(8_000, timeoutMs);
  const quickBrake = await brakeClientVelocity(actor, quickBrakeMs);
  let metrics = (await actor.metricsState()).uiMetrics || {};
  let speed = Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY);
  if (Number.isFinite(speed) && speed <= 8) {
    return { settled: true, speed, quickBrake, metrics };
  }

  // At low Extreme velocities a minimum-duration reverse-thrust pulse can
  // oscillate around zero without crossing brakeClientVelocity's 3 px/s
  // threshold. Finish with the ordinary keyboard feedback controller anchored
  // at the ship's current authoritative point. This remains a real movement
  // path and proves the peer is settled instead of merely widening the
  // acceptance threshold.
  const target = { x: metrics.playerX, y: metrics.playerY };
  const feedbackSettle = await driveClientToWorldTarget(
    actor,
    target,
    Math.max(5_000, timeoutMs - quickBrakeMs),
    true,
    64,
    null,
    30,
    0.2,
    Math.max(5_000, timeoutMs - quickBrakeMs),
  );
  metrics = (await actor.metricsState()).uiMetrics || {};
  speed = Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY);
  const trimPulses = [];
  // The world-target controller deliberately stops before oscillating around
  // a destination. For a firing lane we need a tighter velocity guarantee.
  // Finish any small residual with one-frame, direction-quantized reverse
  // thrust pulses through the same genuine browser input path.
  for (let pulse = 0; speed > 8 && pulse < 4; pulse += 1) {
    const oriented = await orientClientForProtocolDirection(
      actor,
      -metrics.playerVelocityX,
      -metrics.playerVelocityY,
      3_000,
    );
    if (!oriented.oriented) {
      trimPulses.push({ pulse, oriented, speed });
      break;
    }
    await actor.hold("w", 20);
    await sleep(80);
    metrics = (await actor.metricsState()).uiMetrics || {};
    speed = Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY);
    trimPulses.push({ pulse, oriented, speed });
  }
  return {
    settled: Number.isFinite(speed) && speed <= 8,
    speed,
    quickBrake,
    feedbackSettle,
    trimPulses,
    metrics,
  };
}

async function driveLifecyclePeersOutsideSafety(actor, attacker) {
  const plannedHop = (from, victim) => {
    const waypoint = routedMapCounterfireWaypoint({ metrics: from,
      target: { positionX: victim.playerX, positionY: victim.playerY },
      ...authoredRouteGeometry });
    if (!waypoint) return null;
    const distance = Math.hypot(waypoint.x - from.playerX, waypoint.y - from.playerY);
    if (distance <= 768) return waypoint;
    // A random-spawn4619px clear leg cannot fit the existing90s/42px/s
    // stage bound. Drive a prefix of that same hull-clear line and replan;
    // a truncated direct destination is still transit, never a firing stage.
    return { ...waypoint, x: from.playerX + (waypoint.x - from.playerX) * 768 / distance,
      y: from.playerY + (waypoint.y - from.playerY) * 768 / distance,
      distance: 768, routed: true,
      unboundedDestination: { x: waypoint.x, y: waypoint.y, routed: waypoint.routed, distance } };
  };
  if (arena === "extreme") {
    // The chord matrix can leave the Firefox pilot materially east of the
    // original waypoint. Stop that exact authoritative ship first, then stage
    // the attacker relative to its settled position. Fixed absolute lanes let
    // a coasting victim cross the bullet path while the second browser spends
    // time navigating the large map bootstrap.
    await prepareExtremeChordAttempt(actor);
    const victimBrake = await settleLifecycleClient(actor, 30_000);
    let victim = (await actor.metricsState()).uiMetrics || {};
    const victimSpeed = Math.hypot(victim.playerVelocityX, victim.playerVelocityY);
    if (!Number.isFinite(victimSpeed) || victimSpeed > 8) {
      throw new Error(`extreme lifecycle victim did not settle: ${JSON.stringify({
        victimBrake,
        speed: victimSpeed,
        x: victim.playerX,
        y: victim.playerY,
      })}`);
    }
    // A pilot can drift to either side of the central walls. Reuse the
    // counterfire planner's hull-clear map route, not an assumed open column.
    const routeEvidence = [];
    let staged = false;
    let stalledStages = 0;
    for (let stage = 0; stage < 40; stage += 1) {
      victim = (await actor.metricsState()).uiMetrics || {};
      const from = (await attacker.metricsState()).uiMetrics || {};
      const target = plannedHop(from, victim);
      if (!target) throw new Error(`no hull-clear lifecycle route: ${JSON.stringify({ from, victim, routeEvidence })}`);
      const separation = Math.hypot(
        target.x - (from.playerX ?? 0),
        target.y - (from.playerY ?? 0),
      );
      const route = await driveClientToWorldTarget(
        attacker,
        target,
        // The nearest hull-clear firing candidate can be far: the retained
        // Chrome/Extreme stall settled the victim 1,094 px from the attacker's
        // spawn and offered one direct waypoint 914 px away, which the fixed
        // 30 s budget could not cross at the 42 px/s desired-speed cap (it
        // timed out 290 px short). The stage bound is a function of the hop, so
        // both staging loops share `lifecycleRouteBudgetMs`.
        lifecycleRouteBudgetMs(separation, 42),
        target.routed,
        target.routed ? 4 : 24,
        null,
        42,
        0.25,
        20_000,
      );
      routeEvidence.push({ from: { x: from.playerX, y: from.playerY }, victim: { x: victim.playerX, y: victim.playerY }, target, route });
      if (!routedHopAccepted(route)) {
        // A blocked hop is a shape this staging has to survive, not a verdict. The retained stage
        // wedged 502 px short of a direct waypoint whose line the shipped map reports as 0-of-1001
        // hull-blocked, because the attacker had drifted off that line; throwing here skipped the 40-stage
        // re-planning budget below, so no bounded retry was ever reachable for the blocked shape. Settle,
        // count the stage as stalled, and re-plan from where the attacker actually is.
        await settleLifecycleClient(attacker, 10_000);
        if ((stalledStages += 1) >= 2) {
          throw new Error(`extreme lifecycle attacker could not reach its firing lane: ${JSON.stringify({
            stalledStages,
            routeEvidence,
          })}`);
        }
        continue;
      }
      await settleLifecycleClient(attacker, 20_000);
      if (!target.routed) { staged = true; break; }
      // Stage success is measured by movement, not point arrival: a routed
      // waypoint inside the driver's stop precision is a corridor micro-hop, and a
      // legal route around a wall is allowed to raise the victim distance while it
      // works the remaining path down. Two stages that do not move the ship is a
      // stall.
      const after = (await attacker.metricsState()).uiMetrics || {};
      const victimDistancePx = Math.hypot(
        victim.playerX - after.playerX,
        victim.playerY - after.playerY,
      );
      if (stageAdvanced({ from, after })) {
        stalledStages = 0;
      } else if ((stalledStages += 1) >= 2) {
        throw new Error(`extreme lifecycle route livelock: ${JSON.stringify({
          from: { x: from.playerX, y: from.playerY },
          after: { x: after.playerX, y: after.playerY },
          victimDistancePx,
          routeEvidence,
        })}`);
      }
    }
    if (!staged) throw new Error(`lifecycle route exceeded bounded waypoint count: ${JSON.stringify(routeEvidence)}`);
    victim = (await actor.metricsState()).uiMetrics || {};
    const attackerMetrics = (await attacker.metricsState()).uiMetrics || {};
    const attackerSpeed = Math.hypot(
      attackerMetrics.playerVelocityX,
      attackerMetrics.playerVelocityY,
    );
    const firingDistance = Math.hypot(
      victim.playerX - attackerMetrics.playerX,
      victim.playerY - attackerMetrics.playerY,
    );
    if (
      victim.inSafeZone === true ||
      attackerMetrics.inSafeZone === true ||
      !Number.isFinite(attackerSpeed) ||
      attackerSpeed > 8 ||
      !Number.isFinite(firingDistance)
    ) {
      throw new Error(`extreme lifecycle relative firing lane is invalid: ${JSON.stringify({
        victim: {
          x: victim.playerX,
          y: victim.playerY,
          velocityX: victim.playerVelocityX,
          velocityY: victim.playerVelocityY,
          inSafeZone: victim.inSafeZone,
        },
        attacker: {
          x: attackerMetrics.playerX,
          y: attackerMetrics.playerY,
          velocityX: attackerMetrics.playerVelocityX,
          velocityY: attackerMetrics.playerVelocityY,
          speed: attackerSpeed,
          inSafeZone: attackerMetrics.inSafeZone,
        },
        firingDistance,
      })}`);
    }
    // A low but nonzero protocol velocity can move the victim materially
    // during Firefox's real-key route. aimLifecycleAttackerAtVictim owns the
    // final <=160-pixel lane and re-samples both ships before every approach.
    return { routeEvidence };
  }

  // Chaos chords never apply thrust, and the authored spawn is already well
  // outside safety. Keep the measured browser at that exact stationary point
  // and move only the attacker; routing the victim first adds no coverage and
  // can create an avoidable overshoot on Firefox's slower WebDriver cadence.
  const victimBrake = await settleLifecycleClient(actor, 20_000);
  let victim = (await actor.metricsState()).uiMetrics || {};
  const victimSpeed = Math.hypot(victim.playerVelocityX, victim.playerVelocityY);
  if (!Number.isFinite(victimSpeed) || victimSpeed > 8 || victim.inSafeZone === true) {
    throw new Error(`chaos lifecycle victim did not settle outside safety: ${JSON.stringify({
      victimBrake,
      x: victim.playerX,
      y: victim.playerY,
      velocityX: victim.playerVelocityX,
      velocityY: victim.playerVelocityY,
      speed: victimSpeed,
      inSafeZone: victim.inSafeZone,
    })}`);
  }

  // The Chaos lane is not an open column either. Two retained failures are the
  // same wall: the Firefox/Chaos route pinned at (7520, 8909), 232.9 px short of
  // (7515, 9142), and the Chrome/Chaos mild abort that held thrust at
  // (8180.6, 8153.4) toward victim + (0, +96) = (8173.05, 7408.63) for 410
  // pulses at 410 px/s of commanded acceleration, best distance 701.4 px, with
  // velocity (-1.5, 6.5) px/s and selfCorrectionCount 0 - thrust against
  // blocking terrain, not a controller or authority fault. Loading
  // bdhadar.lvl proves it: `movingHullPathIsClear` is false for exactly that
  // straight column, while the harness's existing counterfire planner clears the
  // same pin through (8488, 8152) to a hull-clear firing position 180 px off the
  // victim in two waypoints. So route it like Extreme instead of guessing
  // perpendicular offsets; aimLifecycleAttackerAtVictim still owns the final
  // <=160 px lane.
  const routeEvidence = [];
  let staged = false;
  let stalledStages = 0;
  for (let stage = 0; stage < 40; stage += 1) {
    victim = (await actor.metricsState()).uiMetrics || {};
    const from = (await attacker.metricsState()).uiMetrics || {};
    const waypoint = plannedHop(from, victim);
    if (!waypoint) {
      throw new Error(`no hull-clear chaos lifecycle route: ${JSON.stringify({
        from: { x: from.playerX, y: from.playerY },
        victim: { x: victim.playerX, y: victim.playerY },
        routeEvidence,
      })}`);
    }
    const separation = Math.hypot(
      waypoint.x - (from.playerX ?? 0),
      waypoint.y - (from.playerY ?? 0),
    );
    const route = await driveClientToWorldTarget(
      attacker,
      waypoint,
      lifecycleRouteBudgetMs(separation, 42),
      // A routed corner can be closer than24px: adf629d stopped18px short of
      // the same bend twice with zero pulses. Settle tightly at routed bends,
      // as the shared rendezvous driver does, before planning the next leg.
      // The final firing-envelope waypoint keeps its former24px tolerance;
      // the later <=160px aim, death/respawn and progress checks are unchanged.
      waypoint.routed,
      waypoint.routed ? 4 : 24,
      null,
      42,
      0.25,
      15_000,
    );
    routeEvidence.push({
      from: { x: from.playerX, y: from.playerY },
      victim: { x: victim.playerX, y: victim.playerY },
      waypoint,
      route: {
        reached: route.reached,
        reason: route.reason,
        distance: Number(route.distance?.toFixed?.(1) ?? route.distance),
        bestDistance: Number(route.bestDistance?.toFixed?.(1) ?? route.bestDistance),
        pulses: route.pulses,
        // Second site of the same defect I fixed in flyChaosRouteLeg: this summary is assembled field by
        // field, so the driver's whole-attempt shape was computed, attached to the timeout result, and
        // dropped here on the way into the error. A route timeout therefore still looked uninstrumented
        // even though the instrument had fired. Forward any attempt* key the driver publishes.
        ...Object.entries(route ?? {}).filter(([key]) => key.startsWith("attempt"))
          .reduce((carry, [key, value]) => ({ ...carry, [key]: value }), {}),
        thrustWindowMs: route.thrustWindowMs,
      },
    });
    if (!routedHopAccepted(route)) {
      throw new Error(`chaos lifecycle attacker could not reach its firing lane: ${JSON.stringify(routeEvidence)}`);
    }
    await settleLifecycleClient(attacker, 20_000);
    const [after, victimAfter] = await Promise.all([
      attacker.metricsState().then((state) => state.uiMetrics || {}),
      actor.metricsState().then((state) => state.uiMetrics || {}),
    ]);
    const victimDistancePx = Math.hypot(
      victimAfter.playerX - after.playerX,
      victimAfter.playerY - after.playerY,
    );
    // Stage success is measured against the victim, not against point arrival: a
    // routed waypoint inside the driver's stop precision is a corridor micro-hop,
    // and the direct terminal waypoint is a firing position within 180 px of the
    // victim *as sampled when it was planned*. The Chaos victim is settled but not
    // frozen, so both ships are sampled together here - the stage starts by reading
    // the victim, and a hop budget plus a 20 s settle separate that sample from the
    // attacker's arrival. The retained Chrome/Chaos chords lane broke out at a
    // measured 225.70 px that way (`mild-verify-9.sh.log:702`), with the victim
    // drifting at 3.17 px/s: the planner had run out of routed hops, which is not
    // evidence that the envelope asserted below was met.
    //
    // The threshold is the mechanism floor, not a tuned number: the planner's
    // nearest hull-clear candidate is at most 180 px off the victim and
    // `routedHopAccepted` admits a stop 32 px short of the point, so no stage can
    // measure better than 212 px. Setting the break at 200 px instead livelocked the
    // retained Chrome/Chaos mild lane at a best of 202.495 px
    // (`$QA/mild-verify-10.sh.log:32496`) - a condition the loop could never reach
    // is indistinguishable from a stall.
    //
    // The stall test itself is movement: two stages that do not move the ship.
    if (!waypoint.routed && victimDistancePx <= 212) { staged = true; break; }
    if (stageAdvanced({ from, after })) {
      stalledStages = 0;
    } else if ((stalledStages += 1) >= 2) {
      throw new Error(`chaos lifecycle route livelock: ${JSON.stringify({
        from: { x: from.playerX, y: from.playerY },
        after: { x: after.playerX, y: after.playerY },
        victimDistancePx,
        routeEvidence,
      })}`);
    }
  }
  if (!staged) {
    throw new Error(`chaos lifecycle route exceeded bounded waypoint count: ${JSON.stringify(routeEvidence)}`);
  }
  victim = (await actor.metricsState()).uiMetrics || {};
  const attackerMetrics = (await attacker.metricsState()).uiMetrics || {};
  const attackerSpeed = Math.hypot(
    attackerMetrics.playerVelocityX,
    attackerMetrics.playerVelocityY,
  );
  const firingDistance = Math.hypot(
    victim.playerX - attackerMetrics.playerX,
    victim.playerY - attackerMetrics.playerY,
  );
  if (
    attackerMetrics.inSafeZone === true ||
    !Number.isFinite(attackerSpeed) ||
    attackerSpeed > 8 ||
    !Number.isFinite(firingDistance) ||
    // Envelope check, not the firing contract: the staging loop exits on a
    // simultaneous 212 px measurement, and this re-sample adds the victim's drift
    // since that measurement, so 220 px is the admitted bound.
    // aimLifecycleAttackerAtVictim re-samples both ships and owns the final
    // <=160 px lane, exactly as in the Extreme branch, which never asserted 160 px.
    firingDistance > 220
  ) {
    throw new Error(`chaos lifecycle relative firing lane is invalid: ${JSON.stringify({
      victim: {
        x: victim.playerX,
        y: victim.playerY,
        velocityX: victim.playerVelocityX,
        velocityY: victim.playerVelocityY,
      },
      attacker: {
        x: attackerMetrics.playerX,
        y: attackerMetrics.playerY,
        velocityX: attackerMetrics.playerVelocityX,
        velocityY: attackerMetrics.playerVelocityY,
        speed: attackerSpeed,
        inSafeZone: attackerMetrics.inSafeZone,
      },
      firingDistance,
    })}`);
  }
}

async function aimLifecycleAttackerAtVictim(attacker, actor, directionBiasBins = 0) {
  const attempts = [];
  let stalledAttempts = 0;
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const [attackerMetrics, victimMetrics] = await Promise.all([
      attacker.metricsState().then((state) => state.uiMetrics || {}),
      actor.metricsState().then((state) => state.uiMetrics || {}),
    ]);
    const dx = victimMetrics.playerX - attackerMetrics.playerX;
    const dy = victimMetrics.playerY - attackerMetrics.playerY;
    const distance = Math.hypot(dx, dy);
    if (!Number.isFinite(distance)) {
      throw new Error(`keyboard lifecycle firing lane is invalid: ${JSON.stringify({
        attackerX: attackerMetrics.playerX,
        attackerY: attackerMetrics.playerY,
        victimX: victimMetrics.playerX,
        victimY: victimMetrics.playerY,
        distance,
      })}`);
    }
    // Firefox can spend several seconds draining its final lifecycle/bootstrap
    // work after the two peers first establish the lane. Even a correctly
    // settled victim can drift beyond bullet range during that interval. Move
    // the attacker back to a freshly sampled relative point with genuine
    // WebDriver keys; never widen the firing-distance assertion or synthesize
    // a gameplay packet.
    if (distance > 160) {
      // The lane refresh used to drive straight to a fixed `victim + (0, +96)`
      // column, the same absolute offset the staging loops retired. The retained
      // Firefox/Chaos mild lane held 156 thrust pulses against blocking terrain
      // 716.5 px short of it (`$QA/mild-verify-7.sh.log:32418`): the heading, not
      // the lane, crossed the wall. Routing that column instead then failed twice
      // more - the planner found no candidate around it at all at 197.17 px of
      // separation, and at 196.76 px it returned a 19.03 px micro-hop the driver
      // refuses to drive. The column was never the requirement; being inside
      // 160 px of the victim with a clear line to it is. Close the gap on the
      // authored firing ring first, and only fall back to the multi-tile planner
      // when no bearing on that ring is drivable.
      const victimPoint = {
        positionX: victimMetrics.playerX,
        positionY: victimMetrics.playerY,
      };
      const ring = authoredFiringRingWaypoint({
        metrics: attackerMetrics,
        victim: victimPoint,
        // The loop's own exit is 160 px and `routedHopAccepted` admits a stop 40 px
        // short of the point, so a ring at the default 120 px can plateau exactly on
        // the condition this loop has to clear - the same trap that had put the
        // staging break below its mechanism floor. One radius down leaves margin.
        radius: 110,
        ...authoredRouteGeometry,
      });
      const planned = ring ?? routedMapCounterfireWaypoint({
        metrics: attackerMetrics,
        target: victimPoint,
        ...authoredRouteGeometry,
      });
      if (!planned) {
        throw new Error(`no hull-clear lifecycle aim route: ${JSON.stringify({
          attacker: { x: attackerMetrics.playerX, y: attackerMetrics.playerY },
          victim: victimPoint,
          distance,
          attempts,
        })}`);
      }
      const route = await driveClientToWorldTarget(
        attacker,
        planned,
        lifecycleRouteBudgetMs(Math.hypot(
          planned.x - attackerMetrics.playerX,
          planned.y - attackerMetrics.playerY,
        ), 42),
        false,
        // Same idle band as the staging route above; this path re-verifies
        // afterDistance <= 160 after arriving, so 40 px is still well inside the
        // requirement the stage actually asserts.
        40,
        null,
        42,
        0.25,
        15_000,
      );
      attempts.push({ planned, ring: Boolean(ring), reason: route.reason ?? null });
      if (!routedHopAccepted(route, 40)) {
        throw new Error(`keyboard lifecycle attacker could not refresh its firing lane: ${JSON.stringify({
          target: planned,
          ring: Boolean(ring),
          distance,
          route,
          attempts,
        })}`);
      }
      const settled = await settleLifecycleClient(attacker, 15_000);
      if (!settled.settled) {
        throw new Error(`keyboard lifecycle attacker did not settle after refreshing its firing lane: ${JSON.stringify({
          target: planned,
          distance,
          settled,
        })}`);
      }
      // Two refreshes that do not move the ship are a stall; the loop would
      // otherwise spend its remaining attempts re-planning the same blocked lane.
      const afterSettle = await attacker.metricsState()
        .then((state) => state.uiMetrics || {});
      if (stageAdvanced({ from: attackerMetrics, after: afterSettle })) {
        stalledAttempts = 0;
      } else if ((stalledAttempts += 1) >= 2) {
        throw new Error(`keyboard lifecycle aim route livelock: ${JSON.stringify({
          distance,
          from: { x: attackerMetrics.playerX, y: attackerMetrics.playerY },
          after: { x: afterSettle.playerX, y: afterSettle.playerY },
          attempts,
        })}`);
      }
      continue;
    }
    // Match the exact 0..39 direction that the protocol and server admit.
    // A continuous half-step tolerance can alternate forever across the
    // wrap boundary when browser metrics sample direction 39 just before one
    // key frame and direction 0 just after it.
    // A protocol bin centre sits up to 4.5 degrees off the measured bearing, which
    // at the 96-160 px firing standoff is a ~12 px lateral offset, and a victim held
    // under live keys drifts further during the ~0.3 s bullet flight (retained
    // Extreme lane: every one of eighteen bursts spawned from (8260, 6902) on a
    // heading 13 degrees off the 131 px bearing and produced zero damage events).
    // The kill loop asks for an adjacent bin per burst so the stream sweeps.
    const biasedOrientation = (
      Math.atan2(dx, -dy) / (Math.PI * 2) + 1 + directionBiasBins / 40 + 1
    ) % 1;
    const aimed = await orientClientForProtocolDirection(
      attacker,
      Math.sin(biasedOrientation * Math.PI * 2),
      -Math.cos(biasedOrientation * Math.PI * 2),
      3_000,
    );
    const [afterAttacker, afterVictim] = await Promise.all([
      attacker.metricsState().then((state) => state.uiMetrics || {}),
      actor.metricsState().then((state) => state.uiMetrics || {}),
    ]);
    const afterDx = afterVictim.playerX - afterAttacker.playerX;
    const afterDy = afterVictim.playerY - afterAttacker.playerY;
    const afterDistance = Math.hypot(afterDx, afterDy);
    const afterExactOrientation = (
      Math.atan2(afterDx, -afterDy) / (Math.PI * 2) + 1
    ) % 1;
    const afterDesiredDirection = Math.round(afterExactOrientation * 40) % 40;
    // An integer bin shift commutes with the 40-way rounding, so the re-verified
    // fresh bearing is compared against the bin that was actually asked for.
    const afterBiasedDirection = (afterDesiredDirection + directionBiasBins + 40) % 40;
    attempts.push({
      attempt,
      distance,
      afterDistance,
      desiredDirection: aimed.desiredDirection,
      afterDesiredDirection,
      direction: aimed.direction,
      oriented: aimed.oriented,
    });
    if (
      aimed.oriented === true &&
      aimed.direction === afterBiasedDirection &&
      afterDistance <= 160
    ) {
      return {
        distance: afterDistance,
        orientation: aimed.metrics?.playerOrientation,
        desired: afterExactOrientation,
        direction: aimed.direction,
        directionBiasBins,
        attempts,
      };
    }
  }
  throw new Error(`keyboard lifecycle attacker could not aim at the measured browser: ${JSON.stringify(attempts)}`);
}

async function proveKeyboardOwnedDeathAndRespawn({
  actor,
  attacker,
  server,
  playerId,
  initialLifeSeq,
}) {
  await waitUntil("keyboard lifecycle attacker has exact live authority", async () =>
    initialAuthorityReady(await attacker.metricsState()),
  60_000);
  const attackerState = await attacker.metricsState();
  const attackerId = attackerState.uiMetrics?.selfPlayerId;
  if (!Number.isInteger(attackerId) || attackerId === playerId) {
    throw new Error(`keyboard lifecycle attacker identity is invalid: ${JSON.stringify({ playerId, attackerId })}`);
  }
  // Independent stock spawns can occupy different branches of the maze. The
  // legacy counterfire search is deliberately local; first establish the same
  // full-map genuine-key encounter used by the combat lanes, before any held
  // death input or firing assertion begins. Retain its exact-life setup trace.
  const encounterDeadline = Date.now() + 180_000;
  const lifecycleOrigin = await prepareKeyboardLifecycleOrigin({ actor, server, playerId, initialLifeSeq,
    geometry: authoredRouteGeometry, deadline: encounterDeadline });
  const lifecycleRendezvous = await rendezvousPlayers(actor, attacker, server,
    { setupBudgetMs: encounterDeadline - Date.now() });
  const lifecycleRoute = await driveLifecyclePeersOutsideSafety(actor, attacker);
  let firingLane = await aimLifecycleAttackerAtVictim(attacker, actor);
  const victim = (await actor.metricsState()).uiMetrics;
  const shooter = (await attacker.metricsState()).uiMetrics;
  // Fire perpendicular to the attack lane so the held-input witness does not
  // turn this controlled death into reciprocal combat. Opposing turns cancel.
  const facing = await orientClientForProtocolDirection(actor,
    -(shooter.playerY - victim.playerY), shooter.playerX - victim.playerX, 3_000);
  if (!facing.oriented) throw new Error("could not orient held-input victim across the firing lane");
  const heldBeforeDeath = await beginHeldDeath({ actor, server, playerId, lifeSeq: initialLifeSeq });
  const damageOffset = parseServerDamageEvents(server.logs).length;
  let lethal = null;
  // One fixed bin can miss the same way every burst, so alternate adjacent bins
  // and let the stream cover the bearing instead of betting the lane on one
  // geometry (retained Extreme lane: 18 bursts, 30 px perpendicular miss, zero
  // server_damage_applied events in the whole lane).
  const aimSweepBins = [0, 1, -1, 2, -2];
  for (let burst = 0; burst < 18 && lethal == null; burst += 1) {
    firingLane = await aimLifecycleAttackerAtVictim(
      attacker,
      actor,
      aimSweepBins[burst % aimSweepBins.length],
    );
    // Hold through multiple authored bullet cooldowns. Extreme recharges at
    // 300 energy/s and a sequence of isolated one-tap probes can legitimately
    // heal between aim cycles even though every hit is authoritative.
    await attacker.press("Control", 420, false);
    await sleep(80);
    lethal = parseServerDamageEvents(server.logs).slice(damageOffset).find((event) =>
      event.shooterId === attackerId &&
      event.victimId === playerId &&
      event.victimLifeSeq === initialLifeSeq &&
      event.lethal === true
    ) || null;
  }
  if (lethal == null) {
    throw new Error(`genuine keyboard attacker did not lethally retire the measured life: ${JSON.stringify({
      playerId,
      attackerId,
      initialLifeSeq,
      damage: parseServerDamageEvents(server.logs).slice(damageOffset),
    })}`);
  }
  await waitUntil("measured browser applies death and returns under a newer life", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
      Number.isInteger(metrics.playerLifeSeq) && metrics.playerLifeSeq !== initialLifeSeq;
  }, 15_000);
  const nextLifeSeq = (await actor.metricsState()).uiMetrics?.playerLifeSeq;
  const heldAcrossDeath = await finishHeldDeath({ actor, server, playerId,
    lifeSeq: nextLifeSeq, before: heldBeforeDeath });
  return {
    attackerId,
    victimId: playerId,
    victimLifeSeq: initialLifeSeq,
    nextLifeSeq,
    heldAcrossDeath,
    lifecycleOrigin, lifecycleRendezvous,
    lifecycleRoute: lifecycleRoute ?? null,
    damageSeq: lethal.damageSeq,
    weapon: lethal.weapon,
    energyBefore: lethal.energyBefore,
    energyAfter: lethal.energyAfter,
    respawnAtTick: lethal.respawnAtTick,
    firingLane,
  };
}

async function proveNetworkProfileDrain(readState, label) {
  if (!networkProfile) return null;
  let evidence = null;
  await waitUntil(`${label} seeded network timers drain`, async () => {
    evidence = await readState();
    return evidence?.measurementEnded === true && evidence?.s2c?.pending === 0 && evidence?.c2s?.pending === 0 &&
      evidence?.s2c?.drainPending === 0 && evidence?.c2s?.drainPending === 0;
  }, 10_000);
  return assertNetworkProfileEvidence(evidence, {
    expectedName: networkProfile, expectedSeed: networkProfileSeed, label,
  });
}

// Preserve the renderer evidence the shared browser adapters collect from
// before navigation. Counters retain rejected draws even past the event cap.
function retainKeyboardConsoles(directory, actor, attacker, retiredAttackerConsole = null) {
  return [["browser", actor], ["lifecycle-attacker", attacker]].map(([label, client]) => {
    const evidence = label === "lifecycle-attacker" && !client && retiredAttackerConsole
      ? retiredAttackerConsole : browserConsoleEvidence(client);
    fs.writeFileSync(path.join(directory, `${label}-console.json`),
      `${JSON.stringify(evidence, null, 2)}\n`);
    const { events: _events, ...audit } = evidence;
    return { label, ...audit };
  });
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const artifactDirectory = createAtomicArtifactDirectory(target);
  const staticServer = await startStaticServer();
  const serverPort = await freePort();
  const server = startRealServer(serverPort, { zone: arena, observerNetworkProfile: serverObserverProfile });
  let chrome = null;
  let chromeClient = null;
  let firefox = null;
  let attackerChrome = null;
  let attackerClient = null;
  let state = null;
  let actor = null;
  let attackerArtifactState = null;
  let retiredAttackerConsole = null;
  let attackerNetworkProfileEvidence = null;
  let inputLatencyEvidence = null;
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const networkQuery = networkProfile
      ? `&network_profile=${encodeURIComponent(networkProfile)}&network_profile_seed=${networkProfileSeed}` : "";
    const url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1&name=chords${browser}${networkQuery}`;
    if (browser === "chrome") {
      chrome = startChrome(await freePort(), `keyboard-chords-${arena}`, { graphicsBackend: chromeGraphicsBackend });
      await waitForChrome(chrome);
      chromeClient = await openChromeClient(chrome, url);
      actor = chromeClient;
    } else {
      firefox = startGeckodriver(await freePort(), `keyboard-chords-${arena}-firefox`);
      await waitForGeckodriver(firefox);
      await createSession(firefox);
      await openClient(firefox, url);
      actor = {
        metricsState: () => firefoxMetricsState(firefox),
        ...firefoxArtifactClient(firefox),
        keyState: () => firefoxKeyboardState(firefox),
        heldWindowState: (startedAt, endedAt) =>
          firefoxHeldWindowState(firefox, startedAt, endedAt),
        keyDown: (key) => firefoxKeyDown(firefox, key),
        keyDownRepeat: (key) => firefoxKeyDown(firefox, key),
        keyUp: (key) => firefoxKeyUp(firefox, key),
        keyBatch: changes => firefoxKeyBatch(firefox, changes),
        press: (key, durationMs) => holdFirefoxKeys(firefox, [key], durationMs),
        hold: (key, durationMs) => holdFirefoxKeys(firefox, [key], durationMs),
        releaseKeys: () => releaseFirefoxKeys(firefox),
        cycleFocus: (holdMs) => cycleFirefoxWindowFocus(firefox, holdMs),
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
    }
    actor.captureInput = () => browser === "chrome" ? chromeClient.evaluate(BATCH_CAPTURE)
      : execute(firefox, `return ${BATCH_CAPTURE};`);
    await waitUntil(`${browser} ${arena} initial authority`, async () => {
      state = await actor.metricsState();
      return initialAuthorityReady(state);
    }, 60_000);
    const playerId = state.uiMetrics.selfPlayerId;
    const initialLifeSeq = state.uiMetrics.playerLifeSeq;
    const beforeSafeZoneStatus = await fetchStatus(serverPort);
    // Extreme spawns in authored safety. Keep this separate from the ordinary
    // firing matrix, which deliberately navigates out before admitting shots.
    const safeZoneEvidence = arena === "extreme" ? await proveSafeZoneFireThrust({
      actor, server, playerId,
      isAuthoredSafe: (x, y) => extremeSafeTiles.some(tile =>
        Math.floor(x / 16) === tile.tileX && Math.floor(y / 16) === tile.tileY),
    }) : null;
    if (safeZoneEvidence) {
      const afterSafeZoneStatus = await fetchStatus(serverPort);
      if (beforeSafeZoneStatus.movement_snapbacks_total !== 0 ||
          afterSafeZoneStatus.movement_snapbacks_total !== 0) {
        throw new Error("safe-zone fire/thrust generated an honest-session movement correction");
      }
      safeZoneEvidence.correctionsBefore = beforeSafeZoneStatus.movement_snapbacks_total;
      safeZoneEvidence.correctionsAfter = afterSafeZoneStatus.movement_snapbacks_total;
    }
    inputLatencyEvidence = await proveNextDrawPrediction(actor, () => browser === "chrome"
      ? chromeClient.evaluate(INPUT_LATENCY_CAPTURE)
      : execute(firefox, `return ${INPUT_LATENCY_CAPTURE};`),
      { highRefresh: process.env.ZUBSPACE_E2E_HIGH_REFRESH === "1" });
    const evidence = await proveKeyboardChordMatrix({
      actor,
      server,
      playerId,
      phase: "before-first-death",
      prepareAttempt: arena === "extreme"
        ? (options) => prepareExtremeChordAttempt(actor, options)
        : null,
    });
    if (evidence.some((entry) => entry.lifeSeq !== initialLifeSeq)) {
      throw new Error(`bot-free chord matrix crossed a life boundary: ${JSON.stringify(evidence)}`);
    }
    attackerChrome = startChrome(await freePort(), `keyboard-lifecycle-attacker-${browser}-${arena}`, { highRefresh: false });
    await waitForChrome(attackerChrome);
    const attackerUrl = new URL(url);
    attackerUrl.searchParams.set("name", `attk${browser}`);
    attackerClient = await openChromeClient(
      attackerChrome,
      attackerUrl.toString(),
    );
    const deathEvidence = await proveKeyboardOwnedDeathAndRespawn({
      actor,
      attacker: attackerClient,
      server,
      playerId,
      initialLifeSeq,
    });
    await attackerClient.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest?.(); true");
    attackerNetworkProfileEvidence = await proveNetworkProfileDrain(
      () => attackerClient.evaluate("window.__zubspaceNetworkProfileStateForTest?.() || null"),
      "keyboard lifecycle attacker",
    );
    await attackerClient.evaluate("window.__zubspaceSendDisconnectForTest?.(); true");
    attackerArtifactState = await captureBrowserArtifactState(attackerClient);
    assertCompleteQaTraceCaptures([attackerArtifactState.qaTraceCapture]);
    // Flush preceding CDP console events after the cold capture, then preserve
    // the bounded real recorder before releasing this browser and client.
    await attackerClient.evaluate("true");
    retiredAttackerConsole = browserConsoleEvidence(attackerClient);
    assertRendererConsoleClean(retiredAttackerConsole);
    await attackerClient.close();
    attackerClient = null;
    await stopProcess(attackerChrome.child);
    if (attackerChrome.userDataDir) {
      fs.rmSync(attackerChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    }
    attackerChrome = null;
    // Extreme's canonical respawn is inside the central authored safe region, where
    // sustained thrust is damped and the enclosure geometry bounds travel. The chord
    // matrix immediately after this one already restages through the east midline
    // corridor to the authored nonsafe waypoint; the Afterburner matrix needs the
    // same precondition or it measures the enclosure wall instead of the boost, and
    // it cannot observe an energy cost at all because a fresh respawn sits at max
    // energy (retained Extreme lane: 28 px of travel over a 1.08 s boost against the
    // safe frame wall, energy pinned at 1650/1650).
    // One prepare per attempt is not enough. The retained Firefox/Extreme stall
    // measured the *second* Afterburner case, which had no staging of its own:
    // the first case's boost carried the pilot west into the central green, and
    // the sample landed on authored safe tile 171 at (8051.98, 8367.07) at
    // 3.68 px/s after a 1.08 s thrust+Afterburner hold. The matrix now restages
    // before every case.
    const afterburnerEvidence = await proveAfterburnerMatrixWithLifecycleRetries({
      actor,
      server,
      playerId,
      phase: "after-respawn",
      arena,
      prepareAttempt: arena === "extreme"
        ? (options) => prepareExtremeChordAttempt(actor, options)
        : arena === "chaos-zone"
          ? (options) => prepareChaosChordAttempt(actor, { ...options, server })
          : null,
    });
    const postRespawnEvidence = await proveKeyboardChordMatrix({
      actor,
      server,
      playerId,
      phase: "after-respawn",
      prepareAttempt: arena === "extreme"
        ? (options) => prepareExtremeChordAttempt(actor, options)
        : null,
    });
    const heldRepeatPauseEvidence = await proveHeldChordSurvivesRepeatPause({
      actor,
      server,
      playerId,
      phase: "after-respawn",
    });
    // The held-repeat proof includes sustained forward thrust and can carry an
    // Extreme pilot back into the authored central safe region. Firing there
    // is correctly suppressed, so restore the same genuine-key nonsafe
    // precondition used by the chord matrix before proving focus-loss release.
    if (arena === "extreme") {
      await prepareExtremeChordAttempt(actor);
    }
    const focusLossEvidence = await proveFocusLossReleasesHeldChord({
      actor,
      server,
      playerId,
      phase: "after-respawn",
    });
    const heldBatchEvidence = await proveHeldKeyBatches({ actor, server, playerId,
      capture: () => browser === "chrome" ? chromeClient.evaluate(BATCH_CAPTURE)
        : execute(firefox, `return ${BATCH_CAPTURE};`),
    });
    // Chaos owns its own safe-zone proof: navigation into the authored green plus the held
    // sets that only exist under suppression. Run last so the ordinary firing matrix keeps
    // the nonsafe staging it depends on.
    let chaosSafeZoneEvidence = null;
    if (chaosSafeMapBytes) {
      const orientPilot = (velocityX, velocityY) =>
        orientClientForProtocolDirection(actor, velocityX, velocityY);
      const stage = await stageChaosSafeZonePilot(actor);
      const beforeChaosSafeZone = await fetchStatus(serverPort);
      chaosSafeZoneEvidence = await proveChaosSafeZoneHeldSets({
        actor, server, playerId, isAuthoredSafe: chaosAuthoredSafe,
        orient: orientPilot,
        thrustHeading: stage.escapeHeading, safeHeading: stage.safeRun,
      });
      const afterChaosSafeZone = await fetchStatus(serverPort);
      const corrections = afterChaosSafeZone.movement_snapbacks_total -
        beforeChaosSafeZone.movement_snapbacks_total;
      if (corrections !== 0) {
        throw new Error("Chaos safe-zone held sets generated a movement correction: " +
          JSON.stringify({ corrections, before: beforeChaosSafeZone.movement_snapbacks_total,
            after: afterChaosSafeZone.movement_snapbacks_total }));
      }
      chaosSafeZoneEvidence.stage = stage;
      chaosSafeZoneEvidence.correctionsDuring = corrections;
    }
    state = await captureBrowserArtifactState(actor);
    assertCompleteQaTraceCaptures([state.qaTraceCapture]);
    if (browser === "chrome") {
      await chromeClient.evaluate("window.__zubspaceEndNetworkProfileMeasurementForTest?.(); true");
    } else {
      await execute(firefox, "window.__zubspaceEndNetworkProfileMeasurementForTest?.(); return true;");
    }
    await proveNetworkProfileDrain(
      () => browser === "chrome"
        ? chromeClient.evaluate("window.__zubspaceNetworkProfileStateForTest?.() || null")
        : execute(firefox, "return window.__zubspaceNetworkProfileStateForTest?.() || null;"),
      "keyboard pilot before disconnect",
    );
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [browser === "chrome" ? chrome.logs : firefox.logs],
      disconnect: async () => {
        if (browser === "chrome") {
          await chromeClient.evaluate("window.__zubspaceSendDisconnectForTest?.(); true");
        } else {
          await execute(firefox, "window.__zubspaceSendDisconnectForTest?.(); return true;");
        }
      },
    });
    if (browser === "chrome") {
      fs.writeFileSync(path.join(artifactDirectory.temporary, "browser.png"), Buffer.from(await chromeClient.screenshot(), "base64"));
    } else {
      fs.writeFileSync(path.join(artifactDirectory.temporary, "browser.png"), Buffer.from(await screenshot(firefox), "base64"));
    }
    await sleep(750);
    const cleanupStatus = await waitForCleanup(serverPort);
    const networkProfileEvidence = await proveNetworkProfileDrain(
      () => browser === "chrome"
        ? chromeClient.evaluate("window.__zubspaceNetworkProfileStateForTest?.() || null")
        : execute(firefox, "return window.__zubspaceNetworkProfileStateForTest?.() || null;"),
      "keyboard pilot",
    );
    const serverObserverProfileEvidence = assertServerObserverProfile(cleanupStatus, serverObserverProfile);
    const securityAudit = assertSecurityRejectAudit(server);
    const provenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(initialProvenance, provenance);
    fs.writeFileSync(path.join(artifactDirectory.temporary, "server.log"), server.logs.join(""));
    fs.writeFileSync(path.join(artifactDirectory.temporary, "browser-state.json"), `${JSON.stringify(state, null, 2)}\n`);
    const qaTraceCaptureAudit = [
      writeQaTraceArtifact({ directory: artifactDirectory.temporary, label: "browser", state,
        serverLogs: server.logs, provenance }),
      writeQaTraceArtifact({ directory: artifactDirectory.temporary, label: "lifecycle-attacker",
        state: attackerArtifactState, serverLogs: server.logs, provenance }),
    ];
    const browserConsoleAudits = retainKeyboardConsoles(artifactDirectory.temporary,
      actor, attackerClient, retiredAttackerConsole);
    for (const audit of browserConsoleAudits) assertRendererConsoleClean(audit);
    const result = {
      ok: true,
      browserConsoleAudits,
      browser,
      arena,
      graphicsRig: await keyboardGraphicsEvidence(chromeClient, firefox),
      playerId,
      initialLifeSeq,
      afterburnerEvidence,
      safeZoneEvidence,
      chaosSafeZoneEvidence,
      inputLatencyEvidence,
      highRefreshRequest: process.env.ZUBSPACE_E2E_HIGH_REFRESH === "1"
        ? { scheduler: browser === "chrome" ? "disable-frame-rate-limit" : "layout.frame_rate=0", minimumMeasuredHz: 90,
          ...(browser === "firefox" ? { reduceTimerPrecision: false } : {}) }
        : null,
      heldBatchEvidence,
      keyboardFeedbackReader: keyboardTraceReaderStats(server, "human authority input trace"),
      evidence,
      heldRepeatPauseEvidence,
      focusLossEvidence,
      deathEvidence,
      postRespawnEvidence,
      final: compactMetrics(state.uiMetrics),
      securityAudit,
      deliveryAudit,
      cleanupStatus,
      networkProfileEvidence,
      attackerNetworkProfileEvidence,
      serverObserverProfileEvidence,
      provenance,
      qaTraceCaptureAudit,
      artifactFiles: [],
    };
    const resultPath = path.join(artifactDirectory.temporary, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    result.artifactFiles = artifactFileManifest(artifactDirectory.temporary, { exclude: ["result.json"] });
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    artifactDirectory.publish();
    console.log(JSON.stringify({ ok: true, browser, arena, evidence }, null, 2));
  } catch (error) {
    artifactDirectory.cleanup();
    const failure = `${target}-failure`;
    fs.mkdirSync(failure, { recursive: false });
    const failureState = await captureBrowserArtifactState(actor);
    try {
      fs.writeFileSync(path.join(failure, "graphics-rig.json"),
        `${JSON.stringify(await keyboardGraphicsEvidence(chromeClient, firefox), null, 2)}\n`);
    } catch (captureError) {
      fs.writeFileSync(path.join(failure, "graphics-rig-error.txt"), `${captureError.message}\n`);
    }
    try {
      if (chromeClient) {
        fs.writeFileSync(path.join(failure, "browser.png"), Buffer.from(await chromeClient.screenshot(), "base64"));
      } else if (firefox?.sessionId) {
        fs.writeFileSync(path.join(failure, "browser.png"), Buffer.from(await screenshot(firefox), "base64"));
      }
    } catch (captureError) {
      fs.writeFileSync(path.join(failure, "browser-screenshot-error.txt"), `${captureError.message}\n`);
    }
    fs.writeFileSync(path.join(failure, "server.log"), server.logs.join(""));
    fs.writeFileSync(path.join(failure, "browser-state.json"), `${JSON.stringify(failureState, null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "error.txt"), `${error.stack || error.message}\n`);
    fs.writeFileSync(path.join(failure, "rendezvous-trace.json"),
      `${JSON.stringify(actor?.rendezvousTrace || attackerClient?.rendezvousTrace || [], null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "completed-input-latency.json"), `${JSON.stringify(inputLatencyEvidence, null, 2)}\n`);
    retainKeyboardConsoles(failure, actor, attackerClient, retiredAttackerConsole);
    writeQaTraceFailureResult({ directory: failure, provenance: initialProvenance, captures: [
      { label: "browser", state: failureState, serverLogs: server.logs },
      { label: "lifecycle-attacker", state: attackerClient
        ? await captureBrowserArtifactState(attackerClient) : attackerArtifactState, serverLogs: server.logs },
    ] });
    error.stack = `${error.stack || String(error)}\nretained keyboard failure: ${failure}`;
    throw error;
  } finally {
    await chromeClient?.close().catch(() => {});
    await attackerClient?.close().catch(() => {});
    await deleteSession(firefox).catch(() => {});
    await Promise.all([
      stopProcess(chrome?.child),
      stopProcess(attackerChrome?.child),
      stopProcess(firefox?.child),
      stopProcess(server.child),
    ]).catch(() => {});
    if (chrome?.userDataDir) fs.rmSync(chrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    if (attackerChrome?.userDataDir) fs.rmSync(attackerChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exit(1);
  });
}

export { stageChaosSafeZonePilot };
