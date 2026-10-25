import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import fs from "node:fs";
import { authoredMapGeometry } from "./authored_map_geometry.mjs";
import { movingBombAttackPlan } from "./moving_bomb_pilot.mjs";

test("live bot scenarios do not export full browser state for metric assertions", () => {
  for (const file of ["bot_authority.mjs", "bot_authority_firefox.mjs"]) {
    const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
    // The adapter's pageState closure remains available for cold artifacts.
    // Awaiting it in the live scenario recreated a 263 ms Firefox pause merely
    // to read scoreboard fields already present in the compact metrics reader.
    assert.doesNotMatch(source, /await\s+(?:actor\.)?pageState\s*\(/, file);
  }
});

import {
  arrivalControlDistance,
  driveClientToWorldTarget,
  authoredDoorClosedForCurrentAndNext,
  brakeOrientationDisposition,
  brakeVelocityDisposition,
  dataChannelDrainDisposition,
  logicalInputLifecyclePaused,
} from "./combat_replay.mjs";

test("stopped route pilot outside arrival radius must not idle in acceleration dead zone", async () => {
  const target = { x: 8040, y: 8360 };
  const metrics = { playerX: 8045.484375, playerY: 8354.467773,
    playerVelocityX: 0, playerVelocityY: 0,
    playerOrientation: ((Math.atan2(-5.484375, -5.532227) / (2 * Math.PI)) + 1) % 1 };
  const inputs = [];
  const client = {
    metricsState: async () => ({ uiMetrics: { ...metrics } }),
    press: async key => {
      inputs.push(key);
      assert.equal(key, "w", "the aligned stopped pilot needs a fine thrust pulse");
      metrics.playerX = 8045;
      metrics.playerY = 8355;
    },
  };
  const result = await driveClientToWorldTarget(client, target, 200, true, 7.75, null, 100);
  assert.equal(result.reached, true);
  assert.deepEqual(inputs, ["w"]);
  assert(result.distance <= 7.75, "do not expand the original arrival radius");
});

import {
  assertHumanProjectilePresentationGeometry,
  bombWireAimDisposition,
  botFiringHeadingReady,
  chaosBombStationaryLaneDisposition,
  compactBotPresentationState,
  captureBotPresentationState,
  counterfireRouteNeedsBrake,
  continuumPositionVelocity,
  explosiveContactOverlapsHull,
  exactLifeDamagePresentationEvidence,
  exactRemoteDeathExplosionWitness,
  exactLifePrimaryBombTerminal,
  renewRetiredBombTarget,
  longestTimedTraceSpan,
  metricsTraceWithViewport,
  movingHullPathIsClear,
  ownedProjectileTailSettled,
  postRespawnDamageEvent,
  presentedLiveBotSuccessor,
  presentedBotCombatCycleEvidence,
  presentedBotRepelEvidence,
  protectedAimExitPathIsClear,
  repelPresentationCorridorIsOpen,
  projectileInterceptVector,
  quantizedProjectileIntercept,
  projectileRadialVelocity,
  remoteActorMotionAcrossStatesEvidence,
  botDoorRelocationEvidence,
  remoteActorMotionSmoothnessEvidence,
  resolvePresentedRespawnChain,
  routedMapCounterfireWaypoint,
  serverDamageBoundary,
  serverDamageEventsSince,
  snapshotFreeDamageCommitEvidence,
  stationaryBotMotionEvidence,
  stationaryFixtureLifeSeqs,
  requiredStationaryFixtureLifeSeqs,
  stationaryChaosBombHoldingPoint,
  sustainedExactLifeAttackEvidence,
  timedMovementWindows,
  timedTraceContinuity,
  worldPointOnCenteredViewport,
} from "./bot_combat_evidence.mjs";

test("presentation timing is opt-in without an extra timing RPC on ordinary capture", async () => {
  let reads=0;
  const context=vm.createContext({window:{__zubspaceMetricsTrace:[{at:123}]},performance,Date,structuredClone});
  const result=await captureBotPresentationState({evaluate:async expression=>{
    reads++; return vm.runInContext(expression,context);
  }},[768],0,false,{measure:false});
  assert.equal(result.metricsTrace[0].at,123); assert.equal(reads,3);
  assert.equal(result.diagnosticReadTiming,undefined);
});

test("text transfer retains every projected assertion row and exact finite number", async () => {
  const raw={uiMetrics:{playerLifeSeq:7,playerX:1173.5999999195337},
    metricsTrace:[{at:100,remotePlayers:[{id:768,lifeSeq:9,displayX:12.75}]}],
    remoteSpriteTrace:[{at:100,presentationRenderTick:71,entries:[{id:768,lifeSeq:9,drawn:true}]}]};
  const context={performance,Date,structuredClone,window:{__zubspaceUiMetrics:raw.uiMetrics,
    __zubspaceMetricsTrace:raw.metricsTrace,__zubspaceRemoteSpriteTrace:raw.remoteSpriteTrace}};
  const result=await captureBotPresentationState({evaluate:async expression=>vm.runInNewContext(expression,context)},[768],0,false,{measure:false});
  assert.deepEqual(result,JSON.parse(JSON.stringify(compactBotPresentationState(raw,[768],0,false))));
  assert.equal(result.uiMetrics.playerX,raw.uiMetrics.playerX);
});

test("diagnostic presentation read retains rows and separates freeze from read window", async () => {
  let reads=0;
  const times=[100,102,180];
  const context=vm.createContext({Date,structuredClone,performance:{now:()=>times.shift()},
    window:{__zubspaceMetricsTrace:[{at:123}],__zubspaceRemoteSpriteTrace:[{at:123}]}});
  const result=await captureBotPresentationState({evaluate:async expression=>{
    reads++;
    return vm.runInContext(expression,context);
  }},[768],0,false,{measure:true});
  assert.equal(reads,4); assert.equal(result.metricsTrace[0].at,123);
  assert.equal(result.remoteSpriteTrace[0].at,123);
  assert.equal(result.diagnosticReadTiming.freezeMs,2);
  assert.equal(result.diagnosticReadTiming.readWindowMs,80);
});

test("counterfire route time slices preserve momentum until arrival or failed progress", () => {
  assert.equal(counterfireRouteNeedsBrake({ reached: false, reason: "route timeout" }), false);
  assert.equal(counterfireRouteNeedsBrake({ reached: true }), true);
  assert.equal(counterfireRouteNeedsBrake({ reached: false, reason: "no route progress" }), true);
  assert.equal(counterfireRouteNeedsBrake({ reached: false, reason: "non-finite movement metrics" }), true);
  assert.equal(counterfireRouteNeedsBrake(null), true);
});

test("counterfire route slices do not repeatedly discard straight-corridor acceleration", () => {
  // Independent simple plant: each slice starts with a steering observation,
  // then accelerates. Repeated stop commands prevent reaching cruise speed.
  let position = 0;
  let velocity = 0;
  for (let slice = 0; slice < 10; slice += 1) {
    velocity = Math.min(100, velocity + 10);
    position += velocity;
    if (counterfireRouteNeedsBrake({ reached: false, reason: "route timeout" })) velocity = 0;
  }
  assert.equal(position, 550);
  assert.equal(velocity, 100);
});

test("human projectile geometry evidence requires the selected observer render anchor", () => {
  const damage = {
    damageSeq: 9,
    projectileId: 41,
    shooterId: 7,
    shooterLifeSeq: 3,
    victimId: 768,
    victimLifeSeq: 4,
    damage: 571,
  };
  const witness = {
    type: "server_collision_witness",
    projectile_id: 41,
    owner_id: 7,
    owner_life_seq: 3,
    target_id: 768,
    target_life_seq: 4,
    adjusted_damage: 571,
    history_source: "observer_presentation",
    contact_reason: "splash",
    requested_target_tick: 1_000,
    selected_target_tick: 1_000,
    collision_anchor_x: 1_250,
    collision_anchor_y: 2_500,
    presentation_geometry: {
      status: "resolved_interpolated",
      accepted_timeline_generation: 5,
      effective_timeline_generation: 5,
      render_tick: 1_000,
      target_x: 1_250,
      target_y: 2_500,
      authority_error: 7,
    },
  };
  const server = {
    logs: [`zubspace_event ${JSON.stringify(witness)}\n`],
  };
  assert.deepEqual(
    assertHumanProjectilePresentationGeometry(server, [damage], "fixture"),
    [{
      damageSeq: 9,
      projectileId: 41,
      victimId: 768,
      victimLifeSeq: 4,
      historySource: "observer_presentation",
      contactReason: "splash",
      renderTick: 1_000,
      selectedTargetTick: 1_000,
      timelineGeneration: 5,
      authorityError: 7,
    }],
  );

  witness.history_source = "unified_exact_segment";
  assert.throws(
    () => assertHumanProjectilePresentationGeometry(
      { logs: [`zubspace_event ${JSON.stringify(witness)}\n`] },
      [damage],
      "fixture",
    ),
    /verified observer-presentation geometry/,
  );
});

test("presented live bot successor requires exact current actor and wrap-safe newer life", () => {
  const metrics = {
    remotePlayers: [
      { id: 768, lifeSeq: 4, energy: 1_700 },
      { id: 768, lifeSeq: 6, energy: 1_200 },
    ],
    authoritativeSnapshotActors: [{ id: 768, lifeSeq: 6 }],
  };
  assert.deepEqual(
    presentedLiveBotSuccessor(metrics, 768, 5),
    metrics.remotePlayers[1],
  );
  assert.equal(presentedLiveBotSuccessor(metrics, 768, 6), null);
  assert.equal(
    presentedLiveBotSuccessor({
      remotePlayers: [{ id: 768, lifeSeq: 7, energy: 0 }],
      authoritativeSnapshotActors: [{ id: 768, lifeSeq: 7 }],
    }, 768, 6),
    null,
  );
  assert.equal(
    presentedLiveBotSuccessor({
      remotePlayers: [{ id: 768, lifeSeq: 7, energy: 1_700 }],
      authoritativeSnapshotActors: [{ id: 768, lifeSeq: 8 }],
    }, 768, 6),
    null,
  );
  assert.equal(
    presentedLiveBotSuccessor({
      remotePlayers: [{ id: 768, lifeSeq: 1, energy: 1_700 }],
      authoritativeSnapshotActors: [{ id: 768, lifeSeq: 1 }],
    }, 768, 0xffff_ffff)?.lifeSeq,
    1,
  );
});

test("protected aim edge selection rejects a hull-blocked exit corridor", () => {
  const candidate = {
    origin: { x: 100, y: 100 },
    lead: { x: 0, y: 100 },
    positionIsOnSafeTile: (_x, y) => y < 160,
  };
  assert.equal(protectedAimExitPathIsClear({
    ...candidate,
    hullClearsSolid: (_x, y) => y < 148 || y > 156,
  }), false);
  assert.equal(protectedAimExitPathIsClear({
    ...candidate,
    hullClearsSolid: () => true,
  }), true);
  assert.equal(protectedAimExitPathIsClear({
    ...candidate,
    hullClearsSolid: () => true,
    positionIsOnSafeTile: () => true,
  }), false);

  const retainedStationaryAim = quantizedProjectileIntercept({
    // Retained Chrome/Extreme mild failure: the center ray from the protected
    // point to bot life 2 cleared the south edge, but inherited exit velocity
    // rotated the legal 40-bin bomb far enough west for the hull to clip it.
    relativeX: 8_017.79541015625 - 8_024,
    relativeY: 8_463.0615234375 - 8_408,
    targetVelocityX: 0,
    targetVelocityY: 0,
    playerVelocityX: 0,
    playerVelocityY: 170,
    projectileSpeed: 430,
    maxTime: 2,
    playerSpeedAlongDirection: 170,
  });
  assert.equal(retainedStationaryAim.direction, 21);
  const retainedAimVector = {
    x: retainedStationaryAim.projectileVelocityX * retainedStationaryAim.time,
    y: retainedStationaryAim.projectileVelocityY * retainedStationaryAim.time,
  };
  const retainedHull = (x, y) => !(x < 8_018 && y > 8_390);
  assert.equal(protectedAimExitPathIsClear({
    origin: { x: 8_024, y: 8_360 },
    lead: {
      x: 8_017.79541015625 - 8_024,
      y: 8_463.0615234375 - 8_360,
    },
    hullClearsSolid: retainedHull,
    positionIsOnSafeTile: (_x, y) => y < 8_400,
  }), true);
  assert.equal(protectedAimExitPathIsClear({
    origin: { x: 8_024, y: 8_360 },
    lead: retainedAimVector,
    hullClearsSolid: retainedHull,
    positionIsOnSafeTile: (_x, y) => y < 8_400,
  }), false);

  const retainedWestAim = quantizedProjectileIntercept({
    // Retained Chrome/Extreme clean fallback: this exact life-2 target has a
    // clear west-edge path, but the nearest legal bomb bin is 18 degrees from
    // the axis once the expected westbound exit velocity is included.
    relativeX: -102,
    relativeY: 39,
    targetVelocityX: 0,
    targetVelocityY: 0,
    playerVelocityX: -170,
    playerVelocityY: 0,
    projectileSpeed: 430,
    maxTime: 2,
    playerSpeedAlongDirection: 170,
  });
  const retainedWestAlignment =
    -retainedWestAim.projectileVelocityX /
    Math.hypot(
      retainedWestAim.projectileVelocityX,
      retainedWestAim.projectileVelocityY,
    );
  assert.equal(retainedWestAim.direction, 28);
  assert.ok(retainedWestAlignment >= 0.9);
  assert.ok(retainedWestAlignment < 0.97);
});

test("counterfire routing excludes a center-clear lane the target hull cannot aim through", () => {
  const target = { positionX: 1_000, positionY: 1_000 };
  const unrestricted = routedMapCounterfireWaypoint({
    metrics: { playerX: 1_240, playerY: 1_000 },
    target,
    authoredHullIsTraversable: () => true,
    authoredHullIsOpen: () => true,
    authoredLineIsClear: () => true,
  });
  const constrained = routedMapCounterfireWaypoint({
    metrics: { playerX: 1_240, playerY: 1_000 },
    target,
    authoredHullIsTraversable: () => true,
    authoredHullIsOpen: () => true,
    authoredLineIsClear: () => true,
    authoredTargetAimIsClear: (_fromX, _fromY, toX, toY) => toY > target.positionY,
  });
  assert.deepEqual(unrestricted, {
    x: 1_180,
    y: 1_000,
    distance: 60,
    routed: false,
    routeTiles: 0,
  });
  assert.ok(constrained.y > target.positionY);
  assert.notDeepEqual(constrained, unrestricted);
});

function bentCounterfireCorridor() {
  // Configuration-space corridors: the hull fits along the bottom and top
  // passages, joined only at their east end. The target is across the wall.
  const hullIsOpen = (x, y) =>
    (x >= 984 && x <= 1416 && y >= 1080 && y <= 1112) ||
    (x >= 1384 && x <= 1416 && y >= 784 && y <= 1112) ||
    (x >= 984 && x <= 1416 && y >= 784 && y <= 816);
  const segmentIsOpen = (x, y, toX, toY) => {
    const steps = Math.max(1, Math.ceil(Math.hypot(toX - x, toY - y)));
    for (let index = 0; index <= steps; index += 1) {
      if (!hullIsOpen(x + (toX - x) * index / steps,
        y + (toY - y) * index / steps)) return false;
    }
    return true;
  };
  return {
    metrics: { playerX: 1000, playerY: 1096 },
    target: { positionX: 1000, positionY: 800 },
    authoredHullIsTraversable: hullIsOpen,
    authoredHullIsOpen: hullIsOpen,
    authoredLineIsClear: segmentIsOpen,
    segmentIsOpen,
  };
}

test("counterfire routing advances along the clear corridor instead of stopping every four tiles", () => {
  const fixture = bentCounterfireCorridor();
  const waypoint = routedMapCounterfireWaypoint(fixture);
  assert.equal(waypoint.routed, true);
  assert.ok(waypoint.x >= 1384 && waypoint.x <= 1416,
    `approach must reach the first bend, not brake on a clear straight: ${JSON.stringify(waypoint)}`);
  assert.ok(waypoint.y >= 1080);
  assert.equal(fixture.segmentIsOpen(1000, 1096, waypoint.x, waypoint.y), true);
});

test("counterfire routing cannot shortcut a hull-blocked corner on a clear center ray", () => {
  const fixture = bentCounterfireCorridor();
  const waypoint = routedMapCounterfireWaypoint({
    ...fixture,
    // A center ray cannot stand in for the moving pilot's full hull clearance.
    authoredLineIsClear: () => true,
  });
  assert.equal(waypoint.routed, true);
  assert.equal(fixture.segmentIsOpen(1000, 1096, waypoint.x, waypoint.y), true,
    `the real-key controller must receive a traversable segment: ${JSON.stringify(waypoint)}`);
});

test("counterfire routing can leave walkable safety through a bent corridor", () => {
  const fixture = bentCounterfireCorridor();
  const inSafety = (x, y) => x < 1320 && y >= 1080;
  const waypoint = routedMapCounterfireWaypoint({
    ...fixture,
    authoredHullIsTraversable: fixture.authoredHullIsOpen,
    authoredHullIsOpen: (x, y) => fixture.authoredHullIsOpen(x, y) && !inSafety(x, y),
  });
  assert.ok(waypoint, "walkable safety must not trap the real-key pilot");
  assert.equal(waypoint.routed, true);
  assert.ok(waypoint.x >= 1384 && waypoint.x <= 1416);
  assert.equal(fixture.segmentIsOpen(1000, 1096, waypoint.x, waypoint.y), true);
});

test("counterfire routing may cross safety without selecting it as a firing destination", () => {
  const hullIsTraversable = (x, y) => x >= 900 && x <= 1500 && y >= 984 && y <= 1016;
  const inSafety = (x) => x >= 1040 && x <= 1150;
  const waypoint = routedMapCounterfireWaypoint({
    metrics: { playerX: 1000, playerY: 1000 },
    target: { positionX: 1300, positionY: 1000 },
    authoredHullIsTraversable: hullIsTraversable,
    authoredHullIsOpen: (x, y) => hullIsTraversable(x, y) && !inSafety(x),
    authoredLineIsClear: () => true,
  });
  assert.ok(waypoint, "a safe section of a clear corridor is traversable");
  assert.equal(waypoint.routed, false);
  assert.equal(waypoint.x, 1180);
  assert.equal(waypoint.y, 1000);
  assert.equal(inSafety(waypoint.x), false);
});

test("counterfire routing rejects a destination set that permits movement but not firing", () => {
  assert.equal(routedMapCounterfireWaypoint({
    metrics: { playerX: 1000, playerY: 1000 },
    target: { positionX: 1300, positionY: 1000 },
    authoredHullIsTraversable: () => true,
    authoredHullIsOpen: () => false,
    authoredLineIsClear: () => true,
  }), null);
});

test("counterfire routing requires an explicit traversal predicate", () => {
  assert.throws(() => routedMapCounterfireWaypoint({
    metrics: { playerX: 1000, playerY: 1000 },
    target: { positionX: 1300, positionY: 1000 },
    authoredHullIsOpen: () => true,
    authoredLineIsClear: () => true,
  }), /explicit traversal predicate/);
});

for (const captured of [
  { browser: "Chrome", x: 7432, y: 9304, targetX: 7890.05859375, targetY: 8224.2734375 },
  { browser: "Firefox", x: 7320, y: 9320, targetX: 7327, targetY: 7811 },
]) test(`counterfire routing escapes the captured ${captured.browser} Chaos respawn pocket beyond the local search box`, () => {
  // bfd9443 Chrome bot-bomb RED, native bot movement at tick13938.
  // The human stays at its legal random spawn for120s because the only exit
  // bends east past the former target+1024px search boundary.
  const geometry = authoredMapGeometry(fs.readFileSync(
    new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const metrics = { playerX: captured.x, playerY: captured.y };
  const target = { positionX: captured.targetX, positionY: captured.targetY };
  const waypoint = routedMapCounterfireWaypoint({ metrics, target,
    authoredHullIsTraversable: geometry.authoredTerrainHullIsTraversable,
    authoredHullIsOpen: geometry.authoredHullIsOpen,
    authoredLineIsClear: geometry.authoredTerrainLineIsClear });
  assert.ok(waypoint, "a connected map pocket must not be mistaken for an unreachable target");
  assert.equal(waypoint.routed, true);
  assert.ok(Math.hypot(waypoint.x - metrics.playerX, waypoint.y - metrics.playerY) > 24);
  assert.equal(movingHullPathIsClear({ x: metrics.playerX, y: metrics.playerY,
    velocityX: waypoint.x - metrics.playerX, velocityY: waypoint.y - metrics.playerY,
    seconds: 1, hullIsOpen: geometry.authoredTerrainHullIsTraversable }), true,
  "the fallback must keep the entire hull clear instead of flying through the occluding wall");
});

function exactPresentedDamageState(overrides = {}) {
  const event = {
    damageSeq: 17,
    projectileId: 29,
    serverTick: 0x7fff_fff8,
    victimId: 768,
    victimLifeSeq: 4,
    shooterId: 7,
    shooterLifeSeq: 2,
    weapon: 66,
    damage: 400,
    energyBefore: 1_100,
    energyAfter: 700,
    lethal: false,
  };
  const trace = {
    victimId: event.victimId,
    attackerId: event.shooterId,
    damageSeq: event.damageSeq,
    projectileId: event.projectileId,
    serverTick: event.serverTick,
    victimLifeSeq: event.victimLifeSeq,
    shooterLifeSeq: event.shooterLifeSeq,
    weapon: event.weapon,
    damage: event.damage,
    energyBefore: event.energyBefore,
    energyAfter: event.energyAfter,
    explicitLethal: false,
  };
  const witness = {
    at: 1_000,
    damage: {
      damageSeq: event.damageSeq,
      projectileId: event.projectileId,
      serverTick: event.serverTick,
      victimId: event.victimId,
      victimLifeSeq: event.victimLifeSeq,
      attackerId: event.shooterId,
      shooterLifeSeq: event.shooterLifeSeq,
      weapon: event.weapon,
      amount: event.damage,
      energyBefore: event.energyBefore,
      energyAfter: event.energyAfter,
      explicitLethal: false,
    },
    sprite: {
      id: event.victimId,
      lifeSeq: event.victimLifeSeq,
      drawn: true,
      suppression: "none",
      lastServerPositionTick: event.serverTick,
      authoritativeX: 8_200,
      authoritativeY: 8_200,
    },
    sameLifeEnergySnapshot: {
      observedAt: 1_010,
      snapshotId: 44,
      serverTick: 3,
      playerId: event.victimId,
      lifeSeq: event.victimLifeSeq,
      energy: 705,
      serverEnergy: 705,
    },
  };
  return {
    event,
    state: {
      damageTrace: [{ ...trace, ...(overrides.trace || {}) }],
      damagePresentationTrace: [{
        ...witness,
        ...(overrides.witness || {}),
        damage: { ...witness.damage, ...(overrides.damage || {}) },
        sprite: { ...witness.sprite, ...(overrides.sprite || {}) },
        drawnSprite: overrides.drawnSprite,
        drawnSpriteObservedAt: overrides.drawnSpriteObservedAt,
        sameLifeEnergySnapshot: {
          ...witness.sameLifeEnergySnapshot,
          ...(overrides.energySnapshot || {}),
        },
      }],
    },
  };
}

test("exact-life damage presentation binds a later snapshot across tick wrap", () => {
  const { state, event } = exactPresentedDamageState();
  const evidence = exactLifeDamagePresentationEvidence(state, event);
  assert.equal(evidence.energySnapshot.snapshotId, 44);
  assert.equal(evidence.energySnapshot.energy, 705);
});

test("exact-life damage presentation rejects successor life and stale snapshot evidence", () => {
  for (const overrides of [
    { energySnapshot: { lifeSeq: 5 } },
    { energySnapshot: { serverTick: 0x7fff_f000 } },
    { energySnapshot: { energy: 1_100 } },
    { sprite: { lifeSeq: 5 } },
    { damage: { projectileId: 30 } },
  ]) {
    const { state, event } = exactPresentedDamageState(overrides);
    assert.equal(exactLifeDamagePresentationEvidence(state, event), null);
  }
});

test("lethal damage presentation accepts only a recent drawn exact-life predecessor", () => {
  const { state, event } = exactPresentedDamageState({
    trace: { explicitLethal: true, energyAfter: 0 },
    damage: { explicitLethal: true, energyAfter: 0 },
    witness: { at: 2_000 },
    sprite: {
      drawn: false,
      suppression: "no-display-position",
      lastServerPositionTick: 0x7fff_fff8,
    },
    drawnSprite: {
      id: 768,
      lifeSeq: 4,
      drawn: true,
      suppression: "none",
      lastServerPositionTick: 0x7fff_fff4,
      authoritativeX: 8_198,
      authoritativeY: 8_201,
    },
    drawnSpriteObservedAt: 1_950,
  });
  event.energyAfter = 0;
  event.lethal = true;
  const evidence = exactLifeDamagePresentationEvidence(state, event);
  assert.equal(evidence.presentedSprite.drawn, true);
  assert.equal(evidence.presentedAt, 1_950);
});

test("lethal damage presentation rejects stale or successor-life draw evidence", () => {
  for (const overrides of [
    { drawnSpriteObservedAt: 900 },
    { drawnSprite: { lifeSeq: 5 } },
    { drawnSprite: { lastServerPositionTick: 0x7fff_ff00 } },
  ]) {
    const { state, event } = exactPresentedDamageState({
      trace: { explicitLethal: true, energyAfter: 0 },
      damage: { explicitLethal: true, energyAfter: 0 },
      witness: { at: 2_000 },
      sprite: { drawn: false, suppression: "no-display-position" },
      drawnSprite: {
        id: 768,
        lifeSeq: 4,
        drawn: true,
        suppression: "none",
        lastServerPositionTick: 0x7fff_fff4,
        ...overrides.drawnSprite,
      },
      drawnSpriteObservedAt: overrides.drawnSpriteObservedAt ?? 1_950,
    });
    event.energyAfter = 0;
    event.lethal = true;
    assert.equal(exactLifeDamagePresentationEvidence(state, event), null);
  }
});

test("post-respawn damage prefers an exact lethal over an impossible intermediate frame", () => {
  const base = {
    shooterId: 3,
    victimId: 768,
    victimLifeSeq: 9,
    energyBefore: 1_600,
  };
  const selected = postRespawnDamageEvent({
    hits: [
      { ...base, damageSeq: 41, energyAfter: 1_000, lethal: false },
      { ...base, damageSeq: 42, energyBefore: 1_000, energyAfter: 400, lethal: false },
      { ...base, damageSeq: 43, energyBefore: 400, energyAfter: 0, lethal: true },
    ],
    playerId: 3,
    victimId: 768,
    victimLifeSeq: 9,
  });
  assert.equal(selected.damageSeq, 43);
});

test("post-respawn damage keeps a nonlethal-only decrement snapshot-bound", () => {
  const selected = postRespawnDamageEvent({
    hits: [{
      shooterId: 3,
      victimId: 768,
      victimLifeSeq: 9,
      damageSeq: 41,
      energyBefore: 1_600,
      energyAfter: 1_000,
      lethal: false,
    }],
    playerId: 3,
    victimId: 768,
    victimLifeSeq: 9,
  });
  assert.equal(selected.damageSeq, 41);
  assert.equal(selected.lethal, false);
});

function combatCycleState(events, energyByDamageSeq = new Map()) {
  return {
    damageTrace: events.map((event) => ({
      victimId: event.victimId,
      attackerId: event.shooterId,
      damageSeq: event.damageSeq,
      projectileId: event.projectileId,
      serverTick: event.serverTick,
      victimLifeSeq: event.victimLifeSeq,
      shooterLifeSeq: event.shooterLifeSeq,
      weapon: event.weapon,
      damage: event.damage,
      energyBefore: event.energyBefore,
      energyAfter: event.energyAfter,
      explicitLethal: event.lethal,
    })),
    damagePresentationTrace: events.map((event, index) => ({
      at: 2_000 + index,
      damage: {
        damageSeq: event.damageSeq,
        projectileId: event.projectileId,
        serverTick: event.serverTick,
        victimId: event.victimId,
        victimLifeSeq: event.victimLifeSeq,
        attackerId: event.shooterId,
        shooterLifeSeq: event.shooterLifeSeq,
        weapon: event.weapon,
        amount: event.damage,
        energyBefore: event.energyBefore,
        energyAfter: event.energyAfter,
        explicitLethal: event.lethal,
      },
      sprite: {
        id: event.victimId,
        lifeSeq: event.victimLifeSeq,
        drawn: true,
        suppression: "none",
        lastServerPositionTick: event.serverTick,
        authoritativeX: 8_200,
        authoritativeY: 8_200,
      },
      sameLifeEnergySnapshot: energyByDamageSeq.has(event.damageSeq)
        ? {
            observedAt: 2_010 + index,
            snapshotId: 50 + index,
            serverTick: event.serverTick,
            playerId: event.victimId,
            lifeSeq: event.victimLifeSeq,
            energy: energyByDamageSeq.get(event.damageSeq),
            serverEnergy: energyByDamageSeq.get(event.damageSeq),
          }
        : null,
    })),
  };
}

const coalescedCycle = [
  {
    damageSeq: 21,
    projectileId: 41,
    serverTick: 100,
    victimId: 768,
    victimLifeSeq: 4,
    shooterId: 7,
    shooterLifeSeq: 2,
    weapon: 66,
    damage: 600,
    energyBefore: 1_600,
    energyAfter: 1_000,
    lethal: false,
  },
  {
    damageSeq: 22,
    projectileId: 42,
    serverTick: 105,
    victimId: 768,
    victimLifeSeq: 4,
    shooterId: 7,
    shooterLifeSeq: 2,
    weapon: 66,
    damage: 600,
    energyBefore: 1_015,
    energyAfter: 415,
    lethal: false,
  },
  {
    damageSeq: 23,
    projectileId: 43,
    serverTick: 107,
    victimId: 768,
    victimLifeSeq: 4,
    shooterId: 7,
    shooterLifeSeq: 2,
    weapon: 66,
    damage: 600,
    energyBefore: 421,
    energyAfter: 0,
    lethal: true,
  },
];

const atomicTerminalCycle = coalescedCycle.map((event, index) => ({
  ...event,
  energyBefore: [1_600, 1_000, 400][index],
  energyAfter: [1_000, 400, 0][index],
}));

test("combat-cycle evidence accepts a coalesced intermediate energy snapshot", () => {
  const state = combatCycleState(coalescedCycle, new Map([[21, 1_000]]));
  const entries = presentedBotCombatCycleEvidence(
    state,
    coalescedCycle,
    coalescedCycle.at(-1),
  );
  assert.equal(entries.length, 3);
  assert.equal(entries[0].evidence.energySnapshot.energy, 1_000);
  assert.equal(entries[1].evidence.energySnapshot, null);
  assert.equal(entries[2].evidence.trace.explicitLethal, true);
});

test("combat-cycle evidence still requires a visible nonlethal energy decrement", () => {
  const state = combatCycleState(coalescedCycle);
  assert.equal(
    presentedBotCombatCycleEvidence(
      state,
      coalescedCycle,
      coalescedCycle.at(-1),
    ),
    null,
  );
});

test("combat-cycle evidence accepts one atomic terminal delivery with no possible energy frame", () => {
  const state = combatCycleState(atomicTerminalCycle);
  for (const entry of state.damagePresentationTrace) {
    entry.at = 2_000;
    entry.snapshotId = 77;
  }
  const terminalCommitEvidence = {
    snapshotBoundaryCount: 0,
    damageSeqs: atomicTerminalCycle.map((event) => event.damageSeq),
    projectileIds: atomicTerminalCycle.map((event) => event.projectileId),
  };
  const entries = presentedBotCombatCycleEvidence(
    state,
    atomicTerminalCycle,
    atomicTerminalCycle.at(-1),
    { terminalCommitEvidence },
  );
  assert.equal(entries.length, 3);
  assert.ok(entries.every((entry) => entry.terminalCoalesced === true));
  assert.ok(entries.every((entry) => entry.evidence.energySnapshot == null));
  assert.ok(entries.every((entry) => entry.terminalPresentationSpanMs === 0));
});

test("atomic terminal presentation permits only bounded unordered render frames", () => {
  const reorderedState = combatCycleState(atomicTerminalCycle);
  const byDamageSeq = new Map(
    reorderedState.damagePresentationTrace.map((entry) => [entry.damage.damageSeq, entry]),
  );
  byDamageSeq.get(atomicTerminalCycle[0].damageSeq).at = 2_017;
  byDamageSeq.get(atomicTerminalCycle[0].damageSeq).snapshotId = 78;
  for (const event of atomicTerminalCycle.slice(1)) {
    byDamageSeq.get(event.damageSeq).at = 2_000;
    byDamageSeq.get(event.damageSeq).snapshotId = 77;
  }
  const terminalCommitEvidence = {
    snapshotBoundaryCount: 0,
    damageSeqs: atomicTerminalCycle.map((event) => event.damageSeq),
    projectileIds: atomicTerminalCycle.map((event) => event.projectileId),
  };
  const reordered = presentedBotCombatCycleEvidence(
    reorderedState,
    atomicTerminalCycle,
    atomicTerminalCycle.at(-1),
    { terminalCommitEvidence },
  );
  assert.ok(reordered.every((entry) => entry.terminalCoalesced === true));
  assert.ok(reordered.every((entry) => entry.terminalPresentationSpanMs === 17));

  byDamageSeq.get(atomicTerminalCycle[0].damageSeq).at = 2_051;
  assert.equal(
    presentedBotCombatCycleEvidence(
      reorderedState,
      atomicTerminalCycle,
      atomicTerminalCycle.at(-1),
      { terminalCommitEvidence },
    ),
    null,
  );
});

test("atomic terminal presentation rejects a discontinuous energy chain", () => {
  const discontinuousCycle = coalescedCycle.map((event) => ({ ...event }));
  const state = combatCycleState(discontinuousCycle);
  for (const entry of state.damagePresentationTrace) {
    entry.at = 2_000;
    entry.snapshotId = 77;
  }
  assert.equal(
    presentedBotCombatCycleEvidence(
      state,
      discontinuousCycle,
      discontinuousCycle.at(-1),
      {
        terminalCommitEvidence: {
          snapshotBoundaryCount: 0,
          damageSeqs: discontinuousCycle.map((event) => event.damageSeq),
          projectileIds: discontinuousCycle.map((event) => event.projectileId),
        },
      },
    ),
    null,
  );
});

test("atomic terminal presentation binds to a snapshot-free exact server commit", () => {
  const retained = coalescedCycle.map((event, index) => ({
    ...event,
    serverTick: [8_853, 8_876, 8_876][index],
    energyBefore: [1_650, 1_040, 430][index],
    energyAfter: [1_040, 430, 0][index],
  }));
  let logOffset = 0;
  const damageLines = retained.map((event) => {
    const line = `zubspace_event ${JSON.stringify({
      type: "server_damage_applied",
      damage_seq: event.damageSeq,
      projectile_id: event.projectileId,
      server_tick: event.serverTick,
      victim_id: event.victimId,
      shooter_id: event.shooterId,
      victim_life_seq: event.victimLifeSeq,
      shooter_life_seq: event.shooterLifeSeq,
      weapon: event.weapon,
      damage: event.damage,
      unprotected_damage: event.damage,
      shield_until_tick: 0,
      shield_remaining_ticks: 0,
      shield_lifetime_ticks: 0,
      emp_until_tick: 0,
      energy_before: event.energyBefore,
      energy_after: event.energyAfter,
      lethal: event.lethal,
      kind: "primary",
    })}\n`;
    const hit = { ...event, logIndex: logOffset };
    logOffset += line.length;
    return { hit, line };
  });
  const hits = damageLines.map(({ hit }) => hit);
  const snapshotTrace = "server bot snapshot trace\n";
  const logs = snapshotTrace + damageLines.map(({ line }) => line).join("");
  for (const hit of hits) hit.logIndex += snapshotTrace.length;
  const terminalCommitEvidence = snapshotFreeDamageCommitEvidence(logs, hits);
  assert.deepEqual(terminalCommitEvidence.damageSeqs, [21, 22, 23]);
  assert.equal(terminalCommitEvidence.snapshotBoundaryCount, 0);

  const state = combatCycleState(hits);
  for (const entry of state.damagePresentationTrace) {
    entry.at = 2_000;
    entry.snapshotId = 77;
  }
  const entries = presentedBotCombatCycleEvidence(
    state,
    hits,
    hits.at(-1),
    { terminalCommitEvidence },
  );
  assert.ok(entries.every((entry) => entry.terminalCoalesced === true));

  const splitLogs = `${snapshotTrace}${damageLines[0].line}${snapshotTrace}${
    damageLines.slice(1).map(({ line }) => line).join("")
  }`;
  const splitHits = [
    { ...hits[0] },
    ...hits.slice(1).map((hit) => ({
      ...hit,
      logIndex: hit.logIndex + snapshotTrace.length,
    })),
  ];
  assert.equal(snapshotFreeDamageCommitEvidence(splitLogs, splitHits), null);
});

test("combat-cycle evidence rejects a coalesced hit without exact presentation", () => {
  const state = combatCycleState(coalescedCycle, new Map([[21, 1_000]]));
  state.damagePresentationTrace.splice(1, 1);
  assert.equal(
    presentedBotCombatCycleEvidence(
      state,
      coalescedCycle,
      coalescedCycle.at(-1),
    ),
    null,
  );
});

test("stationary fixture evidence excludes moving lives before activation", () => {
  assert.deepEqual(
    stationaryFixtureLifeSeqs([1, 2, 2, 3], 2),
    [2, 3],
  );
  assert.deepEqual(
    stationaryFixtureLifeSeqs([0xffff_fffe, 0xffff_ffff, 1], 0xffff_ffff),
    [0xffff_ffff, 1],
  );
});

test("finite stationary fixture excludes the moving successor after a bullet-owned death", () => {
  // Retained Extreme sequence: bomb kills life 2, bullets kill life 3,
  // then the repeated bomb kills the ordinary moving life 4.
  assert.deepEqual(stationaryFixtureLifeSeqs([2, 3, 4], 1, [2, 3]), [2, 3]);
  assert.deepEqual(stationaryFixtureLifeSeqs([4], 1, [2, 3]), []);
  assert.deepEqual(stationaryFixtureLifeSeqs([2], 1, [2, 3]), [2]);
});

test("combat fixture audit requires configured lives but never a moving successor", () => {
  assert.deepEqual(requiredStationaryFixtureLifeSeqs([2, 4], 1, [2, 3]), [2, 3]);
  assert.deepEqual(requiredStationaryFixtureLifeSeqs([4], 1, [2, 3]), [2, 3]);
  assert.deepEqual(requiredStationaryFixtureLifeSeqs([1, 2, 3], 2), [2, 3]);
});

const death = (victimLifeSeq, victimNextLifeSeq, logIndex) => ({
  victimId: 768,
  victimLifeSeq,
  victimNextLifeSeq,
  serverTick: 1_000 + logIndex,
  energyAfter: 0,
  respawnDelayMs: 3_250,
  logIndex,
});

const damageLine = (damageSeq) => `zubspace_event ${JSON.stringify({
  type: "server_damage_applied",
  damage_seq: damageSeq,
  projectile_id: damageSeq,
  server_tick: 100 + damageSeq,
  victim_id: 768,
  shooter_id: 0,
  victim_life_seq: 1,
  shooter_life_seq: 1,
  weapon: 3,
  damage: 500,
  unprotected_damage: 500,
  shield_until_tick: 0,
  shield_remaining_ticks: 0,
  shield_lifetime_ticks: 0,
  emp_until_tick: 0,
  energy_before: 500,
  energy_after: 0,
  lethal: true,
  kind: "bot",
})}\n`;

function drainStatus(gameQueuePackets = 0) {
  return {
    rtc_inbound_packets: { game: 100, event: 10, reliable: 20 },
    rtc_ingress_execution: {limit: 4, workers: 4, in_flight: 0, waiting: 0},
    rtc_inbound_queue_packets: { game: gameQueuePackets, event: 0, reliable: 0 },
    rtc_inbound_peak_queue_packets: { game: 8, event: 1, reliable: 2 },
    rtc_inbound_backpressure: { game: 0, event: 0, reliable: 0 },
    position_input_queue_capacity_rejections: 0,
    realtime_event_queue_capacity_per_client: 3_072,
    realtime_event_queue_current: 0,
    realtime_event_queue_peak: 1,
    realtime_event_queue_overflows: 0,
    realtime_event_actions_rejected: 0,
    realtime_event_actions_deferred: 0,
    projectile_pending_hit_capacity_total: 3_072,
    projectile_pending_hit_capacity_per_owner: 128,
    projectile_pending_hit_retry_budget_per_tick: 256,
    projectile_pending_hit_current: 0,
    projectile_pending_hit_retained_bytes: 0,
    projectile_pending_damage_transactions: 0,
    projectile_pending_damage_payload_bytes: 0,
    rtc_outbound_reliable_overflows: 0,
    rtc_outbound_queue_packets: 0,
    rtc_outbound_queue_bytes: 0,
    repel_queue_packets: 0,
    repel_queue_max_target_depth: 0,
    repel_queue_awaiting_application: 0,
    repel_queue_awaiting_receipt_ack: 0,
    repel_queue_overflows: 0,
    movement_snapbacks_total: 0,
    movement_corrections_awaiting_application: 0,
    movement_corrections_awaiting_receipt_ack: 0,
  };
}

test("data-channel drain diagnostics identify the exact ingress blocker", () => {
  assert.equal(dataChannelDrainDisposition(drainStatus()).drained, true);
  for (const field of ["in_flight", "waiting"]) {
    const status = drainStatus();
    status.rtc_ingress_execution[field] = 1;
    const pending = dataChannelDrainDisposition(status);
    assert.equal(pending.drained, false);
    assert.equal(pending.blockers.ingressExecution[field], 1);
  }
  const blocked = dataChannelDrainDisposition(drainStatus(7));
  assert.equal(blocked.drained, false);
  assert.deepEqual(blocked.blockers.applicationQueuePackets, {
    game: 7,
    event: 0,
    reliable: 0,
  });

  const pendingCombat = drainStatus();
  pendingCombat.projectile_pending_hit_current = 1;
  pendingCombat.projectile_pending_hit_retained_bytes = 960;
  pendingCombat.projectile_pending_damage_transactions = 1;
  pendingCombat.projectile_pending_damage_payload_bytes = 1_760;
  const combatBlocked = dataChannelDrainDisposition(pendingCombat);
  assert.equal(combatBlocked.drained, false);
  assert.equal(combatBlocked.blockers.projectilePendingHitCurrent, 1);
  assert.equal(combatBlocked.blockers.projectilePendingHitRetainedBytes, 960);
  assert.equal(combatBlocked.blockers.projectilePendingDamageTransactions, 1);
  assert.equal(combatBlocked.blockers.projectilePendingDamagePayloadBytes, 1_760);
});

test("fresh-step alignment distinguishes lifecycle pauses from a live input freeze", () => {
  const live = {
    playerAlive: true,
    serverPlayerAlive: true,
    localGameplayState: {
      authorityPhase: "Live",
      authorityScopeAlive: true,
    },
  };
  assert.equal(logicalInputLifecyclePaused(live), false);
  assert.equal(logicalInputLifecyclePaused({
    ...live,
    serverPlayerAlive: false,
    localGameplayState: {
      authorityPhase: "AwaitingApplicationReceipt",
      authorityScopeAlive: false,
    },
  }), true);
  assert.equal(logicalInputLifecyclePaused({
    ...live,
    playerAlive: false,
  }), true);
  assert.equal(logicalInputLifecyclePaused({
    ...live,
    localGameplayState: {
      authorityPhase: "AwaitingApplicationReceipt",
      authorityScopeAlive: true,
    },
  }), true);
});

test("brake terminal classification accepts a post-pulse stopped sample", () => {
  const stopped = brakeVelocityDisposition({
    playerVelocityX: -0.18,
    playerVelocityY: -0.73,
  });
  assert.equal(stopped.braked, true);
  assert.ok(stopped.speed < 1);
  assert.equal(brakeVelocityDisposition({
    playerVelocityX: 1.4,
    playerVelocityY: 2.83,
  }).braked, true, "a sub-frame reverse-thrust remainder is stopped");
  assert.equal(brakeVelocityDisposition({
    playerVelocityX: 1.4,
    playerVelocityY: 2.83,
  }, 3).braked, false, "a strict settling route still requires its exact 3 px/s cap");
  assert.equal(brakeVelocityDisposition({
    playerVelocityX: 4,
    playerVelocityY: 0,
  }).braked, false);
});

test("Chaos bomb staging holds a stopped human target without erasing motion samples", () => {
  const staging = (overrides = {}) => chaosBombStationaryLaneDisposition({
    arena: "chaos-zone",
    weaponMode: "bomb",
    targetInFireRange: false,
    lineIsClear: true,
    distance: 120,
    pilotSpeed: 0,
    holdingRange: 140,
    ...overrides,
  });

  assert.deepEqual(staging({ pilotSpeed: 220 }), {
    hold: true,
    action: "brake",
  });
  assert.deepEqual(staging(), {
    hold: true,
    action: "wait",
  });
  assert.deepEqual(staging({ targetOwnsPursuit: false }), {
    hold: false,
    action: "pursue",
  }, "the browser must approach a deliberately stationary test target");
  assert.equal(staging({ targetInFireRange: true }).hold, false);
  assert.equal(staging({ distance: 80 }).hold, false);
  assert.equal(staging({ distance: 140 }).hold, true);
  assert.equal(staging({ holdingRange: Number.NaN }).hold, false);
  assert.equal(staging({ holdingRange: undefined }).hold, false);
  assert.equal(staging({ lineIsClear: false }).hold, false);
  assert.equal(staging({ arena: "extreme" }).hold, false);
  assert.equal(staging({ weaponMode: "bullet" }).hold, false);
});

test("Chaos bomb staging pursues outside the existing firing lane", () => {
  for (const distance of [140.01, 151.06843515440326, 192, 196.1117031184015, 900]) {
    for (const pilotSpeed of [0, 0.7495331880577405, 220]) {
      assert.deepEqual(chaosBombStationaryLaneDisposition({
        arena: "chaos-zone", weaponMode: "bomb", targetInFireRange: false,
        lineIsClear: true, distance, pilotSpeed, holdingRange: 140,
      }), { hold: false, action: "pursue" },
      `outside the unchanged 140px lane, ${distance}px must permit real pursuit`);
    }
  }
});

test("stationary Chaos bomb fixture chooses an exact real-key holding point", () => {
  assert.deepEqual(stationaryChaosBombHoldingPoint({
    fixtureActive: true,
    hullIsOpen: () => true, firingLineIsClear: () => true,
    arena: "chaos-zone",
    weaponMode: "bomb",
    lineIsClear: true,
    humanDamageObserved: true,
    targetMotionReady: true,
    targetSpeed: 0,
    playerX: 100,
    playerY: 200,
    targetX: 400,
    targetY: 200,
  }), { x: 324, y: 200 });
  assert.deepEqual(stationaryChaosBombHoldingPoint({
    fixtureActive: true,
    hullIsOpen: () => true, firingLineIsClear: () => true,
    arena: "chaos-zone",
    weaponMode: "bomb",
    lineIsClear: true,
    humanDamageObserved: true,
    targetMotionReady: true,
    targetSpeed: 0,
    playerX: 390,
    playerY: 200,
    targetX: 400,
    targetY: 200,
  }), { x: 324, y: 200 }, "an inside-safety pilot retreats to the same annulus");
  assert.equal(stationaryChaosBombHoldingPoint({
    fixtureActive: true,
    hullIsOpen: () => true, firingLineIsClear: () => true,
    arena: "chaos-zone",
    weaponMode: "bomb",
    lineIsClear: true,
    humanDamageObserved: true,
    targetMotionReady: true,
    targetSpeed: 0,
    playerX: 324,
    playerY: 200,
    targetX: 400,
    targetY: 200,
  }), null, "a settled pilot remains inside the valid 72..80px lane");
  assert.equal(stationaryChaosBombHoldingPoint({
    fixtureActive: false,
    arena: "chaos-zone",
    weaponMode: "bomb",
    lineIsClear: true,
    humanDamageObserved: true,
    targetMotionReady: true,
    targetSpeed: 0,
    playerX: 100,
    playerY: 200,
    targetX: 400,
    targetY: 200,
  }), null);
  for (const motion of [
    { humanDamageObserved: false, targetMotionReady: true, targetSpeed: 0 },
    { targetMotionReady: false, targetSpeed: 0 },
    { targetMotionReady: true, targetSpeed: 3.51 },
  ]) {
    assert.equal(stationaryChaosBombHoldingPoint({
      fixtureActive: true,
    hullIsOpen: () => true, firingLineIsClear: () => true,
      arena: "chaos-zone",
      weaponMode: "bomb",
      lineIsClear: true,
      humanDamageObserved: true,
      playerX: 100,
      playerY: 200,
      targetX: 400,
      targetY: 200,
      ...motion,
    }), null, "the stationary fixture must not intercept an undamaged/moving/unmeasured life");
  }
});

test("brake orientation accepts exact wire bins and sub-bin physical headings", () => {
  assert.equal(brakeOrientationDisposition(
    { playerOrientation: 0.02 },
    0,
    -41,
  ).oriented, true, "direction zero owns the retained 39 -> 0 wrap");

  const retainedAngle = 0.03397384529468006 * Math.PI * 2;
  assert.equal(brakeOrientationDisposition(
    { playerOrientation: 0.02 },
    Math.sin(retainedAngle),
    -Math.cos(retainedAngle),
  ).oriented, true, "a minimum pulse may skip the nearest bin but land inside the physical tolerance");
  assert.equal(brakeOrientationDisposition(
    { playerOrientation: 0.09 },
    Math.sin(retainedAngle),
    -Math.cos(retainedAngle),
  ).oriented, false, "an unrelated heading is not accepted as braking alignment");
});

test("settling arrival control requests zero velocity inside its accepted circle", () => {
  assert.equal(arrivalControlDistance(3.9, true, 4), 0);
  assert.equal(arrivalControlDistance(4, true, 4), 0);
  assert.equal(arrivalControlDistance(5.5, true, 4), 1.5);
});

test("authored door window requires fresh consecutive closed phases", () => {
  const metrics = (doorSeed, doorSeedNext, doorClockFresh = true) => ({
    localGameplayState: { doorSeed, doorSeedNext, doorClockFresh },
  });
  assert.equal(authoredDoorClosedForCurrentAndNext(metrics(0x80, 0x81), 169), true);
  assert.equal(authoredDoorClosedForCurrentAndNext(metrics(0x80, 0x7f), 169), false);
  assert.equal(authoredDoorClosedForCurrentAndNext(metrics(0x7f, 0x80), 169), false);
  assert.equal(authoredDoorClosedForCurrentAndNext(metrics(0x80, 0x80, false), 169), false);
});

test("moving hull corridor rejects an intercept that crosses authored geometry", () => {
  const openUntilWall = (x, y) => x < 100 && y >= 0;
  assert.equal(movingHullPathIsClear({
    x: 80,
    y: 20,
    velocityX: 40,
    velocityY: 0,
    seconds: 0.6,
    hullIsOpen: openUntilWall,
  }), false);
  assert.equal(movingHullPathIsClear({
    x: 80,
    y: 20,
    velocityX: -40,
    velocityY: 0,
    seconds: 0.6,
    hullIsOpen: openUntilWall,
  }), true);
  assert.equal(movingHullPathIsClear({
    x: 80,
    y: 20,
    velocityX: 0,
    velocityY: 0,
    seconds: 0.6,
    hullIsOpen: openUntilWall,
  }), true);
});

test("explosive contact model matches the reference game's pixel-truncated box overlap", () => {
  assert.equal(explosiveContactOverlapsHull({
    weaponX: 1_000.9,
    weaponY: 1_000.9,
    shipX: 1_017.9,
    shipY: 1_017.9,
  }), true, "a diagonal 4px weapon-box/14px hull-box contact is valid");
  assert.equal(explosiveContactOverlapsHull({
    weaponX: 1_000.9,
    weaponY: 1_000.9,
    shipX: 1_019.1,
    shipY: 1_000.9,
  }), false, "a separated axis is not contact");
});

test("the red Extreme bomb geometry is refused at the wire cell it emitted", () => {
  // Chrome/Extreme, `make test-e2e-immediate-bot-combat`. The pilot held
  // (7703, 9014) against a stationary bot at (7645.5, 8736.1) and spent fourteen bombs without one
  // damage transaction. All fourteen left at velocity (-1334, -4099): the continuous bearing
  // 0.9676 turns that the pilot was aimed at truncates into wire cell 38, whose swept path passes
  // 30 px beside the hull - outside the authored 4px explosive + 14px hull envelope.
  const shot = {
    relativeX: -57.45,
    relativeY: -277.9,
    targetVelocityX: 0,
    targetVelocityY: 0,
    playerVelocityX: 0,
    playerVelocityY: 0,
    projectileSpeed: 430,
    maxTime: 2.5,
  };
  const emitted = quantizedProjectileIntercept({ ...shot, fixedDirection: 38 });
  const best = quantizedProjectileIntercept(shot);
  assert.ok(emitted.missDistance > 18,
    "the cell the old continuous aim selected really cannot reach the hull");
  assert.equal(
    bombWireAimDisposition({ weaponMode: "bomb", arena: "extreme", quantized: emitted }).fire,
    false,
  );
  assert.ok(best.missDistance <= 18, "a wire cell that can reach it exists at this range");
  assert.notEqual(best.direction, emitted.direction);
  assert.deepEqual(
    bombWireAimDisposition({ weaponMode: "bomb", arena: "extreme", quantized: best }),
    { usesWireAim: true, fire: true, missDistance: best.missDistance, currentMissDistance: null },
  );
  // The cell-centre aim decides *which* cell the pilot turns into; the key press still belongs to
  // the cell the ship emits at that instant. A half-bin of turn cadence that spilled into the
  // neighbouring cell must not be allowed to spend the bomb.
  assert.deepEqual(
    bombWireAimDisposition({
      weaponMode: "bomb", arena: "extreme", quantized: best, current: emitted,
    }),
    {
      usesWireAim: true,
      fire: false,
      missDistance: best.missDistance,
      currentMissDistance: emitted.missDistance,
    },
  );
  assert.equal(
    bombWireAimDisposition({
      weaponMode: "bomb", arena: "extreme", quantized: best, current: best,
    }).fire,
    true, "the solved cell, actually emitted, is the shot that was predicted",
  );
});

test("Chaos proximity bombs keep their authored radial contact decision", () => {
  for (const quantized of [null, { feasible: false }, { feasible: true, missDistance: 400 }]) {
    assert.deepEqual(
      bombWireAimDisposition({ weaponMode: "bomb", arena: "chaos-zone", quantized }),
      { usesWireAim: false, fire: true, missDistance: null, currentMissDistance: null },
      "the launched line does not decide Chaos contact",
    );
  }
});

test("aged Extreme bomb pilot fires its reachable exact wire cell instead of oscillating around its centre", () => {
  // 7dca4b8 aged Chrome RED, metricsTrace at1000506305976: human life4
  // (7905.92,8263.80), stationary bot768/life3 at(7882,8161).
  const shot = { relativeX: 7882 - 7905.92, relativeY: 8161 - 8263.80,
    targetVelocityX: 0, targetVelocityY: 0, playerVelocityX: 0.23, playerVelocityY: 0.58,
    projectileSpeed: 430, maxTime: 2.5 };
  const quantized = quantizedProjectileIntercept(shot);
  assert.equal(quantized.direction, 39);
  for (const orientation of [0.98, 0.995]) {
    const direction = Math.floor(Math.fround(orientation) * 40) % 40;
    const current = quantizedProjectileIntercept({ ...shot, fixedDirection: direction });
    const delta = ((quantized.orientation - orientation + 1.5) % 1) - 0.5;
    assert.ok(Math.abs(delta) > 0.006, "the old centre gate refuses both sampled headings");
    const wireAim = bombWireAimDisposition({ weaponMode: "bomb", arena: "extreme", quantized, current });
    assert.equal(botFiringHeadingReady({ delta, tolerance: 0.013, wireAim, quantized, current }), true);
    assert.equal(direction, quantized.direction, "the accepted key emits the solved cell");
    assert.ok(current.missDistance <= 18, "the same native hull/contact envelope is required");
    const neighbour = quantizedProjectileIntercept({ ...shot, fixedDirection: (direction + 1) % 40 });
    assert.equal(botFiringHeadingReady({ delta: 0, tolerance: 0.013,
      wireAim: bombWireAimDisposition({ weaponMode: "bomb", arena: "extreme", quantized, current: neighbour }),
      quantized, current: neighbour }), false, "continuous alignment cannot authorize a different emitted cell");
    assert.equal(botFiringHeadingReady({ delta, tolerance: 0.013, wireAim: { ...wireAim, fire: false }, quantized, current }), false);
    assert.equal(botFiringHeadingReady({ delta, tolerance: 0.013, wireAim, quantized, current: null }), false);
  }
  for (const delta of [0, 0.012, 0.014, -0.014]) {
    assert.equal(botFiringHeadingReady({ delta, tolerance: 0.013, wireAim: { usesWireAim: false } }), Math.abs(delta) <= 0.013,
      "Chaos proximity and bullet heading thresholds are unchanged");
  }
});

test("a sensorless explosive never fires without a wire solution", () => {
  const at = (quantized) => bombWireAimDisposition({ weaponMode: "bomb", arena: "extreme", quantized });
  assert.equal(at(null).fire, false);
  assert.equal(at({ feasible: true, missDistance: Number.NaN }).fire, false);
  assert.equal(at({ feasible: false, missDistance: 1 }).fire, false);
  assert.equal(at({ feasible: true, missDistance: 18.5 }).fire, false, "outside 4px + 14px");
  assert.equal(at({ feasible: true, missDistance: 18 }).fire, true, "the authored envelope itself");
  assert.equal(
    bombWireAimDisposition({ weaponMode: "bullet", arena: "extreme", quantized: null }).fire,
    true, "bullets are gated by their own 12px wire binding elsewhere",
  );
});

test("respawn presentation can overtake only an exact bounded death chain", () => {
  const initial = death(1, 2, 10);
  const overtaken = death(2, 3, 20);
  const resolved = resolvePresentedRespawnChain(
    [initial, overtaken],
    initial,
    3,
  );

  assert.equal(resolved.presentedLifeSeq, 3);
  assert.equal(resolved.terminalDeath, overtaken);
  assert.deepEqual(resolved.overtakenDeaths, [overtaken]);
});

test("respawn presentation rejects a newer life with no exact death chain", () => {
  const initial = death(1, 2, 10);
  assert.equal(resolvePresentedRespawnChain([initial], initial, 3), null);
});

test("projectile-tail evidence waits for every hit or authored expiry across wrap", () => {
  const weaponTypes = new Set([1, 2]);
  const projectileEvents = [
    {
      type: "server_projectile_spawned",
      projectileId: 10,
      ownerId: 7,
      weapon: 2,
      expiryTick: 0x7fff_fffe,
    },
    {
      type: "server_projectile_spawned",
      projectileId: 11,
      ownerId: 7,
      weapon: 2,
      expiryTick: 2,
    },
    {
      type: "server_projectile_hit",
      projectileId: 10,
      ownerId: 7,
      weapon: 2,
    },
    {
      type: "server_projectile_spawned",
      projectileId: 12,
      ownerId: 8,
      weapon: 2,
      expiryTick: 20,
    },
  ];
  assert.equal(ownedProjectileTailSettled({
    projectileEvents,
    ownerId: 7,
    latestServerTick: 1,
    weaponTypes,
  }), false);
  assert.equal(ownedProjectileTailSettled({
    projectileEvents,
    ownerId: 7,
    latestServerTick: 2,
    weaponTypes,
  }), true);
});

test("incremental server damage reader preserves exact log boundaries", () => {
  const first = damageLine(1);
  const second = damageLine(2);
  const server = { logs: [first, second.slice(0, 31)] };
  const boundary = serverDamageBoundary(server);
  assert.deepEqual(serverDamageEventsSince(server, 0).map((event) => event.damageSeq), [1]);

  server.logs.push(second.slice(31), damageLine(3));
  assert.deepEqual(
    serverDamageEventsSince(server, boundary).map((event) => event.damageSeq),
    [3],
    "an event whose line began before the boundary remains predecessor evidence",
  );
});

test("repel presentation accepts the exact outward impulse before a nearby wall bounce", () => {
  const applied = {
    origin_x: 8_200,
    origin_y: 8_200,
    target_x: 8_193.401,
    target_y: 8_152.739,
  };
  const wallLimited = presentedBotRepelEvidence({
    applied,
    target: {
      positionX: 8_192,
      positionY: 8_145,
      velocityX: -66.4,
      velocityY: -475.4,
    },
  });
  assert.equal(wallLimited.delivered, true);
  assert.ok(wallLimited.outwardDisplacement > 0);
  assert.ok(wallLimited.outwardDisplacement < 12);
  assert.ok(wallLimited.presentedSpeed >= 450);
  assert.ok(wallLimited.presentedAlignment >= 0.98);

  assert.equal(presentedBotRepelEvidence({
    applied,
    target: {
      positionX: 8_193,
      positionY: 8_152,
      velocityX: 40,
      velocityY: 40,
    },
  }).delivered, false, "ordinary bot steering is not a Repel delivery proof");
});

test("browser Repel staging excludes a same-tick wall collision", () => {
  const northWallAt = 8_141;
  const hullIsOpen = (_x, y) => y > northWallAt;
  assert.equal(repelPresentationCorridorIsOpen({
    sourceX: 8_200,
    sourceY: 8_200,
    targetX: 8_198,
    targetY: 8_149,
    hullIsOpen,
  }), false, "the retained eight-pixel wall corridor cannot present a 12px impulse");
  assert.equal(repelPresentationCorridorIsOpen({
    sourceX: 8_200,
    sourceY: 8_200,
    targetX: 8_198,
    targetY: 8_100,
    hullIsOpen: (_x, y) => y > 8_000,
  }), true, "an authored 48px open corridor is eligible for visible delivery");
});

test("combat lead derives velocity from the authoritative wrap-safe position tick", () => {
  assert.deepEqual(continuumPositionVelocity({
    serverTick: 0xffff,
    x: 10,
    y: 20,
  }, {
    serverTick: 1,
    x: 14,
    y: 18,
  }), {
    x: 200,
    y: -100,
    elapsedTicks: 2,
  });
  assert.equal(continuumPositionVelocity({
    serverTick: 8,
    x: 10,
    y: 20,
  }, {
    serverTick: 8,
    x: 14,
    y: 18,
  }), null);
});

test("both browser pilots lead crossing targets at world speed rather than wire velocity", () => {
  for (const file of ["bot_authority.mjs", "bot_authority_firefox.mjs"]) {
    const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
    const expression = /const projectileSpeed = ([\s\S]*?);/.exec(source)?.[1];
    const aimExpression = /const aim = ([\s\S]*?);/.exec(source)?.[1];
    assert.ok(expression, `${file}: missing projectile speed used by live aim`);
    assert.ok(aimExpression, `${file}: missing live aim selection`);
    // A stationary launcher and a target crossing 100px away at 100px/s.
    // The actual production world step divides wire velocities by ten.
    for (const [arena, weaponMode, worldSpeed] of [
      ["chaos-zone", "bullet", 200], ["chaos-zone", "bomb", 200],
      ["extreme", "bullet", 360], ["extreme", "bomb", 430],
    ]) {
      const speed = vm.runInNewContext(`(${expression})`, { arena, weaponMode });
      const lead = projectileInterceptVector(0, 100, 100, 0, speed, 2);
      assert.equal(lead.feasible, true);
      const aim = vm.runInNewContext(`(${aimExpression})`, {
        arena, weaponMode, distance: 100, dx: 0, dy: 100,
        quantizedAim: null, pointBlankBombContact: false, lead,
        // This fixture owns the existing world-speed lead fallback. The
        // blast-safe moving-Chaos candidate has its own selection check below.
        chaosBombPlan: null,
      });
      const length = Math.hypot(aim.x, aim.y);
      const projectileX = aim.x / length * worldSpeed * lead.time;
      const projectileY = aim.y / length * worldSpeed * lead.time;
      const miss = Math.hypot(projectileX - 100 * lead.time, projectileY - 100);
      assert.ok(miss < 0.001, `${file} ${arena} ${weaponMode}: world-flight miss ${miss}px`);
    }
  }
});

test("both browser pilots prefer the actual safe moving-bomb candidate over fallback lead", () => {
  const plan = movingBombAttackPlan({ relativeX: 0, relativeY: 320,
    targetVelocityX: 100, targetVelocityY: 0, playerVelocityX: 0, playerVelocityY: 0,
    projectileSpeed: 200, maximumFlightSeconds: 2, explosionRadius: 160,
    proximityRadius: 72, currentDirection: 20 });
  assert.ok(plan.candidate?.safeToAttempt);
  for (const file of ["bot_authority.mjs", "bot_authority_firefox.mjs"]) {
    const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
    const expression = /const aim = ([\s\S]*?);/.exec(source)?.[1];
    assert.ok(expression);
    const aim = vm.runInNewContext(`(${expression})`, {
      arena: "chaos-zone", weaponMode: "bomb", distance: 320, dx: 0, dy: 320,
      quantizedAim: null, pointBlankBombContact: false, lead: { x: 0, y: 320 },
      chaosBombPlan: plan,
    });
    assert.equal(aim, plan.candidate, `${file}: preserve the actual blast-safe firing cell`);
  }
});

test("projectile intercept leads relative target motion without exceeding its bound", () => {
  const crossing = projectileInterceptVector(0, 120, -150, 0, 430);
  assert.equal(crossing.feasible, true);
  assert.ok(crossing.time > 0 && crossing.time < 0.75);
  assert.ok(crossing.x < -40);
  assert.equal(crossing.y, 120);

  // The authored 4300 wire speed is 430px/s, so this ~406px separation
  // exercises a substantial fraction of the two-second flight bound.
  const longFlightBomb = projectileInterceptVector(-355, 202, -80, 87, 430, 2);
  assert.equal(longFlightBomb.feasible, true);
  assert.ok(longFlightBomb.time > 0.75 && longFlightBomb.time < 2);

  const unreachable = projectileInterceptVector(0, 120, 500, 0, 100, 0.5);
  assert.equal(unreachable.feasible, false);
  assert.equal(unreachable.time, 0.5);
  assert.deepEqual(projectileInterceptVector(0, 120, 0, 0, 0), null);
});

test("moving-target aim selects the lowest-miss 40-bin protocol direction", () => {
  const aim = quantizedProjectileIntercept({
    relativeX: -54.56,
    relativeY: 97.02,
    targetVelocityX: -144,
    targetVelocityY: 164.5,
    playerVelocityX: -165.29,
    playerVelocityY: -1.25,
    projectileSpeed: 430,
    maxTime: 2,
  });
  assert.equal(aim.direction, 22);
  assert.ok(aim.time > 0 && aim.time < 0.5);
  assert.ok(aim.missDistance < 6, JSON.stringify(aim));
  assert.equal(Math.floor(aim.orientation * 40), aim.direction);
  const exactWireDirection = quantizedProjectileIntercept({
    relativeX: -54.56,
    relativeY: 97.02,
    targetVelocityX: -144,
    targetVelocityY: 164.5,
    playerVelocityX: -165.29,
    playerVelocityY: -1.25,
    projectileSpeed: 430,
    maxTime: 2,
    fixedDirection: 22,
  });
  assert.equal(exactWireDirection.direction, 22);
  assert.equal(exactWireDirection.missDistance, aim.missDistance);
  const adjacentWireDirection = quantizedProjectileIntercept({
    relativeX: -54.56,
    relativeY: 97.02,
    targetVelocityX: -144,
    targetVelocityY: 164.5,
    playerVelocityX: -165.29,
    playerVelocityY: -1.25,
    projectileSpeed: 430,
    maxTime: 2,
    fixedDirection: 21,
  });
  assert.equal(adjacentWireDirection.direction, 21);
  assert.ok(adjacentWireDirection.missDistance > aim.missDistance);

  const thrustInherited = quantizedProjectileIntercept({
    // Retained Firefox Extreme safe-edge sample: the ship leaves safety while
    // already pointing along direction 31, so its 170 px/s exit velocity is
    // collinear with the bomb rather than with the cardinal map boundary.
    relativeX: -147.54117647058825,
    relativeY: -76.04,
    targetVelocityX: -144,
    targetVelocityY: 156.4,
    playerVelocityX: -170,
    playerVelocityY: 0,
    projectileSpeed: 430,
    maxTime: 2,
    playerSpeedAlongDirection: 170,
  });
  assert.equal(thrustInherited.direction, 31);
  assert.ok(Math.abs(thrustInherited.playerVelocityX + 167.907) < 0.01);
  assert.ok(Math.abs(thrustInherited.playerVelocityY + 26.594) < 0.01);
  assert.ok(thrustInherited.missDistance < 6, JSON.stringify(thrustInherited));

  assert.equal(quantizedProjectileIntercept({
    relativeX: 0,
    relativeY: 0,
    targetVelocityX: 0,
    targetVelocityY: 0,
    playerVelocityX: 0,
    playerVelocityY: 0,
    projectileSpeed: 0,
    maxTime: 1,
  }), null);
});

test("stationary Chaos aim selects the hittable wire cell instead of the visual cell", () => {
  const exact = quantizedProjectileIntercept({
    relativeX: -245,
    relativeY: 191,
    targetVelocityX: 0,
    targetVelocityY: 0,
    playerVelocityX: 0,
    playerVelocityY: 0,
    projectileSpeed: 200,
    maxTime: 5.5,
  });
  const visuallySelected = quantizedProjectileIntercept({
    relativeX: -245,
    relativeY: 191,
    targetVelocityX: 0,
    targetVelocityY: 0,
    playerVelocityX: 0,
    playerVelocityY: 0,
    projectileSpeed: 200,
    maxTime: 5.5,
    fixedDirection: 25,
  });
  assert.equal(exact.direction, 26);
  assert.ok(exact.missDistance < 12, JSON.stringify(exact));
  assert.ok(visuallySelected.missDistance > 38, JSON.stringify(visuallySelected));
});

test("safe-edge projectile staging distinguishes closing and receding lanes", () => {
  assert.equal(projectileRadialVelocity(-182, 99, -89, 107) > 120, true);
  assert.equal(projectileRadialVelocity(-51, 201, -153, 171) > 200, true);
  assert.equal(projectileRadialVelocity(0, 200, 40, -90), -90);
  assert.equal(projectileRadialVelocity(200, 0, 0, 40), 0);
  assert.equal(projectileRadialVelocity(0, 0, 500, 500), 0);
  assert.equal(projectileRadialVelocity(Number.NaN, 0, 0, 0), null);
});

test("projectile ownership presentation is scoped to the actual viewport", () => {
  const frame = {
    playerX: 8_000,
    playerY: 8_000,
    screenWidth: 1_280,
    screenHeight: 720,
  };
  assert.equal(worldPointOnCenteredViewport(frame, 8_000, 8_000), true);
  assert.equal(worldPointOnCenteredViewport(frame, 8_663, 8_000), true);
  assert.equal(worldPointOnCenteredViewport(frame, 8_665, 8_000), false);
  assert.equal(worldPointOnCenteredViewport(frame, 8_000, 8_383), true);
  assert.equal(worldPointOnCenteredViewport(frame, 8_000, 8_385), false);
  assert.equal(worldPointOnCenteredViewport({}, 8_000, 8_000), false);
});

test("bounded metrics traces inherit their enclosing page viewport", () => {
  const [frame] = metricsTraceWithViewport({
    uiMetrics: { screenWidth: 1_280, screenHeight: 720 },
    metricsTrace: [{ at: 7, playerX: 8_000, playerY: 8_000 }],
  });
  assert.equal(frame.screenWidth, 1_280);
  assert.equal(frame.screenHeight, 720);
  assert.equal(worldPointOnCenteredViewport(frame, 8_640, 8_000), true);
});

test("compact bot presentation capture keeps exact evidence without unrelated history", () => {
  const compact = compactBotPresentationState({
    uiMetrics: {
      screenWidth: 1_280,
      authoritativeSnapshotActorTrace: [{ serial: 1 }, { serial: 2 }],
      authoritativeSnapshotActorStream: {
        latestSerial: 2,
        frames: [{ serial: 1 }, { serial: 2 }],
      },
    },
    metricsTrace: [
      { at: 90, remotePlayers: [{ id: 768 }] },
      {
        at: 110,
        latestSnapshotId: 75,
        latestSnapshotServerTick: 77,
        latestServerTick: 77,
        playerX: 8_000,
        playerY: 8_000,
        authoritativeSnapshotActors: [{ id: 768, lifeSeq: 4 }, { id: 7, lifeSeq: 9 }],
        remoteActorPresentation: [{ id: 768, lifeSeq: 4 }, { id: 7, lifeSeq: 9 }],
        remotePlayers: [{ id: 768, lifeSeq: 4 }, { id: 7, lifeSeq: 9 }],
        presentedRemoteProjectileSources: [
          { playerId: 768, lifeSeq: 4, eventId: 2, x: 10 },
          { playerId: 7, lifeSeq: 9, eventId: 4, x: 20 },
        ],
        remoteProjectileSources: [
          { playerId: 768, lifeSeq: 4, eventId: 3 },
          { playerId: 7, lifeSeq: 9, eventId: 4 },
        ],
      },
    ],
    remoteSpriteTrace: [{
      at: 111,
      presentationRenderTick: 31188,
      entries: [{ id: 768, drawn: true }, { id: 7, drawn: true }],
      projectiles: {
        total: 2,
        entries: [
          { sourcePlayerId: 768, drawn: true },
          { sourcePlayerId: 7, drawn: true },
        ],
      },
    }],
    packetLog: [
      { direction: "received", byteLength: 20, bytesTruncated: true, bytes: [0x70] },
      { direction: "received", byteLength: 10, bytesTruncated: true, bytes: [0x3a] },
    ],
    audioUnlock: { installed: true },
  }, [768], 100);

  assert.equal(compact.metricsTrace.length, 1);
  assert.deepEqual(compact.metricsTrace[0].authoritativeSnapshotActors, [
    { id: 768, lifeSeq: 4 },
  ]);
  assert.deepEqual(compact.metricsTrace[0].remoteProjectileSources, [
    { playerId: 768, lifeSeq: 4, eventId: 3 },
  ]);
  assert.deepEqual(compact.metricsTrace[0].presentedRemoteProjectileSources, [
    { playerId: 768, lifeSeq: 4, eventId: 2, x: 10 },
  ]);
  assert.equal(compact.metricsTrace[0].latestSnapshotId, 75);
  assert.equal(compact.metricsTrace[0].latestSnapshotServerTick, 77);
  assert.deepEqual(compact.remoteSpriteTrace[0].entries, [{ id: 768, drawn: true }]);
  assert.equal(compact.remoteSpriteTrace[0].presentationRenderTick, 31188);
  assert.deepEqual(compact.remoteSpriteTrace[0].projectiles.entries, [
    { sourcePlayerId: 768, drawn: true },
  ]);
  assert.deepEqual(compact.uiMetrics.authoritativeSnapshotActorTrace, [{ serial: 2 }]);
  assert.deepEqual(compact.uiMetrics.authoritativeSnapshotActorStream.frames, [{ serial: 2 }]);
  assert.deepEqual(compact.packetAudit, {
    inspectedPackets: 2,
    truncatedPackets: 2,
    maximumRetainedBytes: 1,
    receivedLegacyPositionRelays: 1,
    samples: [{
      direction: "received",
      byteLength: 10,
      bytesTruncated: true,
      bytes: [0x3a],
    }],
  });
  assert.deepEqual(
    compactBotPresentationState({ packetLog: [{ direction: "received", bytes: [0x3a] }] }, [768], 0, false)
      .packetAudit,
    {
      inspectedPackets: 0,
      truncatedPackets: 0,
      maximumRetainedBytes: 0,
      receivedLegacyPositionRelays: 0,
      samples: [],
    },
  );
});

test("movement windows measure elapsed continuity instead of assuming 30 FPS", () => {
  const samples = Array.from({ length: 101 }, (_, index) => ({
    at: index * 40,
    x: index,
    y: 0,
  }));
  assert.deepEqual(timedMovementWindows(samples, { stride: 60 }), [{
    startedAt: 0,
    endedAt: 4_000,
    displacement: 100,
  }]);

  const sparse = [
    { at: 0, x: 0, y: 0 },
    { at: 1_000, x: 50, y: 0 },
    { at: 4_000, x: 100, y: 0 },
  ];
  assert.deepEqual(timedMovementWindows(sparse), []);
});

test("stopped bot translation requires sustained lethal attack evidence from one exact life", () => {
  const hit = (damageSeq, shooterLifeSeq, lethal = false, shooterId = 768) => ({
    damageSeq,
    serverTick: 1_000 + damageSeq * 10,
    shooterId,
    shooterLifeSeq,
    lethal,
  });
  const options = { botId: 768, lifeSeqs: [3] };

  assert.equal(sustainedExactLifeAttackEvidence([], options), null);
  assert.deepEqual(
    sustainedExactLifeAttackEvidence([
      hit(1, 2),
      hit(2, 2, true),
    ], options),
    null,
  );
  assert.equal(
    sustainedExactLifeAttackEvidence([
      hit(1, 3),
      hit(2, 3),
    ], options),
    null,
  );
  assert.equal(
    sustainedExactLifeAttackEvidence([hit(1, 3, true)], options),
    null,
  );
  assert.deepEqual(
    sustainedExactLifeAttackEvidence([
      hit(1, 3),
      hit(1, 3),
      hit(2, 3, true),
      hit(3, 3, false, 769),
    ], options),
    {
      botId: 768,
      lifeSeq: 3,
      hitCount: 2,
      lethalHitCount: 1,
      firstServerTick: 1_010,
      lastServerTick: 1_020,
      damageSeqs: [1, 2],
      lethalDamageSeqs: [2],
    },
  );
});

test("render traces measure elapsed continuity instead of assuming 30 FPS", () => {
  const twentyFiveFps = Array.from({ length: 201 }, (_, index) => ({
    at: index * 40,
  }));
  assert.deepEqual(timedTraceContinuity(twentyFiveFps, {
    minimumSpanMs: 7_000,
  }), {
    ok: true,
    sampleCount: 201,
    spanMs: 8_000,
    maximumGapMs: 40,
  });

  const split = [
    { at: 0 },
    { at: 40 },
    { at: 80 },
    { at: 1_000 },
    { at: 1_040 },
  ];
  assert.deepEqual(timedTraceContinuity(split, {
    minimumSpanMs: 1_000,
  }), {
    ok: false,
    sampleCount: 5,
    spanMs: 1_040,
    maximumGapMs: 920,
  });
  assert.equal(longestTimedTraceSpan(split), 80);
});

test("remote actor smoothness rejects freeze-then-jump and scales for fast actors", () => {
  const sample = (at, positionX, displayX, velocityX) => ({
    at,
    lifeSeq: 3,
    snapCount: 0,
    initialCount: 0,
    positionX,
    positionY: 8_000,
    displayX,
    displayY: 8_000,
    velocityX,
    velocityY: 0,
  });
  const frozen = [
    sample(0, 0, 0, 150),
    sample(20, 3, 0, 150),
    sample(40, 6, 0, 150),
    sample(60, 9, 0, 150),
    sample(80, 12, 0, 150),
    sample(100, 15, 0, 150),
    sample(120, 18, 18, 150),
  ];
  const rejected = remoteActorMotionSmoothnessEvidence(frozen);
  assert.equal(rejected.ok, false);
  assert.equal(rejected.materialHolds.length, 1);
  assert.deepEqual(rejected.materialHolds[0].frames, frozen);
  assert.equal(rejected.materialHolds[0].frameCount, frozen.length);
  assert.equal(rejected.materialHolds[0].omittedFrames, 0);
  assert.equal(rejected.excessiveJumps.length, 1);

  const fastButContinuous = Array.from({ length: 12 }, (_, index) =>
    sample(index * 20, index * 14, index * 14, 700)
  );
  assert.deepEqual(
    remoteActorMotionSmoothnessEvidence(fastButContinuous),
    {
      ok: true,
      sampleCount: 12,
      maximumDisplayStep: 14,
      maximumAuthorityStep: 14,
      stationaryTimelineFrames: 0,
      stationaryTimelineWitnesses: [],
      presentationMotionFrames: 0,
      presentationMotionWitnesses: [],
      materialHolds: [],
      excessiveJumps: [],
      presentationSnaps: [],
      renderFrameGaps: [],
    },
  );

  const boundedRecovery = [
    {
      ...sample(0, 0, 0, 200),
      combatViewTimeline: { generation: 7, presentationRenderTick: 100 },
    },
    {
      ...sample(19, 4, 16, 200),
      combatViewTimeline: { generation: 7, presentationRenderTick: 105 },
    },
  ];
  assert.equal(
    remoteActorMotionSmoothnessEvidence(boundedRecovery).ok,
    true,
    "several genuinely drawn bounded-recovery frames between WebDriver samples are continuous motion",
  );

  const hiddenDistanceSnap = [
    {
      ...sample(0, 100, 100, 200),
      lastSnapReason: 0,
      combatViewTimeline: { generation: 9, presentationRenderTick: 200 },
    },
    {
      ...sample(20, 104, 620, 200),
      snapCount: 1,
      lastSnapReason: 2,
      combatViewTimeline: { generation: 9, presentationRenderTick: 202 },
    },
  ];
  const snapped = remoteActorMotionSmoothnessEvidence(hiddenDistanceSnap);
  assert.equal(
    snapped.ok,
    false,
    "a same-life presentation snap must not disappear behind a segment boundary",
  );
  assert.deepEqual(snapped.presentationSnaps.map((entry) => ({
    at: entry.at,
    snapDelta: entry.snapDelta,
    snapReason: entry.snapReason,
    displayStep: entry.displayStep,
    authorityStep: entry.authorityStep,
  })), [{
    at: 20,
    snapDelta: 1,
    snapReason: 2,
    displayStep: 520,
    authorityStep: 4,
  }]);

  const hiddenTimelineGenerationJump = [
    {
      ...sample(0, 100, 100, 200),
      combatViewTimeline: { generation: 9, presentationRenderTick: 200 },
    },
    {
      ...sample(20, 104, 150, 200),
      combatViewTimeline: { generation: 10, presentationRenderTick: 202 },
    },
  ];
  assert.equal(
    remoteActorMotionSmoothnessEvidence(hiddenTimelineGenerationJump).ok,
    false,
    "an observer schedule generation change must not exempt a visible actor jump",
  );

  const hiddenRenderStall = [
    sample(0, 0, 0, 200),
    sample(120, 24, 24, 200),
  ];
  const stalledRender = remoteActorMotionSmoothnessEvidence(hiddenRenderStall);
  assert.equal(
    stalledRender.ok,
    false,
    "a missing material browser frame interval is a visible freeze even when recovery lands correctly",
  );
  assert.deepEqual(stalledRender.renderFrameGaps.map((entry) => ({
    durationMs: entry.durationMs,
    displayStep: entry.displayStep,
    authorityStep: entry.authorityStep,
  })), [{ durationMs: 120, displayStep: 24, authorityStep: 24 }]);
  assert.equal(
    remoteActorMotionSmoothnessEvidence(hiddenRenderStall, {
      rejectRenderFrameGaps: false,
    }).ok,
    true,
    "instrumented milestone reads may retain a gap without misclassifying actor interpolation",
  );

  for (const durationMs of [70, 120]) {
    const delayedSnap = [sample(0, 0, 0, 100), sample(durationMs, 7, 500, 100)];
    delayedSnap[1].snapCount = delayedSnap[0].snapCount + 1;
    const evidence = remoteActorMotionSmoothnessEvidence(delayedSnap);
    assert.equal(evidence.ok, false, "a delayed frame cannot erase a recorded same-life snap");
    assert.equal(evidence.presentationSnaps.length, 1);
  }
  const delayedJump = [sample(0, 0, 0, 100), sample(70, 7, 500, 100)];
  assert.equal(
    remoteActorMotionSmoothnessEvidence(delayedJump).excessiveJumps.length, 1,
    "a 50–80 ms interval must not fall between jump and frame-gap assertions",
  );
});

test("material-hold failure retains bounded frame context without changing rejection", () => {
  const frozen = Array.from({ length: 200 }, (_, index) => ({
    at: index * 16, lifeSeq: 1, snapCount: 0, initialCount: 1,
    positionX: index * 4, positionY: 100, displayX: 0, displayY: 100,
    velocityX: 250, velocityY: 0,
  }));
  const evidence = remoteActorMotionSmoothnessEvidence(frozen);
  assert.equal(evidence.ok, false);
  const hold = evidence.materialHolds[0];
  assert.equal(hold.frames.length, 64);
  assert.equal(hold.frameCount, 200);
  assert.equal(hold.omittedFrames, 136);
  assert.deepEqual(hold.frames, frozen.slice(0, 64));
});

test("motion jump evidence uses the observed source interval, not a later wall-bounce velocity", () => {
  // Independent piecewise-linear path: a fast Repel ends at a wall, then the
  // actor bounces back more slowly. The observer is six ticks behind authority.
  // Latest-authority velocity is therefore wrong for several drawn frames.
  const path = (tick) => tick <= 1100 ? (tick - 1080) * 7 : 140 - (tick - 1100) * 2;
  const samples = Array.from({ length: 31 }, (_, index) => {
    const tick = 1080 + index * 2;
    return {
      at: index * 20, lifeSeq: 1, snapCount: 0, initialCount: 0,
      positionTick: tick, positionSnapshotId: 300 + index,
      positionX: path(tick), positionY: 0,
      displayX: path(tick - 6), displayY: 0,
      velocityX: tick <= 1100 ? 700 : -200, velocityY: 0,
      combatViewTimeline: {
        generation: 1, scheduledRenderTick: tick - 6,
        presentationRenderTick: tick - 6, holdOrStall: false,
      },
    };
  });
  const normal = remoteActorMotionSmoothnessEvidence(samples);
  assert.equal(normal.ok, true, "pre-bounce rendering must use pre-bounce movement evidence");
  assert.ok(normal.presentationMotionFrames > 0);
  assert.ok(normal.presentationMotionWitnesses.some((witness) =>
    witness.renderTick === 1098 && witness.maximumSpeed === 700));

  const teleported = structuredClone(samples);
  teleported.find((sample) => sample.positionTick === 1104).displayX += 80;
  assert.equal(remoteActorMotionSmoothnessEvidence(teleported).ok, false,
    "a real jump during Repel remains a failure without changing thresholds");

  for (const field of ["positionTick", "positionSnapshotId", "velocityX"]) {
    const missing = samples.map((sample) => {
      const copy = { ...sample };
      delete copy[field];
      return copy;
    });
    assert.equal(remoteActorMotionSmoothnessEvidence(missing).ok, false,
      `missing ${field} cannot grant a historical-speed allowance`);
  }
  const gaps = samples.map((sample) => ({ ...sample, positionSnapshotId: sample.positionSnapshotId * 2 }));
  assert.equal(remoteActorMotionSmoothnessEvidence(gaps).ok, false,
    "unobserved source snapshots cannot establish a complete source interval");
  for (const patch of [{ holdOrStall: true }, { generation: null }, { scheduledRenderTick: null }]) {
    const unknown = samples.map((sample) => ({
      ...sample, combatViewTimeline: { ...sample.combatViewTimeline, ...patch },
    }));
    assert.equal(remoteActorMotionSmoothnessEvidence(unknown).ok, false,
      "unknown or stalled timing cannot borrow old fast movement");
  }
  const wrapped = samples.map((sample) => ({
    ...sample,
    positionTick: (sample.positionTick - 1100) >>> 0,
    positionSnapshotId: (sample.positionSnapshotId - 310) & 0xffff,
    combatViewTimeline: {
      ...sample.combatViewTimeline,
      scheduledRenderTick: (sample.combatViewTimeline.scheduledRenderTick - 1100) >>> 0,
      presentationRenderTick: (sample.combatViewTimeline.presentationRenderTick - 1100) >>> 0,
    },
  }));
  assert.equal(remoteActorMotionSmoothnessEvidence(wrapped).ok, true);
});

test("newer fast movement cannot excuse a jump in the older slow presentation interval", () => {
  const path = (tick) => tick <= 1100 ? (tick - 1080) * 2 : 40 + (tick - 1100) * 7;
  const samples = Array.from({ length: 31 }, (_, index) => {
    const tick = 1080 + index * 2;
    return {
      at: index * 20, lifeSeq: 1, snapCount: 0, initialCount: 0,
      positionTick: tick, positionSnapshotId: 300 + index,
      positionX: path(tick), positionY: 0,
      displayX: path(tick - 6), displayY: 0,
      velocityX: tick <= 1100 ? 200 : 700, velocityY: 0,
      combatViewTimeline: {
        generation: 1, scheduledRenderTick: tick - 6,
        presentationRenderTick: tick - 6, holdOrStall: false,
      },
    };
  });
  assert.equal(remoteActorMotionSmoothnessEvidence(samples).ok, true);
  samples.find((sample) => sample.positionTick === 1104).displayX += 20;
  const corrupt = remoteActorMotionSmoothnessEvidence(samples);
  assert.equal(corrupt.ok, false, "current fast authority must not mask a past slow-world jump");
  assert.equal(corrupt.excessiveJumps[0].sourceMotion.maximumSpeed, 200);
});

test("motion hold evidence compares a stopped actor with its presentation-time source", () => {
  // Independent stop/restart path: the server is stationary through tick 1170.
  // The observer plays seven 10ms ticks behind it. A legitimate restart can
  // therefore retain the stopped sprite for 80ms after newer state first moves.
  // This pins the Chrome/Extreme wall-stop false positive without
  // changing the 80ms hold threshold or allowing a frozen presentation cursor.
  const path = (tick) => Math.max(0, tick - 1170) * 2;
  const samples = Array.from({ length: 51 }, (_, index) => {
    const tick = 1100 + index * 2;
    return {
      at: index * 20, lifeSeq: 1, snapCount: 0, initialCount: 0,
      positionTick: tick, positionSnapshotId: 300 + index,
      positionX: path(tick), positionY: 0,
      displayX: path(tick - 7), displayY: 0,
      velocityX: 200, velocityY: 0,
      combatViewTimeline: {
        generation: 1, scheduledRenderTick: tick - 7,
        presentationRenderTick: tick - 7, holdOrStall: false,
      },
    };
  });
  const normal = remoteActorMotionSmoothnessEvidence(samples);
  assert.equal(normal.ok, true, "the authored wall stop must play at the observer tick");
  assert.ok(normal.stationaryTimelineFrames > 0, "retain why current-time movement was not a render freeze");
  const frozen = structuredClone(samples);
  for (const sample of frozen) {
    if (sample.positionTick <= 1190) sample.displayX = 0;
  }
  assert.equal(remoteActorMotionSmoothnessEvidence(frozen).ok, false,
    "holding after the observer reaches moving source samples remains a failure");
  for (const field of ["positionTick", "positionSnapshotId"]) {
    const missing = samples.map((sample) => {
      const copy = structuredClone(sample);
      delete copy[field];
      return copy;
    });
    assert.equal(remoteActorMotionSmoothnessEvidence(missing).ok, false,
      `unknown ${field} cannot explain away a hold`);
  }
  const gaps = samples.map((sample) => ({ ...sample, positionSnapshotId: sample.positionSnapshotId * 2 }));
  assert.equal(remoteActorMotionSmoothnessEvidence(gaps).ok, false,
    "missing source snapshots cannot prove a stationary interpolation segment");
  for (const patch of [{ holdOrStall: true }, { generation: null }, { scheduledRenderTick: null }]) {
    const unknownClock = samples.map((sample) => ({
      ...sample, combatViewTimeline: { ...sample.combatViewTimeline, ...patch },
    }));
    assert.equal(remoteActorMotionSmoothnessEvidence(unknownClock).ok, false,
      "unknown or stalled observer timing cannot explain away a hold");
  }
  const stalledClock = structuredClone(frozen);
  for (const sample of stalledClock) {
    if (sample.positionTick >= 1170) sample.combatViewTimeline.presentationRenderTick = 1163;
  }
  assert.equal(remoteActorMotionSmoothnessEvidence(stalledClock).ok, false,
    "a held old cursor cannot borrow the historical stop indefinitely");
  const wrapped = samples.map((sample) => ({
    ...sample,
    positionTick: (sample.positionTick - 1170) >>> 0,
    positionSnapshotId: (sample.positionSnapshotId - 335) & 0xffff,
    combatViewTimeline: {
      ...sample.combatViewTimeline,
      scheduledRenderTick: (sample.combatViewTimeline.scheduledRenderTick - 1170) >>> 0,
      presentationRenderTick: (sample.combatViewTimeline.presentationRenderTick - 1170) >>> 0,
    },
  }));
  assert.equal(remoteActorMotionSmoothnessEvidence(wrapped).ok, true,
    "source/render tick and snapshot-id wrap preserve the same stopped path");
});

test("aged milestone motion unwraps short source ticks against observed snapshot time", () => {
  const fixture = JSON.parse(fs.readFileSync(new URL(
    "./fixtures/aged_actor_stationary.json", import.meta.url), "utf8"));
  const metricsTrace = fixture.rows.map(([
    at, sourceTick, snapshotId, positionX, positionY, displayX, displayY,
    velocityX, velocityY, appliedTick, renderTick,
  ]) => ({
    at,
    combatViewTimeline: { generation: 1, gameAppliedServerTick: appliedTick,
      presentationRenderTick: renderTick, scheduledRenderTick: renderTick, holdOrStall: false },
    remotePlayers: [{ id: 768, lifeSeq: 4, lastServerPositionTick: sourceTick,
      positionX, positionY, displayX, displayY, velocityX, velocityY }],
    remoteActorPresentation: [{ id: 768, lifeSeq: 4, snapCount: 0,
      initialCount: 1, lastSnapReason: 1, lastSeenSnapshotId: snapshotId }],
  }));
  const evaluate = frames => remoteActorMotionAcrossStatesEvidence([{ metricsTrace: frames }], [768]);
  const normal = evaluate(metricsTrace);
  assert.equal(normal.ok, true,
    "the retained 85ms hold draws an observed stationary interval, not current authority");
  assert.ok(normal.bots[0].lives[0].smoothness.stationaryTimelineFrames > 0);

  // Preserve the same geometry at later u16 epochs and across the full u32
  // seam. The observed applied tick is the anchor; elapsed wall time is not.
  for (const shift of [65536, 131072, 0xffff_ffe0 - metricsTrace[0].combatViewTimeline.gameAppliedServerTick]) {
    const shifted = structuredClone(metricsTrace);
    for (const frame of shifted) {
      const timeline = frame.combatViewTimeline;
      timeline.gameAppliedServerTick = (timeline.gameAppliedServerTick + shift) >>> 0;
      timeline.presentationRenderTick = (timeline.presentationRenderTick + shift) >>> 0;
      timeline.scheduledRenderTick = timeline.presentationRenderTick;
      frame.remotePlayers[0].lastServerPositionTick = timeline.gameAppliedServerTick & 0xffff;
    }
    assert.equal(evaluate(shifted).ok, true, `source/render phase ${shift}`);
  }
  const frozen = structuredClone(metricsTrace);
  for (const frame of frozen) Object.assign(frame.remotePlayers[0], { displayX: 8126, displayY: 7999 });
  assert.equal(evaluate(frozen).ok, false, "a hold after the source resumes moving still fails");
  const jump = structuredClone(metricsTrace);
  jump.at(-2).remotePlayers[0].displayX += 100;
  assert.equal(evaluate(jump).ok, false, "tick expansion cannot hide a genuine jump");
  for (const change of [
    frame => { delete frame.combatViewTimeline.gameAppliedServerTick; },
    frame => { frame.remotePlayers[0].lastServerPositionTick =
      (frame.combatViewTimeline.gameAppliedServerTick + 0x8000) & 0xffff; },
    frame => { frame.remotePlayers[0].lastServerPositionTick =
      (frame.combatViewTimeline.gameAppliedServerTick + 1) & 0xffff; },
    frame => { delete frame.remoteActorPresentation[0].lastSeenSnapshotId; },
  ]) {
    const unknown = structuredClone(metricsTrace);
    unknown.forEach(change);
    assert.equal(evaluate(unknown).ok, false,
      "missing, future, ambiguous or incomplete source evidence cannot excuse a hold");
  }
});

test("compact presentation retains the existing monotonic clock and frame identity", () => {
  const result = compactBotPresentationState({ metricsTrace: [{
    at: 10_000, monotonicMs: 125.25, renderOrdinal: 9,
  }] }, [768]);
  assert.equal(result.metricsTrace[0].monotonicMs, 125.25);
  assert.equal(result.metricsTrace[0].renderOrdinal, 9);
});

test("milestone motion uses monotonic time across a backwards wall-clock step", () => {
  const frames = Array.from({ length: 30 }, (_, index) => ({
    at: 10_000 + index * 20 - (index >= 15 ? 300 : 0),
    monotonicMs: 1_000 + index * 20,
    renderOrdinal: index + 1,
    remotePlayers: [{ id: 768, lifeSeq: 1, positionX: index * 4,
      positionY: 0, displayX: index * 4, displayY: 0,
      velocityX: 200, velocityY: 0 }],
  }));
  const evaluate = rows => remoteActorMotionAcrossStatesEvidence([
    { metricsTrace: rows.slice(0, 24) },
    { metricsTrace: rows.slice(10) },
  ], [768]);
  const result = evaluate(frames);
  assert.equal(result.ok, true, "a wall-clock adjustment cannot reorder rendered frames");
  assert.equal(result.bots[0].sampleCount, 30, "overlapping rings retain every unique frame");
  const jump = structuredClone(frames);
  jump[18].remotePlayers[0].displayX += 100;
  assert.equal(evaluate(jump).ok, false, "a genuine jump still fails with a stepped wall clock");
  const freeze = structuredClone(frames);
  for (let index = 15; index < 25; index++) freeze[index].remotePlayers[0].displayX = 56;
  assert.equal(evaluate(freeze).ok, false, "a genuine hold and catch-up still fail");
});

test("milestone monotonic evidence rejects missing, conflicting and backwards identities", () => {
  const frame = (ordinal, monotonicMs) => ({
    at: 10_000, renderOrdinal: ordinal, monotonicMs,
    remotePlayers: [{ id: 768, lifeSeq: 1, positionX: ordinal,
      positionY: 0, displayX: ordinal, displayY: 0, velocityX: 50, velocityY: 0 }],
  });
  const evaluate = frames => remoteActorMotionAcrossStatesEvidence([{ metricsTrace: frames }], [768]);
  const valid = [frame(1, 100), frame(2, 120)];
  assert.equal(evaluate(valid).bots[0].sampleCount, 2,
    "distinct rendered frames cannot be erased by equal wall timestamps");
  assert.equal(evaluate([...valid, structuredClone(valid[0])]).bots[0].sampleCount, 2);
  const incomplete = structuredClone(valid);
  delete incomplete[1].monotonicMs;
  assert.throws(() => evaluate(incomplete), /incomplete monotonic/);
  const conflicting = structuredClone(valid[0]);
  conflicting.remotePlayers[0].displayX += 50;
  assert.throws(() => evaluate([...valid, conflicting]), /conflicting presentation/);
  assert.throws(() => evaluate([frame(1, 120), frame(2, 100)]), /clock moved backwards/);
});

test("milestone presentation rings retain rare continuous same-life snaps", () => {
  const frame = (at, positionX, displayX, snapCount = 0, lastSnapReason = 0) => ({
    at,
    combatViewTimeline: { generation: at < 1_000 ? 1 : 2, presentationRenderTick: at / 10 },
    remoteActorPresentation: [{
      id: 768,
      lifeSeq: 4,
      snapCount,
      initialCount: 1,
      lastSnapReason,
    }],
    remotePlayers: [{
      id: 768,
      lifeSeq: 4,
      positionX,
      positionY: 500,
      displayX,
      displayY: 500,
      velocityX: 200,
      velocityY: 0,
    }],
  });
  const evidence = remoteActorMotionAcrossStatesEvidence([
    { metricsTrace: [frame(0, 0, 0), frame(20, 4, 4)] },
    // A retained-ring gap is not visible evidence by itself.
    { metricsTrace: [
      frame(2_000, 400, 400),
      frame(2_020, 404, 404),
      frame(2_040, 408, 900, 1, 2),
    ] },
  ], [768]);
  assert.equal(evidence.ok, false);
  assert.equal(evidence.capturedStateCount, 2);
  assert.equal(evidence.bots[0].failedLives.length, 1);
  assert.equal(
    evidence.bots[0].failedLives[0].smoothness.presentationSnaps.length,
    1,
  );
});

test("stationary bot evidence requires activation and sustained exact-life zero motion", () => {
  const activation = "INFO server bot stationary E2E geometry fixture active server_tick=100 bot_id=768 life_seq=2 x=8000 y=8100";
  const sample = (tick, x = 8000, speed = 0, passiveFire = false) =>
    `INFO server bot movement trace server_tick=${tick} bot_id=768 life_seq=2 x=${x} y=8100 velocity_x=0 velocity_y=0 speed=${speed} bot_in_safe_zone=false e2e_stationary_motion=true e2e_passive_fire=${passiveFire}`;
  assert.deepEqual(stationaryBotMotionEvidence([
    `${activation}\n${sample(101)}\n${sample(102)}\n`,
  ], {
    botId: 768,
    requiredLifeSeqs: [2],
  }), {
    botId: 768,
    lives: [{
      botId: 768,
      lifeSeq: 2,
      activation: { serverTick: 100, botId: 768, lifeSeq: 2, x: 8000, y: 8100 },
      sampleCount: 2,
      firstServerTick: 101,
      lastServerTick: 102,
      x: 8000,
      y: 8100,
      observedMaximumSpeed: 0,
      observedMaximumDisplacement: 0,
      passiveFireRequired: false,
      passiveFireSamples: 0,
    }],
  });
  assert.throws(() => stationaryBotMotionEvidence([
    `${activation}\n${sample(101)}\n${sample(102, 8001, 1)}\n`,
  ], {
    botId: 768,
    requiredLifeSeqs: [2],
  }), /violated exact motion\/passive evidence/);
  const passive = stationaryBotMotionEvidence([
    `${activation}\n${sample(101, 8000, 0, true)}\n${sample(102, 8000, 0, true)}\n`,
  ], {
    botId: 768,
    requiredLifeSeqs: [2],
    requiredPassiveFireLifeSeqs: [2],
  });
  assert.equal(passive.lives[0].passiveFireRequired, true);
  assert.equal(passive.lives[0].passiveFireSamples, 2);
  assert.throws(() => stationaryBotMotionEvidence([
    `${activation}\n${sample(101, 8000, 0, true)}\n${sample(102)}\n`,
  ], {
    botId: 768,
    requiredLifeSeqs: [2],
    requiredPassiveFireLifeSeqs: [2],
  }), /violated exact motion\/passive evidence/);
});

import {
  nearestAuthoredSafetyEdge,
  lureTargetOutOfSafety,
} from "./bot_combat_evidence.mjs";

const SAFE_TILE = 171;
const LURE_TARGET_ID = 768;

function lureTestSafety() {
  const authoredTiles = new Uint8Array(1_024 * 1_024);
  // A 5x5 authored safe block at tiles (100..104, 100..104); everything else is open space.
  for (let tileY = 100; tileY <= 104; tileY += 1) {
    for (let tileX = 100; tileX <= 104; tileX += 1) {
      authoredTiles[tileY * 1_024 + tileX] = SAFE_TILE;
    }
  }
  const tileAt = (x, y) => authoredTiles[Math.floor(y / 16) * 1_024 + Math.floor(x / 16)];
  return {
    authoredTiles,
    positionIsOnAuthoredSafeTile: (x, y) => tileAt(x, y) === SAFE_TILE,
    authoredTileIsSolid: () => false,
    authoredHullIsOpen: () => true,
    authoredLineIsClear: () => true,
  };
}

function lureTestClient(initialMetrics) {
  const state = { ...initialMetrics, remotePlayers: [...initialMetrics.remotePlayers] };
  return {
    metrics: state,
    client: { metricsState: async () => ({ uiMetrics: state }) },
  };
}

test("the safety lure enters the nearest open authored boundary, not a blocked one", () => {
  const safety = lureTestSafety();
  // Pilot inside the block, west of centre: the nearest boundary is the west edge.
  const west = nearestAuthoredSafetyEdge(safety, { playerX: 1_608, playerY: 1_608 });
  assert.deepEqual(west.inside, { x: 1_608, y: 1_608 });
  assert.deepEqual(west.outside, { x: 1_592, y: 1_608 });

  // A solid tile outside the boundary is not an entry the pilot can be driven through.
  safety.authoredTiles[100 * 1_024 + 99] = 200;
  safety.authoredTileIsSolid = (id) => id === 200;
  const blocked = nearestAuthoredSafetyEdge(safety, { playerX: 1_608, playerY: 1_608 });
  assert.notDeepEqual(blocked.outside, { x: 1_592, y: 1_608 });
});

test("a target already standing off authored safety is staged without moving the pilot", async () => {
  const safety = lureTestSafety();
  const { metrics, client } = lureTestClient({
    playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 4,
    playerX: 1_608, playerY: 1_608, inSafeZone: true,
    remotePlayers: [{
      id: LURE_TARGET_ID, lifeSeq: 4, positionX: 2_400, positionY: 1_608,
    }],
  });
  const driven = [];
  const lured = await lureTargetOutOfSafety({
    safety,
    actor: client,
    preferredTargetId: LURE_TARGET_ID,
    preferredTargetLifeSeq: 4,
    label: "test",
    drive: async (_actor, point) => { driven.push(point); return { reached: true }; },
    leaveSafety: async () => true,
  });
  assert.equal(lured, true);
  assert.deepEqual(driven, [], "a target outside safety needs no bait flight");
  assert.equal(metrics.inSafeZone, true, "the pilot must stay in its authored reserve");
});

test("the lure baits the target off safety, retreats into the nearest entry, and reports it", async () => {
  const safety = lureTestSafety();
  const { metrics, client } = lureTestClient({
    playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 4,
    playerX: 1_608, playerY: 1_608, inSafeZone: true,
    remotePlayers: [{
      id: LURE_TARGET_ID, lifeSeq: 4, positionX: 1_616, positionY: 1_608,
    }],
  });
  const driven = [];
  const lured = await lureTargetOutOfSafety({
    safety,
    actor: client,
    preferredTargetId: LURE_TARGET_ID,
    preferredTargetLifeSeq: 4,
    label: "test",
    drive: async (_actor, point) => {
      driven.push({ x: point.x, y: point.y });
      metrics.playerX = point.x;
      metrics.playerY = point.y;
      metrics.inSafeZone = safety.positionIsOnAuthoredSafeTile(point.x, point.y);
      // The hostile follows the pilot out the moment the pilot leaves the block.
      if (!metrics.inSafeZone) {
        metrics.remotePlayers[0].positionX = 2_400;
        metrics.remotePlayers[0].positionY = 1_608;
      }
      return { reached: true };
    },
    leaveSafety: async (actor, drive) => {
      await drive(null, { x: 1_708, y: 1_608 });
      return true;
    },
  });
  assert.equal(lured, true);
  assert(metrics.inSafeZone === true, "staging must start back inside authored safety");
  assert.deepEqual(
    driven.at(-1),
    { x: 1_672, y: 1_608 },
    "the retreat must end at the nearest authored entry, not the map-centre reserve",
  );
});

test("a target that never leaves authored safety reports the lure failed", async () => {
  const safety = lureTestSafety();
  const { metrics, client } = lureTestClient({
    playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 4,
    playerX: 1_608, playerY: 1_608, inSafeZone: true,
    remotePlayers: [{
      id: LURE_TARGET_ID, lifeSeq: 4, positionX: 1_616, positionY: 1_608,
    }],
  });
  const started = Date.now();
  const lured = await lureTargetOutOfSafety({
    safety,
    actor: client,
    preferredTargetId: LURE_TARGET_ID,
    preferredTargetLifeSeq: 4,
    label: "test",
    attempts: 2,
    followWindowMs: 250,
    drive: async (_actor, point) => {
      metrics.playerX = point.x;
      metrics.playerY = point.y;
      metrics.inSafeZone = false;
      return { reached: true };
    },
    leaveSafety: async (actor, drive) => (await drive(null, { x: 1_708, y: 1_608 })).reached,
  });
  assert.equal(lured, false);
  assert(metrics.remotePlayers[0].positionX < 1_700, "the target must stay on its safe tile");
  assert(Date.now() - started < 5_000, "an unfollowed bait must end within its windows");
});


test("primary-bomb retry does not credit shrapnel or a different retired life", () => {
  const fragment={damageSeq:79,projectileId:140,shooterId:0,victimId:768,victimLifeSeq:2,weapon:47,energyAfter:0,lethal:true};
  const later={damageSeq:303,projectileId:453,shooterId:0,victimId:768,victimLifeSeq:3,weapon:8612,energyAfter:0,lethal:true};
  assert.equal(exactLifePrimaryBombTerminal([fragment,later],0,{victimId:768,victimLifeSeq:2}).kind,"retired");
  assert.equal(exactLifePrimaryBombTerminal([later],0,{victimId:768,victimLifeSeq:2}).kind,"pending");
  assert.equal(exactLifePrimaryBombTerminal([later],0,{victimId:768,victimLifeSeq:3}).kind,"primary-bomb");
  assert.equal(exactLifePrimaryBombTerminal([{...later,shooterId:769}],0,{victimId:768,victimLifeSeq:3}).kind,"retired");
});

test("retired bomb target retries bind an applied live successor and fresh score/log baseline", async () => {
  const server={logs:[damageLine(79).replace('"victim_life_seq":1','"victim_life_seq":2').replace('"weapon":3','"weapon":47')]};
  const metrics={selfWins:2,playerVelocityX:0,playerVelocityY:0,remotePlayers:[{id:768,lifeSeq:3,energy:1700}],authoritativeSnapshotActors:[{id:768,lifeSeq:3}]};
  const actor={metricsState:async()=>({uiMetrics:metrics})};
  const retry=await renewRetiredBombTarget({actor,server,playerId:0,target:{victimId:768,victimLifeSeq:2},damageBoundary:0,label:"unit exact-life"});
  assert.deepEqual(retry.target,{victimId:768,victimLifeSeq:3});
  assert.equal(retry.retirement.damageSeq,79); assert.equal(retry.retirement.weapon&31,15);
  assert.equal(retry.winsBefore,2); assert.equal(retry.damageBoundary,serverDamageBoundary(server));
  assert.equal(retry.presentationBefore,metrics);
  assert.deepEqual(serverDamageEventsSince(server,retry.damageBoundary),[]);
});

test("retired bomb target cannot bind an undrawn successor", async () => {
  const server={logs:[damageLine(79).replace('"victim_life_seq":1','"victim_life_seq":2').replace('"weapon":3','"weapon":47')]};
  const actor={metricsState:async()=>({uiMetrics:{selfWins:2,playerVelocityX:0,playerVelocityY:0,remotePlayers:[{id:768,lifeSeq:3,energy:1700}],authoritativeSnapshotActors:[{id:768,lifeSeq:2}]}})};
  await assert.rejects(renewRetiredBombTarget({actor,server,playerId:0,target:{victimId:768,victimLifeSeq:2},damageBoundary:0,label:"unit undrawn",timeoutMs:1}),/binds a presented live successor/);
});

test("retired bomb target brakes a coasting pilot before waiting for its visible successor", async () => {
  const server = { logs: [damageLine(79).replace('"victim_life_seq":1', '"victim_life_seq":2').replace('"weapon":3', '"weapon":47')] };
  let braked = false;
  const actor = { metricsState: async () => ({ uiMetrics: {
    selfWins: 2, playerVelocityX: braked ? 0 : 325, playerVelocityY: 0,
    remotePlayers: braked ? [{ id: 768, lifeSeq: 3, energy: 1700 }] : [],
    authoritativeSnapshotActors: braked ? [{ id: 768, lifeSeq: 3 }] : [],
  } }) };
  let brakeCalls = 0;
  const retry = await renewRetiredBombTarget({ actor, server, playerId: 0,
    target: { victimId: 768, victimLifeSeq: 2 }, damageBoundary: 0,
    label: "unit coasting successor", timeoutMs: 20,
    brake: async (receivedActor, budgetMs) => {
      assert.equal(receivedActor, actor);
      assert(budgetMs > 0 && budgetMs <= 20);
      brakeCalls += 1;
      braked = true;
      return { braked: true, speed: 0 };
    },
  });
  assert.equal(brakeCalls, 1);
  assert.deepEqual(retry.target, { victimId: 768, victimLifeSeq: 3 });
  assert.equal(retry.retirement.weapon & 31, 15);
  assert.equal(retry.damageBoundary, serverDamageBoundary(server));
  assert.equal(retry.coastBrake.braked, true);
});

test("a failed coast brake cannot bind even a visible live successor", async () => {
  const server = { logs: [damageLine(79).replace('"victim_life_seq":1', '"victim_life_seq":2').replace('"weapon":3', '"weapon":47')] };
  let reads = 0;
  const actor = { metricsState: async () => {
    reads += 1;
    return { uiMetrics: { selfWins: 2, remotePlayers: [{ id: 768, lifeSeq: 3, energy: 1700 }],
      authoritativeSnapshotActors: [{ id: 768, lifeSeq: 3 }] } };
  } };
  await assert.rejects(renewRetiredBombTarget({ actor, server, playerId: 0,
    target: { victimId: 768, victimLifeSeq: 2 }, damageBoundary: 0, label: "unit failed brake",
    brake: async () => ({ braked: false, reason: "unavailable input" }),
  }), /could not brake/);
  assert.equal(reads, 0);
});

test("a current primary-bomb terminal does not trigger successor controls or reset baselines", async () => {
  const server = { logs: [damageLine(79).replace('"victim_life_seq":1', '"victim_life_seq":2')] };
  let brakeCalls = 0;
  const retry = await renewRetiredBombTarget({ actor: {}, server, playerId: 0,
    target: { victimId: 768, victimLifeSeq: 2 }, damageBoundary: 0, label: "unit primary terminal",
    brake: async () => { brakeCalls += 1; throw new Error("must not touch controls"); },
  });
  assert.equal(retry, null);
  assert.equal(brakeCalls, 0);
});


test("weapon contacts preserve the reference game's ship-first inclusive/exclusive pixel edges", () => {
  for (const shipRadius of [14, 20]) {
    for (const [offset, expected] of [[shipRadius + 4, true], [shipRadius + 5, false], [-shipRadius - 4, false], [-shipRadius - 3, true]]) {
      for (const [weaponX, weaponY] of [[1000 + offset, 1000], [1000, 1000 + offset]]) {
        assert.equal(explosiveContactOverlapsHull({weaponX, weaponY, shipX: 1000, shipY: 1000, shipRadius}), expected);
      }
    }
  }
});


test("browser contact oracle matches actual retained reference-game outputs", () => {
  const fixture = fs.readFileSync(new URL('../fixtures/reference_game_weapon_contacts.csv', import.meta.url), 'utf8');
  const rows = fixture.trim().split('\n').filter(line => !line.startsWith('#'));
  assert.equal(rows.length, 176);
  for (const line of rows) {
    const [shipX,shipY,shipRadius,weaponX,weaponY,,direct] = line.split(',').map(Number);
    assert.equal(explosiveContactOverlapsHull({weaponX,weaponY,shipX,shipY,shipRadius}), Boolean(direct), line);
  }
});

test("milestone gap Flash requires the exact authoritative door relocation", () => {
  // Exact counter/tick/time bounds from retained Firefox lane18 RED. The
  // actual warp frame was outside both bounded browser rings.
  const frame = (at, tick, snapshot, x, y, displayX, displayY, snapCount) => ({
    at,
    combatViewTimeline: { generation: 1, gameAppliedServerTick: tick,
      presentationRenderTick: tick - 5 },
    remoteActorPresentation: [{ id: 769, lifeSeq: 1, snapCount,
      initialCount: 0, lastSnapReason: snapCount ? 3 : 0,
      lastSeenSnapshotId: snapshot }],
    remotePlayers: [{ id: 769, lifeSeq: 1, positionX: x, positionY: y,
      displayX, displayY, lastServerPositionTick: tick & 0xffff,
      velocityX: 156.3, velocityY: 151.9 }],
  });
  const states = [
    { metricsTrace: [frame(1000382703987, 1944, 745, 7896, 8011, 7909, 8019, 0)] },
    { metricsTrace: [frame(1000382897073, 21254, 12331, 8522, 8027, 8514.5, 8019.49, 1)] },
  ];
  const door = { playerId: 769, lifeSeq: 1, serverTick: 5040,
    destinationX: 6808, destinationY: 9000 };
  const evaluate = (events, input = states) =>
    remoteActorMotionAcrossStatesEvidence(input, [769], { doorRelocations: events });
  assert.equal(evaluate([]).ok, false, "missing server evidence must remain a RED");
  const proved = evaluate([door]);
  assert.equal(proved.ok, true);
  const smooth = proved.bots[0].lives[0].smoothness;
  assert.equal(smooth.authorizedDoorRelocations.length, 1);
  assert.equal(smooth.observedPresentationSnaps.length, 1);
  assert.equal(smooth.presentationSnaps.length, 0);
  assert.equal(smooth.renderFrameGaps[0].durationMs, 193086,
    "retain the unobserved gap; this is not continuous-motion proof");
  for (const invalid of [
    [], [door, door], [{ ...door, playerId: 768 }],
    [{ ...door, lifeSeq: 2 }], [{ ...door, serverTick: 1944 }],
    [{ ...door, serverTick: 21255 }], [{ ...door, serverTick: -1 }],
    [{ ...door, serverTick: 0x1_0000_0000 }],
    [{ ...door, destinationX: NaN }],
  ]) assert.equal(evaluate(invalid).ok, false, JSON.stringify(invalid));
  for (const change of [
    frame => { frame.remoteActorPresentation[0].snapCount = 2; },
    frame => { frame.remoteActorPresentation[0].lastSnapReason = 2; },
    frame => { delete frame.remoteActorPresentation[0].lastSeenSnapshotId; },
    frame => { delete frame.remoteActorPresentation[0].initialCount; },
    frame => { frame.combatViewTimeline.generation = 2; },
    frame => { delete frame.combatViewTimeline.gameAppliedServerTick; },
    frame => { frame.remotePlayers[0].lastServerPositionTick = 0xffff; },
  ]) {
    const input = structuredClone(states);change(input[1].metricsTrace[0]);
    assert.equal(evaluate([door], input).ok, false, "ambiguous counter/source evidence fails closed");
  }
  const continuous = structuredClone(states);
  continuous[1].metricsTrace[0].at = continuous[0].metricsTrace[0].at + 20;
  assert.equal(evaluate([door], continuous).ok, false,
    "a missing-interval explanation cannot excuse a continuously observed snap");
});

test("door relocation evidence accepts only committed typed tracing fields", () => {
  const text = '2000-01-01T04:05:34.913715Z  INFO event="door_overlap_bot_relocated" player_id=769 life_seq=1 server_tick=5040 destination_x=6808.0 destination_y=9000.0';
  const fields = { event: "door_overlap_bot_relocated", player_id: 769,
    life_seq: 1, server_tick: 5040, destination_x: 6808, destination_y: 9000 };
  const envelope = data => JSON.stringify({ level: "INFO", fields: data });
  const expected = [{ playerId: 769, lifeSeq: 1, serverTick: 5040,
    destinationX: 6808, destinationY: 9000 }];
  assert.deepEqual(botDoorRelocationEvidence(text), expected);
  assert.deepEqual(botDoorRelocationEvidence([text.slice(0, 80), text.slice(80)]), expected);
  assert.deepEqual(botDoorRelocationEvidence(envelope(fields)), expected);
  assert.equal(botDoorRelocationEvidence(`${text}\n${text}`).length, 2,
    "duplicate evidence cannot silently become one unique transaction");
  for (const invalid of [
    `INFO message=${JSON.stringify(text)}`,
    envelope({ message: text }),
    envelope({ ...fields, event: "door_overlap_bot_destination_unavailable" }),
    envelope({ ...fields, player_id: "769" }),
    envelope({ ...fields, server_tick: 0x1_0000_0000 }),
    envelope({ ...fields, life_seq: 0 }),
    envelope({ ...fields, destination_x: "NaN" }),
    text.replace('player_id=769', 'player_id=769 player_id=768'),
  ]) assert.deepEqual(botDoorRelocationEvidence(invalid), [], invalid);
});

// Exercise both actual scenario assertions without starting their browser mains.
for (const [file, name] of [
  ["bot_authority.mjs", "assertBotProjectileOwnersVisible"],
  ["bot_authority_firefox.mjs", "assertFirefoxBotProjectileOwnersVisible"],
]) {
  const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
  const start = source.indexOf(`function ${name}(`);
  const end = source.indexOf("\nfunction ", start + 1);
  const check = vm.runInNewContext(`(${source.slice(start, end).trim()})`, {
    worldPointOnCenteredViewport,
  });
  const invoke = (metrics, sprites, options) => file.includes("firefox")
    ? check(metrics, sprites, [768], options)
    : check(metrics, sprites, [768], "fixture", options);
  const projectile = { playerId: 768, authenticated: true, sessionId: "7", lifeSeq: 1,
    eventId: 4, serverTick: 1120, authorityProjectileId: 2, authorityStateTick: 1120,
    shotGroupId: "2", sourceProjectileIndex: 0, kind: "bullet", x: 100, y: 200 };
  const frame = { at: 1000, latestServerTick: 1121, playerX: 100, playerY: 200,
    screenWidth: 1280, screenHeight: 720, remoteProjectileSources: [projectile],
    presentedRemoteProjectileSources: [] };
  const entry = { sourcePlayerId: 768, sourceSessionId: "7", sourceLifeSeq: 1,
    authorityProjectileId: 2, authorityStateTick: 1120, shotGroupId: "2",
    sourceProjectileIndex: 0, drawn: true };
  const sprites = [{ at: 1000, projectiles: { entries: [entry] } }];
  test(`${file}: installed pre-birth state is not a presentation claim`, () => {
    assert.equal(invoke([frame], [], { required: false }), null);
    assert.throws(() => invoke([frame], []), /no fresh bot projectile/);
  });
  test(`${file}: every prepared exact state still requires its matching draw`, () => {
    const presented = [{ ...frame, presentedRemoteProjectileSources: [projectile] }];
    assert.equal(invoke(presented, sprites).exactActiveProjectileDraws, 1);
    for (const field of ["authorityProjectileId", "authorityStateTick", "sourcePlayerId",
      "sourceSessionId", "sourceLifeSeq", "shotGroupId", "sourceProjectileIndex", "drawn"]) {
      const bad = { ...entry, [field]: field === "drawn" ? false : "wrong" };
      assert.throws(() => invoke(presented, [{ at: 1000, projectiles: { entries: [bad] } }]),
        /without a matching texture draw/, field);
    }
    assert.throws(() => invoke(presented, [{ ...sprites[0], at: 1001 }]),
      /without a matching texture draw/, "a future draw cannot excuse a currently invisible bullet");
    assert.throws(() => invoke([{ ...frame, presentedRemoteProjectileSources: undefined }], sprites),
      /prepared projectile diagnostics/, "missing instrumentation cannot silently pass");
  });
}

const deathExplosionFixture = () => {
  const death = {victimId: 768, victimNextLifeSeq: 2, greenCollectibleId: 19};
  const first = {ordinal: 1, authoritySegment: "18446744073709551615", playerId: 768,
    nextLifeSeq: 2, respawnAtTick: 3023, greenCollectibleId: 19, x: 7892, y: 8596};
  const second = {...first, ordinal: 2, playerId: 769, greenCollectibleId: 20, x: 7834, y: 8572};
  const metrics = {remoteShipExplosionCount: 2, lastRemoteShipExplosionX: second.x,
    lastRemoteShipExplosionY: second.y, remoteShipExplosionWitnesses: [first, second]};
  return {death, first, second, metrics};
};

test("death-explosion position uses the exact admitted death after another bot dies", () => {
  const {death, first, metrics} = deathExplosionFixture();
  assert(Math.hypot(metrics.lastRemoteShipExplosionX-first.x, metrics.lastRemoteShipExplosionY-first.y) > 32);
  assert.deepEqual(exactRemoteDeathExplosionWitness(metrics, death, 0), first);
  assert.equal(exactRemoteDeathExplosionWitness(metrics, {...death, victimId: 769, greenCollectibleId: 20}, 0).ordinal, 2);
});

test("death-explosion evidence rejects missing, stale, ambiguous or misidentified effects", () => {
  const {death, first, metrics} = deathExplosionFixture();
  assert.throws(() => exactRemoteDeathExplosionWitness({...metrics, remoteShipExplosionWitnesses: undefined}, death), /history/);
  assert.throws(() => exactRemoteDeathExplosionWitness({...metrics, remoteShipExplosionWitnesses: Array(65).fill(first)}, death), /history/);
  assert.equal(exactRemoteDeathExplosionWitness(metrics, death, 1), null);
  for (const change of [{playerId: 769}, {nextLifeSeq: 3}, {greenCollectibleId: 20},
    {ordinal: 3}, {x: null}, {y: NaN}, {authoritySegment: "18446744073709551616"}]) {
    assert.equal(exactRemoteDeathExplosionWitness({...metrics, remoteShipExplosionWitnesses: [{...first, ...change}]}, death), null);
  }
  assert.equal(exactRemoteDeathExplosionWitness({...metrics,
    remoteShipExplosionWitnesses: [first, {...first, ordinal: 2}]}, death), null,
    "two admitted effects for one exact death cannot be hidden by selecting the nearer one");
});

test("death-explosion ordinals remain exact through u32 wrap", () => {
  const {death, first, metrics} = deathExplosionFixture();
  const wrapped = {...first, ordinal: 0};
  assert.deepEqual(exactRemoteDeathExplosionWitness({...metrics, remoteShipExplosionCount: 0,
    remoteShipExplosionWitnesses: [wrapped]}, death, 0xffffffff), wrapped);
});

test("the actual bot-death notification conjunction accepts the exact player handle and rejects obsolete or wrong names", async () => {
  const module = await import("./bot_combat_evidence.mjs");
  const source = fs.readFileSync(new URL("./bot_combat_evidence.mjs", import.meta.url), "utf8");
  const expression = /const notificationAdvanced =\s*([\s\S]*?);/.exec(source)?.[1];
  assert(expression, "exercise the real conjunction used by the browser death loop");
  const matches = (text, playerName, count = 1) => vm.runInNewContext(`(${expression})`, {
    metrics: { combatKillNotificationCount: count, lastCombatKillNotification: text },
    presentationBefore: { combatKillNotificationCount: 0 }, playerName,
    killNotificationNamesPlayer: module.killNotificationNamesPlayer,
  });
  assert.equal(matches("Stardust(38) killed by: botactor", "botactor"), true);
  assert.equal(matches("Stardust(38) killed by: ffbotactor", "ffbotactor"), true);
  assert.equal(matches("Stardust(38) killed by: Queen!*", "Queen!*"), true);
  for (const text of ["Stardust(38) killed by: You", "Stardust(38) killed by: botactor2",
    "Stardust(38) killed by: other", "botactor(38) killed by: other", null]) {
    assert.equal(matches(text, "botactor"), false, String(text));
  }
  assert.equal(matches("Stardust(38) killed by: botactor", "botactor", 0), false,
    "the original notification increment remains mandatory");
});

test("stationary Chaos bomb staging rejects the retained wall point and keeps its76px firing radius", async () => {
  const { loadRendezvousMap } = await import("./player_rendezvous.mjs");
  const { geometry } = loadRendezvousMap("chaos-zone", [new URL("../../assets/zones", import.meta.url).pathname]);
  const player = { x: 7299.75, y: 8142.01 };
  const target = { x: 7269.8427734375, y: 8203.216796875 };
  assert(geometry.authoredHullIsTraversable(player.x, player.y, 14));
  assert(geometry.authoredHullIsTraversable(target.x, target.y, 14));
  const point = stationaryChaosBombHoldingPoint({
    fixtureActive: true, arena: "chaos-zone", weaponMode: "bomb", lineIsClear: true,
    humanDamageObserved: true, targetMotionReady: true, targetSpeed: 0,
    playerX: player.x, playerY: player.y, targetX: target.x, targetY: target.y,
    hullIsOpen: (x, y) => geometry.authoredHullIsTraversable(x, y, 14) && geometry.tileAt(x, y) !== 171,
    firingLineIsClear: geometry.authoredLineIsClear,
  });
  assert(point, "the unchanged map has a reachable alternative on the original ring");
  assert(Math.abs(Math.hypot(point.x-target.x, point.y-target.y)-76)<1e-8);
  for (const dx of [-3,0,3]) for (const dy of [-3,0,3])
    assert(geometry.authoredHullIsTraversable(point.x+dx,point.y+dy,14),
      "the entire original3px arrival tolerance needs full hull clearance");
  const distance=Math.hypot(point.x-player.x,point.y-player.y);
  for(let i=0;i<=Math.ceil(distance*4);i++) {
    const t=i/Math.max(1,Math.ceil(distance*4));
    assert(geometry.authoredHullIsTraversable(player.x+(point.x-player.x)*t,
      player.y+(point.y-player.y)*t,14),"the actual direct keyboard route must fit the hull");
  }
});


test("stationary holding-point search cannot accept blocked hulls or a blocked firing line", () => {
  const base = { fixtureActive:true,arena:"chaos-zone",weaponMode:"bomb",lineIsClear:true,
    humanDamageObserved:true,targetMotionReady:true,targetSpeed:0,
    playerX:100,playerY:200,targetX:400,targetY:200 };
  assert.equal(stationaryChaosBombHoldingPoint({...base,hullIsOpen:()=>false,firingLineIsClear:()=>true}),null);
  assert.equal(stationaryChaosBombHoldingPoint({...base,hullIsOpen:()=>true,firingLineIsClear:()=>false}),null);
  assert.throws(()=>stationaryChaosBombHoldingPoint(base),/requires authored/);
});

test('a retained gap requires one unique committed relocation for every Flash increment',()=>{
 const frame=(at,tick,snapshot,count)=>({at,combatViewTimeline:{generation:1,gameAppliedServerTick:tick,presentationRenderTick:tick-7},
  remoteActorPresentation:[{id:768,lifeSeq:2,snapCount:count,initialCount:1,lastSnapReason:3,lastSeenSnapshotId:snapshot}],
  remotePlayers:[{id:768,lifeSeq:2,positionX:8000,positionY:8000,displayX:8000,displayY:8000,
   lastServerPositionTick:tick&0xffff,velocityX:0,velocityY:0}]});
 const states=[{metricsTrace:[frame(0,100,1,0),frame(1000,200,50,2)]}];
 const events=[110,180].map(serverTick=>({playerId:768,lifeSeq:2,serverTick,destinationX:8000,destinationY:8000}));
 const evaluate=doorRelocations=>remoteActorMotionAcrossStatesEvidence(states,[768],{doorRelocations});
 assert.equal(evaluate(events).ok,true);
 assert.equal(evaluate(events.slice(1)).ok,false);
 assert.equal(evaluate([events[0],events[0]]).ok,false);
 assert.equal(evaluate([...events,{...events[0],serverTick:190}]).ok,false);
});
test('Warp-prize relocations need the same committed typed identity and destination as doors',()=>{
 const fields={event:'bot_warp_prize_relocated',player_id:769,life_seq:2,server_tick:66883,destination_x:8200,destination_y:8200};
 const expected=[{playerId:769,lifeSeq:2,serverTick:66883,destinationX:8200,destinationY:8200}];
 assert.deepEqual(botDoorRelocationEvidence(JSON.stringify({level:'INFO',fields})),expected);
 const text='2000-01-01T02:40:52Z  INFO server bot Warp prize relocation event="bot_warp_prize_relocated" player_id=769 life_seq=2 server_tick=66883 destination_x=8200.0 destination_y=8200.0 collectible_id=40 prize_id=9';
 assert.deepEqual(botDoorRelocationEvidence(text),expected);
 assert.deepEqual(botDoorRelocationEvidence(JSON.stringify({level:'INFO',fields:{message:text}})),[]);
});
