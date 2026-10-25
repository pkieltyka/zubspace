import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import { botAuthorityLoadout, botBombSafetyProfile, botBombSafetyFloor } from './bot_authority_loadout.mjs';
import { movingBombAttackPlan, movingBombControlStep, movingBombNeedsVelocityControl } from "./moving_bomb_pilot.mjs";

const bombSafetyProfiles = Object.fromEntries(['chaos-zone', 'extreme'].map(arena => [arena,
  botBombSafetyProfile(botAuthorityLoadout({}, arena), fs.readFileSync(
    new URL(`../../assets/zones/${arena}/arena.conf`, import.meta.url), 'utf8'))]));

// Exercise the actual Firefox controller, not a duplicate policy function.
// This retained failure stayed safe for six attempts with zero new bomb edges.
const source = fs.readFileSync(new URL("bot_authority_firefox.mjs", import.meta.url), "utf8");
const start = source.indexOf("async function attackBotsWithFirefoxInput(");
assert.ok(start >= 0);
const end = source.indexOf("\n}", start) + 2;
assert.ok(end > start);
const controllerSource = source.slice(start, end);
const capture = new Error("ordinary keyboard pursuit reached");

async function runSafetyController({ blocked = false, lifeSeq = 3, present = true } = {}) {
  let now = 0;
  const driven = [];
  let protectedStagingCalls = 0;
  let routeCalls = 0;
  const metrics = {
    selfPlayerId: 0, playerLifeSeq: 3, playerAlive: true, serverPlayerAlive: true,
    playerX: 8258.17, playerY: 8261.84, inSafeZone: true,
    localGameplayState: {bombLevel: 3},
    remotePlayers: present ? [{ id: 768, lifeSeq, displayX: 7882, displayY: 8161,
      positionX: 7882, positionY: 8161, velocityX: 0, velocityY: 0 }] : [],
  };
  const actor = {};
  const context = {
    Date: { now: () => (now += 100) }, Number, Math,
    arena: "extreme", botIdBase: 768, bombWeaponTypes: new Set([3, 4]),
    bulletWeaponTypes: new Set([1, 2]),
    botBombSafetyFloor, bombSafetyProfile: bombSafetyProfiles.extreme,
    firefoxMetricsState: async () => ({ uiMetrics: metrics }),
    playerWeaponDamageToBots: () => [], serverDamageEventsSince: () => [],
    exactLifePrimaryBombTerminal: () => ({ kind: "pending" }),
    stageExtremeFirefoxCombatEncounter: async () => { protectedStagingCalls++; return false; },
    sleep: async () => {},
    authoredLineIsClear: () => !blocked,
    authoredHullIsTraversable: () => true, authoredHullIsOpen: () => true,
    routedMapCounterfireWaypoint: () => { routeCalls++; return { x: 8008, y: 8248 }; },
    driveClientToWorldTarget: async (...args) => {
      driven.push({ sameActor: args[0] === actor, point: structuredClone(args[1]),
        timeoutMs: args[2], fire: args[3] });
      throw capture;
    },
  };
  const controller = vm.runInNewContext(`(${controllerSource})`, context);
  let error;
  try { await controller({}, actor, {}, 0, 0, 768, true, "bomb", 500, 3); }
  catch (caught) { error = caught; }
  return { driven, protectedStagingCalls, routeCalls, error };
}

test("Firefox's ordinary bomb fallback leaves safety when no protected edge exists", async () => {
  const result = await runSafetyController();
  assert.equal(result.error, capture, "the real attack loop must initiate keyboard pursuit");
  assert.deepEqual(result.driven, [{ sameActor: true, point: { x: 7882, y: 8161 },
    timeoutMs: 1200, fire: false }]);
  assert.equal(result.protectedStagingCalls, 0, "fallback cannot recurse into the failed edge solver");
});

test("safe departure routes around authored geometry without firing", async () => {
  const result = await runSafetyController({ blocked: true });
  assert.equal(result.error, capture);
  assert.equal(result.routeCalls, 1);
  assert.deepEqual(result.driven[0].point, { x: 8008, y: 8248 });
  assert.equal(result.driven[0].fire, false);
});

for (const options of [{ lifeSeq: 4 }, { present: false }]) {
  test(`safe departure cannot pursue a missing/wrong-life target: ${JSON.stringify(options)}`, async () => {
    const result = await runSafetyController(options);
    assert.equal(result.error, undefined);
    assert.deepEqual(result.driven, []);
    assert.equal(result.protectedStagingCalls, 0);
  });
}

import {
  chaosBombStationaryLaneDisposition,
  projectileInterceptVector,
  stationaryChaosBombHoldingPoint,
} from "./bot_combat_evidence.mjs";

async function runChaosPursuitController(orientation = 0.81044, onPursuit = () => {}, override = null) {
  // One exact frame from the post-failure cold capture. It is a regression
  // stress state, not proof of the keys issued at that later timestamp.
  const metrics = {
    selfPlayerId: 0, playerLifeSeq: 5, playerAlive: true, serverPlayerAlive: true,
    playerX: 8581.93, playerY: 7031.47, playerVelocityX: -22.24,
    playerVelocityY: 97.6, playerOrientation: orientation,
    localGameplayState: {bombLevel: 2},
    remotePlayers: [{ id: 768, lifeSeq: 2, positionX: 8802, positionY: 6613,
      velocityX: 190.5, velocityY: 220.7, lastServerPositionTick: 39658 }],
  };
  if (override) {
    Object.assign(metrics, override.player);
    Object.assign(metrics.remotePlayers[0], override.target);
  }
  let now = 0;
  const keys = [];
  const context = {
    Date: { now: () => (now += 10) }, Number, Math,
    arena: "chaos-zone", botIdBase: 768, bombWeaponTypes: new Set([3, 4]),
    bulletWeaponTypes: new Set([1, 2]), stationaryBotAfterHumanDamage: false,
    botBombSafetyFloor, bombSafetyProfile: bombSafetyProfiles['chaos-zone'],
    firefoxMetricsState: async () => ({ uiMetrics: metrics }),
    playerWeaponDamageToBots: () => [], playerDamageToBots: () => [],
    serverDamageEventsSince: () => [],
    exactLifePrimaryBombTerminal: () => ({ kind: "pending" }),
    positionIsOnAuthoredSafeTile: () => false, authoredHullIsOpen: () => true, authoredLineIsClear: () => true,
    interceptAimVector: projectileInterceptVector,
    movingBombAttackPlan, movingBombControlStep, movingBombNeedsVelocityControl,
    stationaryChaosBombHoldingPoint, chaosBombStationaryLaneDisposition,
    reportStage: (stage, fields) => {
      if (stage === "pursuit_control") onPursuit(fields);
      if (stage === "moving_bomb_pursuit") onPursuit(fields.plan);
    },
    sleep: async () => {},
    signedOrientationDelta: (current, desired) => ((desired - current + 1.5) % 1) - 0.5,
    holdFirefoxKeys: async (_, held) => { keys.push(...held); throw capture; },
  };
  const controller = vm.runInNewContext(`(${controllerSource})`, context);
  await assert.rejects(controller({}, {}, {}, 0, 0, 768, true, "bomb", 500, 2),
    error => error === capture);
  return keys;
}

test("Chaos pursuit thrust must reduce the requested velocity error while turning", async () => {
  let pursuit;
  const orientation = 0.81044;
  const keys = await runChaosPursuitController(orientation, fields => { pursuit = fields; });
  assert.ok(keys.length >= 1 && keys.length <= 2);
  assert.ok(keys.some(key => ["a", "d"].includes(key)));
  assert.ok(keys.every(key => ["a", "d", "w", "s"].includes(key)),
    "pursuit cannot fire or spend bomb reserve on afterburner");
  const thrust = keys.includes("w") ? 1 : keys.includes("s") ? -1 : 0;
  if (thrust) {
    const errorX = pursuit.desiredVelocityX + 22.24;
    const errorY = pursuit.desiredVelocityY - 97.6;
    const projection = (Math.sin(orientation * 2 * Math.PI) * thrust * errorX -
      Math.cos(orientation * 2 * Math.PI) * thrust * errorY) / Math.hypot(errorX, errorY);
    assert.ok(projection >= Math.SQRT1_2,
      "real thrust must correct the velocity error within the45degree steering envelope");
  }
});

test("a140px Chaos encounter retreats before firing into its own160px blast", async () => {
  const keys = await runChaosPursuitController(0, () => {}, {
    player: { playerX: 8200, playerY: 8200, playerVelocityX: 0, playerVelocityY: 0,
      playerEnergy: 1700, localGameplayState: { bombLevel: 2, bombCooldown: 0 } },
    target: { positionX: 8200, positionY: 8060, velocityX: 0, velocityY: 0 },
  });
  assert.deepEqual(keys, ["s"], "genuine reverse thrust must precede any Z edge in this unsafe lane");
});

test("aligned Chaos standoff pursuit retains genuine thrust without spending bomb energy on afterburner", async () => {
  // Desired acceleration heading for the same retained source-time sample.
  const dx = 8802 - 8581.93, dy = 6613 - 7031.47;
  const plan = movingBombAttackPlan({ relativeX: dx, relativeY: dy,
    targetVelocityX: 190.5, targetVelocityY: 220.7, playerVelocityX: -22.24, playerVelocityY: 97.6,
    projectileSpeed: 200, maximumFlightSeconds: 2, explosionRadius: 160,
    proximityRadius: 72, currentDirection: 0 });
  const heading = (Math.atan2(plan.desiredVelocityX + 22.24, -(plan.desiredVelocityY - 97.6)) / (2 * Math.PI) + 1) % 1;
  const keys = await runChaosPursuitController(heading);
  assert.deepEqual(keys, ["w"]);
});

test("Chaos bomb pursuit matches observed lateral motion within the existing speed cap", async () => {
  let pursuit;
  await runChaosPursuitController(0.81044, fields => { pursuit = fields; });
  assert.ok(pursuit);
  const dx = 8802 - 8581.93, dy = 6613 - 7031.47;
  const distance = Math.hypot(dx, dy);
  const tangentX = -dy / distance, tangentY = dx / distance;
  const targetLateralSpeed = 190.5 * tangentX + 220.7 * tangentY;
  const desiredLateralSpeed = pursuit.desiredVelocityX * tangentX + pursuit.desiredVelocityY * tangentY;
  assert(targetLateralSpeed > 250, "the observed target has substantial transverse motion");
  assert(desiredLateralSpeed > 200,
    "aiming desired velocity only at the current point forgets the target's lateral flight");
  assert(Math.hypot(pursuit.desiredVelocityX, pursuit.desiredVelocityY) <= 300.000001,
    "velocity matching cannot increase the existing physical pursuit envelope");
});
