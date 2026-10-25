import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
import path from "node:path";
import { browserConsoleEvidence, createConsoleRecorder, assertRendererConsoleClean } from "../../tools/browser_console_evidence.mjs";
import { brakeVelocityDisposition } from "./combat_replay.mjs";
import {
  authoredMapGeometry,
  authoredSafeTiles,
  authoredSafetyDistance,
  authoredSafetyMargin,
  bestBoostHeading,
  authoredExitHeading,
  deepestAuthoredPoint,
  deepestStagingPoint,
} from "./authored_map_geometry.mjs";
import { chaosSafeRouteToAuthoredSafety } from "./chaos_safe_route.mjs";
import {
  authoredFiringRingWaypoint,
  lifecycleRouteBudgetMs,
  movingHullPathIsClear,
  routedHopAccepted,
  routedMapCounterfireWaypoint,
  stageAdvanced,
} from "./bot_combat_evidence.mjs";

test("keyboard consoles retain the real attacker snapshot after its client retires", () => {
  const source = fs.readFileSync(new URL("./keyboard_chord_matrix.mjs", import.meta.url), "utf8");
  const start = source.indexOf("function retainKeyboardConsoles("), end = source.indexOf("async function main(", start);
  assert(start >= 0 && end > start);
  const writes = new Map(), primary = createConsoleRecorder("CDP"), retired = createConsoleRecorder("CDP");
  retired.record({ type: "warning", text: "retained driver warning" });
  const saved = retired.snapshot();
  const ctx = vm.createContext({ path, browserConsoleEvidence,
    fs: { writeFileSync: (name, content) => writes.set(name, content) } });
  vm.runInContext(source.slice(start, end), ctx);
  const actor = { consoleEvidence: () => primary.snapshot() };
  const audits = ctx.retainKeyboardConsoles("/evidence", actor, null, saved);
  for (const audit of audits) assertRendererConsoleClean(audit);
  assert.deepEqual(JSON.parse(writes.get("/evidence/lifecycle-attacker-console.json")), saved);
  assert.equal(audits[1].total, 1);

  const missing = ctx.retainKeyboardConsoles("/evidence", actor, null);
  assert.throws(() => assertRendererConsoleClean(missing[1]), /missing/);
  retired.record({ type: "warning", text: "Entire call rejected: fixture" });
  const active = ctx.retainKeyboardConsoles("/evidence", actor,
    { consoleEvidence: () => retired.snapshot() }, saved);
  assert.throws(() => assertRendererConsoleClean(active[1]), /rejected a draw/,
    "a live recorder takes precedence over an older saved snapshot");
});

async function replayLifecycleSetup({ pilotX = 9124.17, pilotY = 9130.52, victimX = 6678.99, victimY = 10640.46,
  arenaName = "chaos-zone" } = {}) {
  const mapPath = arenaName === "extreme" ? "extreme/doomsy8.lvl" : "chaos-zone/bdhadar.lvl";
  const geometry = authoredMapGeometry(fs.readFileSync(new URL(`../../assets/zones/${mapPath}`, import.meta.url)));
  const source = fs.readFileSync(new URL("./keyboard_chord_matrix.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function driveLifecyclePeersOutsideSafety("),
    end = source.indexOf("async function aimLifecycleAttackerAtVictim(", start);
  assert(start >= 0 && end > start);
  // Native orchestration/geometry replay, not a ship-physics or browser proof.
  // The virtual driver stops at the ACTUAL caller's accepted arrival radius.
  const victim = { playerX: victimX, playerY: victimY, playerVelocityX: 0, playerVelocityY: 0, inSafeZone: false };
  let pilot = { playerX: pilotX, playerY: pilotY, playerVelocityX: 0, playerVelocityY: 0, inSafeZone: false };
  const drives = [];
  const actor = { metricsState: async () => ({ uiMetrics: victim }) },
    attacker = { metricsState: async () => ({ uiMetrics: pilot }) };
  const ctx = vm.createContext({ arena: arenaName, authoredRouteGeometry: geometry,
    routedMapCounterfireWaypoint, lifecycleRouteBudgetMs, routedHopAccepted, stageAdvanced,
    prepareExtremeChordAttempt: async () => {},
    settleLifecycleClient: async () => ({ settled: true }),
    driveClientToWorldTarget: async (_client, point, budget, settle, tolerance) => {
      const dx = point.x - pilot.playerX, dy = point.y - pilot.playerY, distance = Math.hypot(dx, dy);
      drives.push({ distance, settle, tolerance, budget });
      assert(movingHullPathIsClear({ x: pilot.playerX, y: pilot.playerY, velocityX: dx, velocityY: dy,
        seconds: 1, hullIsOpen: geometry.authoredHullIsTraversable }), "every planned leg clears the real map hull");
      const ratio = distance > tolerance ? (distance - tolerance) / distance : 0;
      pilot = { ...pilot, playerX: pilot.playerX + dx * ratio, playerY: pilot.playerY + dy * ratio };
      return { reached: true, distance: Math.min(distance, tolerance), pulses: ratio > 0 ? 1 : 0 };
    },
  });
  vm.runInContext(source.slice(start, end), ctx);
  await ctx.driveLifecyclePeersOutsideSafety(actor, attacker);
  return { drives, pilot, victim };
}

test("the actual lifecycle coordinator drives a retained sub24px corner before replanning", async () => {
  const { drives, pilot, victim } = await replayLifecycleSetup();
  assert(drives[0].distance < 24 && drives[0].distance > 8, "retain the exact corner that the old radius swallowed");
  assert.equal(drives[0].settle, true);
  assert.equal(drives[0].tolerance, 4);
  assert(drives.every(d => d.distance <= 768.000001), "every actual setup hop fits the bounded route controller");
  assert(Math.hypot(pilot.playerX - victim.playerX, pilot.playerY - victim.playerY) <= 212);
});

test("the retained 4.6km direct lifecycle approach is split inside its existing stage deadline", async () => {
  const { drives, pilot, victim } = await replayLifecycleSetup({
    pilotX: 9304, pilotY: 8952, victimX: 7939.37, victimY: 4351.96,
  });
  assert(drives.length > 1, "the old single hop cannot physically fit90s at42px/s");
  for (const drive of drives) {
    assert(drive.distance <= 768.000001, "bounded hops preserve the original capped stage budget");
    assert(drive.distance / 42 * 1000 < drive.budget);
    assert(drive.budget <= 90_000, "the repair must not raise the original hard deadline");
  }
  assert(Math.hypot(pilot.playerX - victim.playerX, pilot.playerY - victim.playerY) <= 212);
});

test("the Extreme coordinator preserves full hull clearance and bounded transit hops", async () => {
  const { drives, pilot, victim } = await replayLifecycleSetup({ arenaName: "extreme",
    pilotX: 7929.54, pilotY: 8048.8, victimX: 7917.15, victimY: 6641.05 });
  assert(drives.every(d => d.distance <= 768.000001));
  assert(drives[0].settle && drives[0].tolerance === 4);
  assert(Math.hypot(pilot.playerX - victim.playerX, pilot.playerY - victim.playerY) <= 220);
});

test("lifecycle combat awaits the explicit shared encounter before local firing setup", async () => {
  const source = fs.readFileSync(new URL("./keyboard_chord_matrix.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function proveKeyboardOwnedDeathAndRespawn("),
    end = source.indexOf("  let firingLane =", start);
  assert(start >= 0 && end > start);
  const events = [], actor = {}, server = {}, attacker = {
    metricsState: async () => ({ uiMetrics: { selfPlayerId: 2 } }),
  };
  let arrived = false, originReady = false, encounterDeadline, now = 1_000;
  const geometry = {};
  const ctx = vm.createContext({
    Date: { now: () => now }, authoredRouteGeometry: geometry,
    prepareKeyboardLifecycleOrigin: async options => {
      assert.equal(options.actor, actor); assert.equal(options.server, server);
      assert.equal(options.playerId, 1); assert.equal(options.initialLifeSeq, 1);
      assert.equal(options.geometry, geometry);
      assert.equal(options.deadline, now + 180_000);
      encounterDeadline = options.deadline; events.push("origin-start");
      await Promise.resolve(); now += 250; originReady = true; events.push("origin-ready");
      return { warps: [] };
    },
    waitUntil: async (_label, predicate) => assert(await predicate()), initialAuthorityReady: () => true,
    rendezvousPlayers: async (pilot, peer, actualServer, options) => {
      assert(originReady, "origin recovery must finish before route certification");
      assert(options.setupBudgetMs > 0 && options.setupBudgetMs <= 180_000);
      assert.equal(options.setupBudgetMs, encounterDeadline - now);
      assert.equal(pilot, actor); assert.equal(peer, attacker); assert.equal(actualServer, server);
      events.push("encounter-start"); await Promise.resolve(); arrived = true; events.push("encounter-ready");
      return { status: "complete", keysOnly: true };
    },
    driveLifecyclePeersOutsideSafety: async () => {
      assert(arrived); events.push("local-firing-setup"); return { routeEvidence: [] };
    },
  });
  vm.runInContext(source.slice(start, end) + "return { lifecycleRendezvous, lifecycleRoute };\n}", ctx);
  const result = await ctx.proveKeyboardOwnedDeathAndRespawn({ actor, attacker, server, playerId: 1, initialLifeSeq: 1 });
  assert.deepEqual(events, ["origin-start", "origin-ready", "encounter-start", "encounter-ready", "local-firing-setup"]);
  assert.equal(result.lifecycleRendezvous.status, "complete");
});

test("retained west-drift Extreme lifecycle route avoids the authored wall", () => {
  const geometry = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/extreme/doomsy8.lvl", import.meta.url)));
  const target = { positionX: 7917.15, positionY: 6641.05 };
  const metrics = { playerX: 7929.54, playerY: 8048.8 };
  assert.equal(movingHullPathIsClear({ x: metrics.playerX, y: metrics.playerY,
    velocityX: target.positionX - metrics.playerX, velocityY: target.positionY + 96 - metrics.playerY,
    seconds: 1, hullIsOpen: geometry.authoredHullIsTraversable }), false);
  let reached = false;
  for (let step = 0; step < 16; step += 1) {
    const waypoint = routedMapCounterfireWaypoint({ metrics, target, ...geometry });
    assert.ok(waypoint, `route must progress at ${JSON.stringify(metrics)}`);
    assert.equal(movingHullPathIsClear({ x: metrics.playerX, y: metrics.playerY,
      velocityX: waypoint.x - metrics.playerX, velocityY: waypoint.y - metrics.playerY,
      seconds: 1, hullIsOpen: geometry.authoredHullIsTraversable }), true);
    metrics.playerX = waypoint.x;
    metrics.playerY = waypoint.y;
    if (!waypoint.routed) { reached = true; break; }
  }
  assert.ok(reached, "bounded route reaches a hull-clear firing position");
  assert.equal(geometry.authoredHullIsOpen(metrics.playerX, metrics.playerY), true);
  assert.equal(geometry.authoredLineIsClear(metrics.playerX, metrics.playerY, target.positionX, target.positionY), true);
});

test("retained Chaos lifecycle pin is routed around the authored wall", () => {
  // Coordinates from the retained Chrome/Chaos mild abort: the attacker held
  // thrust at (8180.6, 8153.4) toward the authored staging column
  // victim + (0, +96) = (8173.05, 7408.63) for 410 pulses, best distance
  // 701.4 px, velocity (-1.5, 6.5) px/s, selfCorrectionCount 0.
  const geometry = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const victim = { positionX: 8173.05, positionY: 7312.63 };
  const authoredColumn = { x: victim.positionX, y: victim.positionY + 96 };
  const pilot = { playerX: 8180.58, playerY: 8153.37 };
  assert.equal(movingHullPathIsClear({
    x: pilot.playerX,
    y: pilot.playerY,
    velocityX: authoredColumn.x - pilot.playerX,
    velocityY: authoredColumn.y - pilot.playerY,
    seconds: 1,
    hullIsOpen: geometry.authoredHullIsTraversable,
  }), false, "the authored Chaos staging column must be the wall that pinned the lane");

  let metrics = { ...pilot };
  let reached = false;
  for (let step = 0; step < 16; step += 1) {
    const waypoint = routedMapCounterfireWaypoint({ metrics, target: victim, ...geometry });
    assert.ok(waypoint, `route must progress at ${JSON.stringify(metrics)}`);
    assert.equal(movingHullPathIsClear({
      x: metrics.playerX,
      y: metrics.playerY,
      velocityX: waypoint.x - metrics.playerX,
      velocityY: waypoint.y - metrics.playerY,
      seconds: 1,
      hullIsOpen: geometry.authoredHullIsTraversable,
    }), true, `waypoint ${step} must be hull-clear`);
    metrics = { playerX: waypoint.x, playerY: waypoint.y };
    if (!waypoint.routed) { reached = true; break; }
  }
  assert.ok(reached, "bounded route reaches a hull-clear firing position");
  assert.equal(geometry.authoredLineIsClear(metrics.playerX, metrics.playerY, victim.positionX, victim.positionY), true);
});

// Recorded Chrome/Chaos mild staging stall (`MILD-VERIFY-3`): the attacker
// reached a 36 px corridor hop and then every one of the 16 bounded stages
// reported `reached: true` with `pulses: 0`, crawling 0.05 px each time.
const recordedChaosStall = [
  [8200, 8200, 8200, 8031.97],
  [8455.03, 8155.93, 8200, 8054.49],
  [8455.08, 8155.95, 8200, 8054.54],
  [8455.08, 8155.95, 8200, 8054.6],
  [8455.13, 8155.97, 8200, 8054.65],
  [8455.18, 8155.99, 8200, 8054.65],
  [8455.27, 8156.02, 8200, 8054.75],
  [8455.27, 8156.02, 8200, 8054.81],
  [8455.32, 8156.04, 8200, 8054.86],
  [8455.37, 8156.06, 8200, 8054.86],
  [8455.42, 8156.08, 8200, 8054.91],
  [8455.42, 8156.08, 8200, 8054.97],
  [8455.47, 8156.1, 8200, 8054.97],
  [8455.52, 8156.11, 8200, 8055.02],
];

test("recorded Chaos route stall hop is shorter than the arrival circle that ate it", () => {
  const geometry = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const [pilotX, pilotY, victimX, victimY] = recordedChaosStall[1];
  const metrics = { playerX: pilotX, playerY: pilotY };
  const victim = { positionX: victimX, positionY: victimY };
  const waypoint = routedMapCounterfireWaypoint({ metrics, target: victim, ...geometry });
  assert.ok(waypoint?.routed, "the stuck pilot is still behind the authored wall");
  const hop = Math.hypot(waypoint.x - metrics.playerX, waypoint.y - metrics.playerY);
  assert.ok(hop < 40, `hop ${hop} must sit inside the retired 40 px arrival circle`);
  assert.ok(hop > 24, `hop ${hop} must exceed the arrival circle the staging loop now uses`);
});

test("staging livelock guard measures movement, not the distance a detour raises", () => {
  assert.equal(routedHopAccepted({ reached: true, distance: 38.5 }), true);
  assert.equal(routedHopAccepted({ reached: false, reason: "no route progress", bestDistance: 28.98 }), true);
  assert.equal(routedHopAccepted({ reached: false, reason: "no route progress", bestDistance: 40.2 }), false);
  assert.equal(routedHopAccepted({ reached: false, reason: "route blocked", bestDistance: 701.4 }), false);

  // The recorded Chrome/Chaos stall: the first stage is a real 273 px hop into the
  // corridor, then every stage crawls about 0.05 px, so the guard has to trip on the
  // second dead stage.
  let stalledStages = 0;
  let guardStage = -1;
  for (let stage = 1; stage < recordedChaosStall.length && guardStage < 0; stage += 1) {
    const [fromX, fromY] = recordedChaosStall[stage - 1];
    const [afterX, afterY] = recordedChaosStall[stage];
    if (stageAdvanced({
      from: { playerX: fromX, playerY: fromY },
      after: { playerX: afterX, playerY: afterY },
    })) {
      stalledStages = 0;
    } else if ((stalledStages += 1) >= 2) {
      guardStage = stage;
    }
  }
  assert.equal(guardStage, 3, "two non-advancing stages trip the guard");
  assert.equal(stalledStages, 2);

  // The route the victim-distance metric killed
  // (`$QA/mild-verify-11.sh.log:32460`): the victim sat 137 px away while the
  // attacker worked a 35 -> 15 -> 14 tile path around the wall south of it,
  // covering 15-23 px per stage. Those stages moved, and the quantity the old
  // guard watched went the wrong way.
  const leg = [[8_200, 8_200], [8_469.14, 8_154.18], [8_479.25, 8_142.84]];
  const victim = [8_200, 8_062.48];
  for (let stage = 1; stage < leg.length; stage += 1) {
    assert.equal(stageAdvanced({
      from: { playerX: leg[stage - 1][0], playerY: leg[stage - 1][1] },
      after: { playerX: leg[stage][0], playerY: leg[stage][1] },
    }), true, `stage ${stage} moved the ship`);
  }
  assert.ok(
    Math.hypot(leg[1][0] - victim[0], leg[1][1] - victim[1])
      > Math.hypot(leg[0][0] - victim[0], leg[0][1] - victim[1]),
    "the retained detour raised the victim distance while doing real work",
  );
});

test("lifecycle staging budget outlasts the capped cruise of the retained 914px hop", () => {
  // MILD-VERIFY-6 stage 0 (Chrome/Extreme): the victim settled 1,094 px from
  // the attacker's spawn, the planner offered one direct waypoint 914.21 px
  // away, and the fixed 30 s Extreme budget expired at bestDistance 290.76 px
  // after seven pulses. At the driver's 42 px/s desired-speed cap that hop is
  // 21.8 s of cruise before any acceleration, turning or movement correction.
  const cruiseMs = (914.21 / 42) * 1_000;
  assert.ok(cruiseMs > 21_000 && cruiseMs < 30_000,
    `the retained hop only fits a 30 s budget as bare cruise (${cruiseMs} ms)`);
  const budget = lifecycleRouteBudgetMs(914.21, 42);
  assert.ok(budget > cruiseMs * 1.4, `${budget} ms must outlast ${cruiseMs} ms of capped cruise`);
  assert.ok(budget < 90_000, `${budget} ms must stay inside the hard stage bound`);
  // Short hops keep the original floor, so this is a budget, not a licence to
  // let a stalled lane coast for a minute and a half.
  assert.equal(lifecycleRouteBudgetMs(120, 42), 20_000);
  assert.ok(lifecycleRouteBudgetMs(200, 42) < lifecycleRouteBudgetMs(900, 42));
});

test("lifecycle firing gap closes on the authored ring around the victim", () => {
  const chaos = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const extreme = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/extreme/doomsy8.lvl", import.meta.url)));

  // The first mild run of the set (`$QA/mild-verify-8.sh.log:22344`,
  // Firefox/Extreme): the refresh routed the fixed `victim + (0, +96)` column
  // and threw
  // `no hull-clear lifecycle aim route` at 197.2 px of separation. The column was
  // the bug - on this Extreme map it is inside terrain, so no planner candidate
  // exists at all, while the victim-centred alternatives are both drivable.
  const walled = { playerX: 8_281.23, playerY: 7_342.38 };
  const walledVictim = { positionX: 8_303.25, positionY: 7_146.44 };
  const column = { positionX: walledVictim.positionX, positionY: walledVictim.positionY + 96 };
  assert.equal(extreme.authoredHullIsTraversable(column.positionX, column.positionY), false,
    "the fixed column sits in authored terrain here");
  assert.equal(routedMapCounterfireWaypoint({ metrics: walled, target: column, ...extreme }), null,
    "so routing to it can only throw");
  assert.ok(routedMapCounterfireWaypoint({ metrics: walled, target: walledVictim, ...extreme }),
    "the same ship has a firing position when the target is the victim");
  const ring = authoredFiringRingWaypoint({ metrics: walled, victim: walledVictim, ...extreme });
  assert.ok(ring, "and the ring finds one on the bearing the ship already holds");
  assert.ok(ring.distance >= 40, `${ring.distance} px must clear the driver's arrival circle`);
  assert.equal(Math.round(Math.hypot(ring.x - walledVictim.positionX, ring.y - walledVictim.positionY)), 120,
    "the ring sits at the assertion radius, so arrival cannot land outside 160 px");

  // The second mild run of the set (`$QA/mild-verify-8.sh.log:28668`,
  // Firefox/Chaos): the planner answered a 19.03 px candidate to a ship that had
  // to close to 160 px. A point inside the driver's arrival circle is never
  // driven, so the loop livelocked.
  // Reconstructed from the retained candidate (the pilot stood 19.03 px off it):
  const stalled = { playerX: 7_818.48, playerY: 7_877.13 };
  const stalledVictim = { positionX: 7_818.48, positionY: 7_762.10 };
  const plannerHop = routedMapCounterfireWaypoint({ metrics: stalled, target: stalledVictim, ...chaos });
  assert.ok(plannerHop.distance < 40,
    `the planner still offers the refused micro-hop here (${plannerHop.distance} px)`);
  const closed = authoredFiringRingWaypoint({ metrics: stalled, victim: stalledVictim, ...chaos });
  assert.ok(closed.distance >= 40, `the ring hop is drivable (${closed.distance} px)`);

  // The ring is not a licence to give up on a walled victim: when no bearing on
  // it is drivable the caller still falls back to the multi-tile planner. This is
  // the retained 716.5 px pin where the straight refresh pinned into a wall.
  const blocked = { playerX: 8_180.58, playerY: 8_153.37 };
  const blockedVictim = { positionX: 8_173.05, positionY: 7_312.63 };
  assert.equal(authoredFiringRingWaypoint({ metrics: blocked, victim: blockedVictim, ...chaos }), null,
    "no bearing on the ring reaches that victim");
  assert.ok(routedMapCounterfireWaypoint({ metrics: blocked, target: blockedVictim, ...chaos })?.routed,
    "so the planner routes around the wall");

  assert.equal(authoredFiringRingWaypoint({
    metrics: walled,
    victim: walledVictim,
    minimumHopPx: Number.POSITIVE_INFINITY,
    ...extreme,
  }), null, "candidates shorter than the minimum hop are rejected");
});

test("staging exit measures the envelope against a reachable threshold", () => {
  const envelopePx = 220;
  const plannerRadiusPx = 180;
  const hopAcceptancePx = 32;
  const mechanismFloorPx = plannerRadiusPx + hopAcceptancePx;

  // `$QA/mild-verify-9.sh.log:702` exited staging at a measured 225.70 px: the
  // terminal waypoint was scored against the victim sampled before the hop, and
  // the settled Chaos victim kept drifting at 3.17 px/s through the hop budget and
  // a 20 s attacker settle. Planner radius plus the driver's ~29 px stop precision
  // plus that drift is already past the envelope, which is why the loop now samples
  // both ships together instead of inferring the separation.
  const staleBoundPx = plannerRadiusPx + 29 + 3.17 * 20;
  assert.ok(staleBoundPx > envelopePx,
    `${staleBoundPx.toFixed(1)} px must be able to exceed the ${envelopePx} px envelope`);

  // `$QA/mild-verify-10.sh.log:32496` is the other half of the same lesson: with a
  // simultaneous measurement the loop plateaued at 202.495 px - inside the
  // envelope - and livelocked, because the break had been set at an arbitrary
  // 200 px that no stage can reach. The threshold is the mechanism floor, and the
  // envelope keeps the remaining margin for the re-sample's drift.
  const retainedBestPx = 202.495;
  assert.ok(retainedBestPx <= envelopePx,
    "the retained lane had already satisfied the envelope when it livelocked");
  assert.ok(mechanismFloorPx <= envelopePx,
    "the threshold must leave the envelope a drift margin");
  assert.ok(retainedBestPx < mechanismFloorPx,
    "the floor explains the distance the loop plateaued at");
});

test("authored boost heading refuses the retained Extreme safe-tile stall", () => {
  // CHORDS-VERIFY-2 (Firefox/Extreme, after-respawn ArrowUp->Shift): the pilot
  // was sampled at (8051.98, 8367.07) with a 1.08 s thrust+Afterburner hold
  // reaching 3.68 px/s. That point is authored safe tile 171, where the server
  // damps thrust, and the heading it held (`input_direction: 30`, due west)
  // pointed back into the central green.
  const bytes = fs.readFileSync(new URL("../../assets/zones/extreme/doomsy8.lvl", import.meta.url));
  const geometry = authoredMapGeometry(bytes);
  const safetyDistance = authoredSafetyDistance(authoredSafeTiles(bytes));
  const hull = (x, y) => geometry.authoredHullIsTraversable(x, y);
  assert.equal(geometry.tileAt(8_051.98, 8_367.07), 171);
  assert.equal(safetyDistance(8_051.98, 8_367.07), 0);
  assert.equal(bestBoostHeading({
    hullIsTraversable: hull, safetyDistance, x: 8_051.98, y: 8_367.07,
    travelPx: 512, clearancePx: 128,
  }), null, "no measurement heading may exist inside authored safety");

  // The staged waypoint is measurable, and its authored corridor runs east,
  // away from the green.
  const heading = bestBoostHeading({
    hullIsTraversable: hull, safetyDistance, x: 9_000, y: 8_200,
    travelPx: 512, clearancePx: 128,
  });
  assert.ok(heading, "the staged Extreme waypoint must offer a boost corridor");
  assert.ok(heading.dx > 0.9 && Math.abs(heading.dy) < 0.3, JSON.stringify(heading));
  assert.ok(heading.clearancePx >= 128, JSON.stringify(heading));
  assert.ok(heading.runwayPx >= 512, JSON.stringify(heading));
});

test("authored boost heading requires both travel room and safety clearance", () => {
  // West of x = 500 is the safe half-plane; the ship sits at (700, 500).
  const safetyDistance = (x) => (x < 500 ? 0 : x - 500);
  const options = { safetyDistance, x: 700, y: 500, travelPx: 512, clearancePx: 128, bins: 4 };
  const east = bestBoostHeading({ ...options, hullIsTraversable: () => true });
  assert.ok(Math.abs(east.dx - 1) < 0.01 && Math.abs(east.dy) < 0.01,
    `the corridor away from safety wins the tie (${JSON.stringify(east)})`);

  // A wall east of the ship leaves only the headings parallel to safety.
  const walled = bestBoostHeading({
    ...options,
    hullIsTraversable: (x) => x < 950,
  });
  assert.ok(Math.abs(walled.dx) < 0.01 && Math.abs(Math.abs(walled.dy) - 1) < 0.01,
    `a short east corridor must be rejected (${JSON.stringify(walled)})`);

  assert.equal(
    bestBoostHeading({ ...options, hullIsTraversable: () => true, safetyDistance: () => 0 }),
    null, "a ship with no authored clearance has no measurement heading",
  );
});

test("the map model stamps an authored animated tile across its whole footprint", () => {
  // The server's `animated_tile_size` expands these ids into a footprint and stamps
  // the id into every covered cell, and collision runs against that runtime grid. A
  // model that paints only the anchor tile hands a staging harness a phantom corridor,
  // so the covered cells must be part of the contract, not an optimisation.
  const records = [
    { x: 10, y: 10, id: 219, size: 6 },
    { x: 20, y: 21, id: 217, size: 2 },
    { x: 30, y: 31, id: 220, size: 5 },
    { x: 40, y: 41, id: 161, size: 1 },
  ];
  const bytes = Buffer.alloc(6 + records.length * 4);
  bytes.write("BM", 0, "ascii");
  bytes.writeUInt32LE(6, 2);
  records.forEach((record, index) => {
    bytes.writeUInt32LE(((record.id << 24) | (record.y << 12) | record.x) >>> 0, 6 + index * 4);
  });

  const geometry = authoredMapGeometry(bytes);
  for (const { x, y, id, size } of records) {
    for (let dy = 0; dy < size + 1; dy += 1) {
      for (let dx = 0; dx < size + 1; dx += 1) {
        const covered = dx < size && dy < size;
        assert.equal(
          geometry.tiles[(y + dy) * 1024 + (x + dx)],
          covered ? id : 0,
          `${id}@${x},${y} cell +${dx}+${dy} must be ${covered ? id : "untouched"}`,
        );
      }
    }
  }

  // The footprint changes what the harness may believe about open space: this hull sits
  // entirely inside the trailing columns of the 219 blob, which the anchor record alone
  // would have reported as empty map.
  assert.equal(geometry.authoredHullIsTraversable(10 * 16 + 8 + 5 * 16, 10 * 16 + 8), false);
});

test("Chaos safety is only reachable by navigation and the route follows authored corridors", () => {
  const bytes = fs.readFileSync(
    new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url),
  );
  const geometry = authoredMapGeometry(bytes);
  const route = chaosSafeRouteToAuthoredSafety(bytes, { from: { x: 8_200, y: 8_200 } });
  // The authored fact that makes Chaos a *navigation* case rather than a spawn case:
  // unlike Extreme, the canonical spawn is not on the safe surface.
  assert.equal(route.fromOnSafety, false);
  assert.equal(geometry.tileAt(8_200, 8_200), 0);
  assert.ok(route.safeTileCount > 100, "Chaos authors plenty of safety, just not at spawn");
  // The reachable green is far, which is why the case cannot teleport or respawn into it.
  assert.ok(route.hops > 100, `expected a multi-corridor route, got ${route.hops} hops`);
  assert.equal(route.stageTileId, 171);
  assert.equal(geometry.tileAt(route.stage.x, route.stage.y), 171);
  assert.ok(route.waypoints.length >= 2);

  const spawn = { x: 8_200, y: 8_200 };
  let from = spawn;
  for (const [index, waypoint] of route.waypoints.entries()) {
    const length = Math.hypot(waypoint.x - from.x, waypoint.y - from.y);
    assert.ok(
      geometry.authoredLineIsClear(from.x, from.y, waypoint.x, waypoint.y),
      `route leg ${index} to ${JSON.stringify(waypoint)} crosses authored terrain`,
    );
    assert.ok(length <= 257, `route leg ${index} is ${length}px, longer than one corridor`);
    from = waypoint;
  }
  // A stage with no room to travel would turn the fire+thrust release witness into the
  // wall bounce the Extreme staging already refuses to measure.
  const stage = route.stage;
  // A stage whose facing runs into a wall would measure a wall bounce instead of thrust.
  // The approach brakes at every drive point it is handed, so the corner polyline has to be
  // the one that gets flown: fewer brakes on a straight corridor, and still only ever
  // straight runs of tiles the BFS certified as hull-traversable.
  assert.ok(route.drivePoints.length < route.waypoints.length,
    `the 900px drive polyline should be shorter than the ${route.waypoints.length}-point leg list, got ${route.drivePoints.length}`);
  assert.deepEqual(route.drivePoints.at(-1), route.stage);
  for (let index = 1; index < route.drivePoints.length; index += 1) {
    const from = route.drivePoints[index - 1];
    const to = route.drivePoints[index];
    assert.ok(geometry.authoredLineIsClear(from.x, from.y, to.x, to.y),
      `drive segment ${index} from ${JSON.stringify(from)} to ${JSON.stringify(to)} crosses authored terrain the hull cannot pass`);
  }
  assert.ok(route.safeRun.runPx >= 48,
    `the reachable green must run at least three authored tiles deep, got ${JSON.stringify(route.safeRun)}`);
  assert.ok(geometry.authoredLineIsClear(stage.x, stage.y,
    stage.x + route.safeRun.x * route.safeRun.runPx, stage.y + route.safeRun.y * route.safeRun.runPx));
  assert.ok(route.escapeHeading.clearPx >= 256,
    `the reachable green needs 256px of authored clearance, got ${JSON.stringify(route.escapeHeading)}`);
  assert.ok(geometry.authoredLineIsClear(stage.x, stage.y,
    stage.x + route.escapeHeading.x * route.escapeHeading.clearPx,
    stage.y + route.escapeHeading.y * route.escapeHeading.clearPx));
  assert.ok(
    geometry.authoredLineIsClear(stage.x, stage.y, stage.x, stage.y + 320),
    "the reachable Chaos green needs travel room for the post-release thrust witness",
  );

  // The first real run failed exactly here: the ordinary chord matrix had flown the pilot
  // to x=10440 before this phase, so a route drawn from the spawn aimed it at nothing.
  const elsewhere = chaosSafeRouteToAuthoredSafety(bytes, { from: { x: 10_440.63, y: 8_447.28 } });
  assert.equal(elsewhere.stageTileId, 171);
  assert.ok(elsewhere.hops > 0 && elsewhere.hops < route.hops,
    `a route from the parked pilot should be nearer than one from the spawn: ${elsewhere.hops} vs ${route.hops}`);
  assert.equal(Math.floor(elsewhere.start.x / 16), elsewhere.startTile.x);
});

test("the Chaos safe route fails loudly when a map offers no reachable safety", () => {
  const header = Buffer.alloc(6);
  header.write("BM", 0, "utf8");
  header.writeUInt32LE(4, 2);
  const records = [];
  const pack = (id, x, y) => {
    const b = Buffer.alloc(4);
    b.writeUInt32LE(((id << 24) | (y << 12) | x) >>> 0);
    return b;
  };
  // One isolated safe tile in an otherwise empty field, walled in on all four sides.
  records.push(pack(171, 300, 300));
  for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
    records.push(pack(2, 300 + dx, 300 + dy));
  }
  for (let x = 0; x < 64; x += 1) {
    for (let y = 0; y < 64; y += 1) records.push(pack(0, x, y));
  }
  assert.throws(
    () => chaosSafeRouteToAuthoredSafety(Buffer.concat([header, ...records]), {
      from: { x: 8_200, y: 8_200 },
    }),
    /no authored Chaos safe tile is reachable/,
  );
});

// The Chaos chord lane's safety assertions were made from positions a few px inside a green that never offers
// more than two tiles, which is why sub-tile jitter decided them. These cases pin the quantity itself, because
// every margin-floor decision downstream is only as good as this measurement.
const blockSafe = (tilesWide) => (x, y) => {
  const tx = Math.floor(x / 16);
  const ty = Math.floor(y / 16);
  return tx >= 0 && tx < tilesWide && ty >= 0 && ty < tilesWide;
};

test("authoredSafetyMargin is null outside safety and one tile at the axis-limited centre of a 3x3 block", () => {
  const margin = authoredSafetyMargin(blockSafe(3));
  assert.equal(margin(60, 24), null, "a point outside the block reports no margin rather than a distance");
  // A 48x48 square is 24 px from its centre to the nearest face, and a 32-direction fan contains the four axis
  // directions, so the minimum walk is the face distance - not the 33.9 px diagonal exit.
  assert.equal(margin(24, 24), 24);
  assert.equal(margin(47, 24), 1, "one px inside the face is a one px margin, which is what these lanes ran on");
});

test("authoredSafetyMargin reports its cap as a cap and the shipped Chaos green as two tiles thick", () => {
  // The centre of a 40-tile (640 px) block is 320 px from its nearest face, so the true margin is below the
  // cap and must be reported as a distance; a 100-tile block's centre is 800 px out, which the cap reports as
  // the cap - at least that much, not exactly that much.
  assert.equal(authoredSafetyMargin(blockSafe(40))(320, 320), 320);
  assert.equal(authoredSafetyMargin(blockSafe(100))(800, 800), 480);
  const chaosGeometry = authoredMapGeometry(fs.readFileSync(
    new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const chaos = authoredSafetyMargin((x, y) => chaosGeometry.tileAt(x, y) === 171);
  // Measured: the deepest authored point near the chord lane's staging spot, and the six release
  // positions its instrument captured. If the green changes shape, this is the assertion that says whether the
  // sub-tile problem was fixed or merely moved.
  assert.equal(chaos(10086, 6930), 30, "deepest authored point in the staging neighbourhood");
  for (const [x, y] of [[10119.64, 6954.12], [10068.36, 6952.58], [10122.06, 6948.88],
    [10069.78, 6955.41], [10121.19, 6954.02], [10105.12, 6958.84]]) {
    const value = chaos(x, y);
    assert.ok(value !== null && value <= 12, `release position ${x},${y} sits with margin ${value}, expected <=12`);
  }
});

test("deepestAuthoredPoint moves an entry point off a thin edge to the widest spot in the region", () => {
  // A 3x3 block entered at its corner tile: the corner's own margin is one tile, and the widest spot in the
  // block is two tiles out at the centre. This is the shape the Chaos entry edge has, at a scale the
  // assertions can state exactly.
  const isSafe = blockSafe(3);
  const best = deepestAuthoredPoint(isSafe, { from: { x: 8, y: 8 }, radiusPx: 48, stepPx: 8 });
  assert.equal(best.x, 24);
  assert.equal(best.y, 24);
  assert.equal(best.marginPx, 24);
  assert.equal(best.marginCap, false);
  assert.equal(deepestAuthoredPoint((() => false), { from: { x: 8, y: 8 }, radiusPx: 16 }), null,
    "a region with no authored point returns null rather than a fabricated spot");
});

test("deepestAuthoredPoint finds the Chaos staging region's real depth from a captured release position", () => {
  const geometry = authoredMapGeometry(fs.readFileSync(
    new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const best = deepestAuthoredPoint((x, y) => geometry.tileAt(x, y) === 171,
    { from: { x: 10119.64, y: 6954.12 }, radiusPx: 256, stepPx: 8 });
  // The release position itself has 6 px of margin; the region within 256 px holds 30 px. A staging floor can be
  // asked for here, which is what the chord lane now does instead of asserting on the entry edge.
  assert.ok(best.marginPx >= 24, `expected at least 24 px of authored depth nearby, found ${best.marginPx}`);
  assert.ok(Math.hypot(best.x - 10119.64, best.y - 6954.12) <= 256);
});

test("deepestStagingPoint refuses a spot that is deep in safety but hull-blocked", () => {
  // Safety covers the whole 16x16 block, but the hull only fits the left half - the shape of the Chaos pocket,
  // where the release position sat 23 px inside authored safety with a hull that could not leave its nook. A
  // safety-only search lands on the blocked half's edge with no hull clearance at all; binding clearance picks
  // the point that has room on both surfaces.
  const isSafe = (x, y) => x >= 0 && x < 256 && y >= 0 && y < 256;
  const isHullClear = (x, y) => isSafe(x, y) && x < 128;
  const best = deepestStagingPoint({ isSafe, isHullClear, from: { x: 128, y: 128 }, radiusPx: 128 });
  assert.ok(best.safetyMarginPx >= 48 && best.hullMarginPx >= 48, JSON.stringify(best));
  assert.equal(best.bindingMarginPx, Math.min(best.safetyMarginPx, best.hullMarginPx));
  assert.ok(best.x < 128, "the chosen spot must sit where the hull actually fits");
  const safetyOnly = deepestAuthoredPoint(isSafe, { from: { x: 128, y: 128 }, radiusPx: 128 });
  assert.ok(isHullClear(safetyOnly.x, safetyOnly.y) === false,
    "the safety-only choice is exactly the trap this primitive exists to avoid");
  assert.throws(() => deepestStagingPoint({ isSafe, from: { x: 0, y: 0 } }), /one surface/);
});

test("fractional entry phases cannot hide the thin Chaos strip's true staging centre", () => {
  const geometry = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const isSafe = (x, y) => geometry.tileAt(x, y) === 171;
  const isHullClear = (x, y) => geometry.authoredHullIsTraversable(x, y);
  for (const dx of [0.03, 0.42, 0.97]) for (const dy of [0.19, 0.51, 0.99]) {
    const best = deepestStagingPoint({ isSafe, isHullClear, from: { x: 7773 + dx, y: 9488 + dy } });
    assert(best?.safetyMarginPx >= 16 && best?.hullMarginPx >= 16, JSON.stringify(best));
    const safety = authoredSafetyMargin(isSafe), hull = authoredSafetyMargin(isHullClear);
    for (let angle = 0; angle < Math.PI * 2; angle += Math.PI / 8) {
      assert(safety(best.x + Math.cos(angle) * 0.5, best.y + Math.sin(angle) * 0.5) >= 16);
      assert(hull(best.x + Math.cos(angle) * 0.5, best.y + Math.sin(angle) * 0.5) >= 16);
    }
  }
});

test("actual Chaos safety setup centres both arriving and already-safe pilots without lowering the floors", async () => {
  const geometry = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const isSafe = (x, y) => geometry.tileAt(x, y) === 171;
  const source = fs.readFileSync(new URL("./keyboard_chord_matrix.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function stageChaosSafeZonePilot("), end = source.indexOf("\nif (!new Set", start);
  assert(start >= 0 && end > start);
  // Orchestration/geometry only. The fake driver supplies an arrival at the
  // actual caller's declared precision; browser input and physics need renewal.
  for (const alreadySafe of [false, true]) {
    const entry = { x: 7773.42, y: 9488.19 }, drives = [];
    let m = { playerX: alreadySafe ? entry.x : 7740, playerY: entry.y, inSafeZone: alreadySafe };
    const actor = { metricsState: async () => ({ uiMetrics: m }) };
    const ctx = vm.createContext({ authoredRouteGeometry: geometry, chaosSafeMapBytes: Buffer.alloc(0),
      chaosAuthoredSafe: isSafe, authoredSafetyMargin, deepestStagingPoint, authoredExitHeading,
      CHAOS_ROUTE_LEG_ARRIVAL_TOLERANCE_PX: 4,
      brakeClientVelocity: async () => ({ braked: true, speed: 0 }),
      chaosSafeRouteToAuthoredSafety: () => ({ stage: entry, stageTile: {}, hops: 1,
        waypoints: [entry], legLengths: [], drivePoints: [entry], safeRun: {} }),
      driveRouteWithReplans: async options => {
        for (const point of options.initialDrivePoints) await options.driveLeg(point, 0);
        return { legs: [], replans: 0 };
      },
      flyChaosRouteLeg: async (_actor, point, options) => {
        const isEntry = point.x === entry.x && point.y === entry.y;
        drives.push({ point, tolerance: options.tolerance });
        m = { playerX: point.x - (isEntry ? 0 : options.tolerance - 0.000001), playerY: point.y, inSafeZone: true };
        return { reached: true };
      },
      console: { log() {} },
    });
    vm.runInContext(source.slice(start, end), ctx);
    const result = await ctx.stageChaosSafeZonePilot(actor);
    assert(authoredSafetyMargin(isSafe)(m.playerX, m.playerY) >= 16);
    assert(authoredSafetyMargin((x, y) => geometry.authoredHullIsTraversable(x, y))(m.playerX, m.playerY) >= 16);
    assert.equal(drives.at(-1).tolerance, 0.5, "the final safety pocket needs subpixel arrival");
    assert.equal(result.navigated, !alreadySafe, "already-safe pilots still centre without claiming a coarse route");
  }
});

test("actual safety steering completes on measured resting clearance, not arbitrary coordinate precision", async () => {
  const geometry = authoredMapGeometry(fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const source = fs.readFileSync(new URL("./keyboard_chord_matrix.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function flyChaosRouteLeg("), end = source.indexOf("\nif (!new Set", start);
  const isSafe = (x, y) => geometry.tileAt(x, y) === 171;
  let m = { playerX: 10088, playerY: 6952, playerVelocityX: 0, playerVelocityY: 0, inSafeZone: true };
  const actor = { metricsState: async () => ({ uiMetrics: m }) };
  const ctx = vm.createContext({ authoredRouteGeometry: geometry, chaosSafeMapBytes: Buffer.alloc(0),
    chaosAuthoredSafe: isSafe, authoredSafetyMargin, deepestStagingPoint, authoredExitHeading,
    runwayAlongHeading: () => ({ runwayPx: 1000 }), distanceToAuthoredSafety: () => Infinity,
    brakeVelocityDisposition, CHAOS_ROUTE_LEG_ARRIVAL_TOLERANCE_PX: 4,
    brakeClientVelocity: async () => ({ braked: true, speed: Math.hypot(m.playerVelocityX, m.playerVelocityY) }),
    chaosSafeRouteToAuthoredSafety: () => ({ stage: {}, stageTile: {}, hops: 0,
      waypoints: [], legLengths: [], drivePoints: [], safeRun: {} }),
    driveClientToWorldTarget: async (_actor, point, _budget, _settle, _tolerance, complete) => {
      assert.equal(typeof complete, "function");
      m = { ...m, playerX: point.x, playerY: point.y, playerVelocityX: 20 };
      assert.equal(await complete(), null, "a fast pass through the pocket cannot count as rest");
      m = { ...m, playerX: 10088, playerY: 6952, playerVelocityX: 0 };
      assert.equal(await complete(), null, "authored safety without the16px margin cannot pass");
      m = { ...m, playerX: point.x + 3.5, playerY: point.y, playerVelocityX: 2.4 };
      const completed = await complete();
      assert(completed && completed.safetyPx >= 16 && completed.hullPx >= 16);
      return { reached: true, completed, pulses: 1 };
    }, console: { log() {} },
  });
  vm.runInContext(source.slice(start, end), ctx);
  const result = await ctx.stageChaosSafeZonePilot(actor);
  assert.equal(result.failedLeg.distance, null, "measured completion must not invent exact coordinate arrival");
  assert.equal(result.failedLeg.completed.kind, "measured-safety-clearance");
});

test("authoredExitHeading rejects a clear corridor that never leaves safety, and an exit behind a wall", () => {
  // Safety is the whole upper half-plane, so thrusting north is clear forever and never exits - the exact
  // conflation that let the Chaos case be handed a north heading. South is the only heading that leaves.
  const lake = (x, y) => y >= 0;
  const open = () => true;
  const { chosen, candidates } = authoredExitHeading({ isSafe: lake, isHullClear: open, from: { x: 200, y: 100 } });
  // Headings are engine turns: (0, 1) increases y, which in this lake goes on forever inside safety.
  const neverExits = candidates.find((candidate) => candidate.heading.y === 1);
  assert.equal(neverExits.exitDistancePx, null);
  assert.ok(neverExits.hullClearPx >= 1024,
    "the corridor is genuinely clear, which is exactly why corridor length alone misleads a chooser");
  assert.equal(neverExits.usable, false);
  assert.deepEqual(chosen.heading, { x: 0, y: -1 }, "only the heading that actually leaves safety is usable");
  // Same lake, but the hull stops 56 px short of the boundary: the exit exists, and the ship cannot reach it.
  const walled = authoredExitHeading({ isSafe: lake, isHullClear: (x, y) => y >= 50, from: { x: 200, y: 100 } });
  assert.equal(walled.chosen, null, "an exit that sits beyond the hull's reach is not an escape route");
  assert.throws(() => authoredExitHeading({ isSafe: lake, from: { x: 200, y: 100 } }), /one surface/);
});

test("the shipped Chaos release positions face a heading that cannot exit and a west heading that can", () => {
  // Not a fixture: these are the two positions the release-instant instrument captured on failing runs, read
  // against the map the game ships. The case oriented north at both, where north never leaves safety inside
  // 1024 px. If the level ever changes shape, this is the assertion that notices.
  const geometry = authoredMapGeometry(fs.readFileSync(
    new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const isSafe = (x, y) => geometry.tileAt(x, y) === 171;
  const isHullClear = (x, y) => geometry.authoredHullIsTraversable(x, y);
  for (const from of [{ x: 13545.1, y: 7100.79 }, { x: 13556.82, y: 7075.07 }]) {
    const { chosen, candidates } = authoredExitHeading({ isSafe, isHullClear, from });
    const north = candidates.find((candidate) => candidate.heading.y === -1);
    assert.equal(north.exitDistancePx, null, `north must not exit safety near ${from.x},${from.y}`);
    assert.equal(north.usable, false);
    assert.ok(chosen !== null && chosen.exitDistancePx <= 64,
      `a usable exit within 64 px should exist at ${from.x},${from.y}, got ${JSON.stringify(chosen)}`);
    assert.ok(chosen.hullClearPx + 8 >= chosen.exitDistancePx);
  }
});
