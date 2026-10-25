import assert from "node:assert/strict";
import test from "node:test";
import path from "node:path";
import fs from "node:fs";
import vm from "node:vm";
import crypto from "node:crypto";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { loadRendezvousMap, planAuthoredApproach } from "./player_rendezvous.mjs";
import { planTimedPrizePreparation } from "./timed_prize_preparation.mjs";
import { planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";
import { parseHiddenPrizeSpawns } from "./combat_replay.mjs";

const loaded = loadRendezvousMap("chaos-zone", [path.resolve(new URL("../../assets/zones", import.meta.url).pathname)]);
const origins = [{ x: 7960, y: 6728 }, { x: 7752, y: 8856 }];

const warpSpawn = { collectibleId: 9, prizeId: 7, x: 8152, y: 8200, expiresAtTick: 5665 };
const warpOrigins = [{ x: 8898.05, y: 8213.63 }, { x: 8958.63, y: 8216 }];
const warpLog = "2000-01-01T15:10:09.811382Z DEBUG server hidden prize spawned x=8152 y=8200 collectible_id=9 prize_id=7 ttl_ticks=4499 expires_at_tick=5665\n";

test("retained Warp remains reachable after rendezvous despite a late spawn-log boundary", () => {
  const server = { logs: [warpLog, "2000-01-01T15:10:16.322000Z INFO rendezvous complete\n"] };
  assert.equal(parseHiddenPrizeSpawns(server, server.logs.join("").length).length, 0,
    "the old post-rendezvous boundary discards the still-live spawn");
  assert.deepEqual(parseHiddenPrizeSpawns(server, 0), [warpSpawn]);
  const prepared = planTimedPrizePreparation(loaded.geometry, warpOrigins, warpSpawn, 3121);
  assert(prepared);
  assert.equal(prepared.remainingTicks, 2544);
  assert(prepared.requiredTtlTicks <= prepared.remainingTicks);
  assert(Math.hypot(prepared.plan.target.x - warpSpawn.x, prepared.plan.target.y - warpSpawn.y) > 30);
  assert.equal(prepared.plan.hullRadiusPx, 20);
  assert.equal(prepared.plan.stageRadiusPx, 20);
  assert.equal(warpSpawn.expiresAtTick, 5665, "the real prize deadline is unchanged");
  assert.equal(planTimedPrizePreparation(loaded.geometry, warpOrigins, warpSpawn, 5666), null);
});

async function coordinateWarpPreparation(stale = null) {
  let staged = false;
  const metrics = [{ x: 8216, y: 8168 }, { x: 8216, y: 8200 }].map((p, i) => ({ playerX: p.x, playerY: p.y, selfPlayerId: i === 0 ? 1 : 0,
    playerLifeSeq: 1, latestSnapshotServerTick: 1188, localGameplayState: { brickTiles: [], activePrizeCollectibleIds: [9],
      authorityScopeServerSessionId: "session", authorityScopeArenaStreamId: String(i + 1) } }));
  const clients = metrics.map(m => ({ metricsState: async () => ({ uiMetrics: structuredClone(m) }),
    pageState: async () => ({ uiMetrics: structuredClone(m) }), evaluate: async () => "chaos-zone" }));
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const gateStart = source.indexOf("async function runWarpRelocationGate(");
  const start = source.indexOf("  const prizeScope =", gateStart);
  const end = source.indexOf("\n  const beforeConsumed =", start);
  assert(gateStart >= 0 && start > gateStart && end > start);
  const server = { logs: [warpLog] };
  const ctx = vm.createContext({ actor: clients[0], peer: clients[1], playerId: 1, server,
    prizeStreamLogOffset: 0, process: { env: {} }, path, repoRoot: path.resolve(new URL("../..", import.meta.url).pathname),
    loadRendezvousMap, planTimedPrizePreparation, parseHiddenPrizeSpawns,
    parsePrizeClaims: () => staged && stale === "claimed" ? [{ collectibleId: 9 }] : [],
    exactLiveGameplayScope: () => true,
    navigateAuthoredTarget: async (client, destination, _server, options) => {
      assert(staged); assert(options.timeoutMs > 0 && options.timeoutMs <= 90_000);
      const m = metrics[clients.indexOf(client)];
      m.playerX = destination.x; m.playerY = destination.y;
      if (stale === "launch" && client === clients[1]) m.playerY += 40;
      return { reached: true, keysOnly: true };
    },
    waitForPlayersReady: async () => { assert(staged, "mutual visibility is checked after prize-specific travel"); },
    waitUntil: async (_label, predicate, timeout) => { assert.equal(timeout, 12_000); assert.equal(await predicate(), true); },
    rendezvousPlayers: async (_actor, _peer, _server, options) => {
      assert.equal(options.stageRadiusPx, 20);
      assert(options.setupBudgetMs <= 90_000);
      assert(options.avoidTiles.some(p => p.x === 509 && p.y === 512));
      for (const m of metrics) { m.playerX = options.meetingPoint.x; m.playerY = options.meetingPoint.y; }
      staged = true;
      if (stale === "missing") metrics[0].localGameplayState.activePrizeCollectibleIds = [];
      if (stale === "expired") metrics[0].latestSnapshotServerTick = 5666;
      if (stale === "life") metrics[0].playerLifeSeq++;
      return { keysOnly: true, status: "complete" };
    },
  });
  return vm.runInContext(`(async()=>{${source.slice(start, end)};return prizePreparation;})()`, ctx);
}

test("actual Warp coordinator stages outside the live prize before holding Repel", async () => {
  const prepared = await coordinateWarpPreparation();
  assert.equal(prepared.spawn.collectibleId, 9);
  assert.equal(prepared.spawn.expiresAtTick, 5665);
  assert.equal(prepared.remainingTicksAfterSetup, 4477);
  assert.equal(prepared.encounter.keysOnly, true);
  assert(prepared.repelStaging.peer.keysOnly && prepared.repelStaging.actor.keysOnly);
  assert.equal(prepared.repelStaging.launch.missDistance, 0);
});
for (const stale of ["missing", "expired", "claimed", "life"]) {
  test(`actual Warp coordinator refuses a ${stale} prize or player after staging`, async () => {
    await assert.rejects(coordinateWarpPreparation(stale), /no longer live and unclaimed|lifecycle boundary/);
  });
}
test("actual Warp coordinator refuses a Repel line that would miss the pickup", async () => {
  await assert.rejects(coordinateWarpPreparation("launch"), /launch does not cross its pickup/);
});

test("Warp Repel preparation uses a hull-clear source line through the exact prize", () => {
  const initial = [{ x: 8216, y: 8168 }, { x: 8216, y: 8200 }];
  const p = planTimedPrizePreparation(loaded.geometry, initial, warpSpawn, 1188, {
    repelPickup: true, avoidTiles: [{ x: 509, y: 512 }],
  });
  assert(p && p.requiredTtlTicks <= p.remainingTicks);
  assert.deepEqual(p.plan.target, p.repelLaunch.actor);
  assert.equal(Math.hypot(p.repelLaunch.actor.x - warpSpawn.x, p.repelLaunch.actor.y - warpSpawn.y), 64);
  assert.equal(p.repelLaunch.source.routes[0].drivePoints.length, 1);
  assert.equal(p.repelLaunch.pickup.routes[0].drivePoints.length, 1);
  assert.equal(p.repelLaunch.source.routes[0].distancePx, 96);
  for (const r of [p.repelLaunch.source, p.repelLaunch.pickup]) {
    const from = r.routes[0].from, to = r.target;
    for (let i = 0; i <= 100; i++) assert(loaded.geometry.authoredHullIsTraversable(
      from.x + (to.x - from.x) * i / 100, from.y + (to.y - from.y) * i / 100, 20));
  }
  // Retained native A launched away from (8201,8164), with target(8199,8168).
  // That ray misses Warp(8152,8200) by27.7px, outside its22px pickup overlap.
  const dx = 8199 - 8201, dy = 8168 - 8164;
  assert(Math.abs(dx * (8200 - 8168) - dy * (8152 - 8199)) / Math.hypot(dx, dy) > 22);
  assert.throws(() => planTimedPrizePreparation(loaded.geometry, initial, warpSpawn, 1188,
    { repelPickup: true, shooterStandoffPx: 160 }), /conflicting/);
});

test("Warp pickup accepts only its exact native claim before skipping redundant navigation", async () => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const gate = source.indexOf("async function runWarpRelocationGate(");
  const start = source.indexOf("  let claim = null;", gate), end = source.indexOf("\n  let warpHeldStatus =", start);
  assert(start > gate && end > start);
  const exact = { playerId: 1, collectibleId: 9, prizeId: 7 };
  for (const claims of [[exact], [{ ...exact, playerId: 0 }], [{ ...exact, collectibleId: 8 }], [{ ...exact, prizeId: 8 }]]) {
    let navigationCalls = 0, brakingCalls = 0;
    const ctx = vm.createContext({ actor: {}, target: warpSpawn, server: {}, playerId: 1, logOffset: 0,
      prizePreparation: { preparation: { pickupAvoidTiles: [] } },
      parsePrizeClaims: () => claims,
      navigateAuthoredTarget: async () => { navigationCalls++; return { reached: false, completed: false }; },
      brakeClientVelocity: async (_actor, budget) => {
        assert(budget > 0 && budget <= 8_000); brakingCalls++; return { braked: true };
      },
    });
    const run = () => vm.runInContext(`(async()=>{${source.slice(start, end)};return {claim,route};})()`, ctx);
    if (claims[0] === exact) {
      const result = await run();
      assert.equal(result.route.contact, "native Repel flight");
      assert.deepEqual(result.claim, exact);
      assert.equal(navigationCalls, 0); assert.equal(brakingCalls, 1);
    } else {
      await assert.rejects(run(), /did not collect the natural Warp prize/);
      assert.equal(navigationCalls, 1); assert.equal(brakingCalls, 0);
    }
  }
});

test("queued Repel B uses a hull-clear normal-speed approach within its original20s bound", async () => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const gate = source.indexOf("async function runWarpRelocationGate(");
  const start = source.indexOf("  const actorBeforeSecondRepel =", gate);
  const end = source.indexOf('  await peer.press("q", 80);', start);
  assert(start > gate && end > start);
  const actorPosition = { x: 7082.12, y: 8200 };
  let route;
  const ctx = vm.createContext({ actor: { metricsState: async () => ({ uiMetrics: {
    playerX: actorPosition.x, playerY: actorPosition.y,
  } }) }, peer: {}, server: {}, navigateAuthoredTarget: async (_peer, target, _server, options) => {
    assert.equal(options.timeoutMs, 20_000);
    assert.equal(options.arrivalRadiusPx, 64);
    assert.equal(options.maximumDesiredSpeed, 325);
    assert.equal(options.desiredSpeedScale, 0.5);
    route = planAuthoredApproach(loaded.geometry, { x: 8310, y: 8200 }, target, options);
    const distance = route.routes[0].distancePx;
    assert(distance > 42 * options.timeoutMs / 1000, "the old collector cannot cover this path in20s");
    assert(distance < 325 * 5, "the new route fits ordinary-speed setup flight");
    assert(route.routes[0].drivePoints.every(p => loaded.geometry.authoredHullIsTraversable(p.x, p.y, 20)));
    assert(Math.hypot(route.target.x - target.x, route.target.y - target.y) <= 64);
    return { reached: true, route };
  } });
  const result = await vm.runInContext(`(async()=>{${source.slice(start, end)};return peerRoute;})()`, ctx);
  assert.equal(result.reached, true);
  assert(route);
});

test("only the three declared team compatibility lanes receive randomized Spawn fixtures", t => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("  const teamSpawnFixture ="), end = source.indexOf("  const warpSpawnFixture =", start);
  assert(start >= 0 && end > start);
  const select = (flags = {}) => {
    const ctx = vm.createContext({ semanticInputGate: false, baselineReceiptGate: false, prizePhaseOneGate: false,
      ...flags, arena: "chaos-zone", createSpawnRegionFixture });
    vm.runInContext(source.slice(start, end), ctx);
    return vm.runInContext("teamSpawnFixture", ctx);
  };
  assert.equal(select(), null, "ordinary replay cases keep their production spawn policy");
  const fixture = select({ prizePhaseOneGate: true });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  for (const flag of ["semanticInputGate", "baselineReceiptGate"]) {
    const other = select({ [flag]: true });
    t.after(() => fs.rmSync(other.root, { recursive: true, force: true }));
    assert.deepEqual(other.spawnRegion, fixture.spawnRegion);
    assert.equal(other.settingsSha256, fixture.settingsSha256);
    assert.equal(other.mapSha256, fixture.mapSha256);
  }
  assert.deepEqual(fixture.spawnRegion, { x: 512, y: 512, radius: 2 });
  const directory = path.join(fixture.root, "chaos-zone");
  const original = fs.readFileSync(path.join(loaded.provenance.directory, "arena.conf"), "utf8");
  const settings = fs.readFileSync(path.join(directory, "arena.conf"), "utf8");
  const withoutSpawn = text => text.replace(/^Team[0-3]-(?:X|Y|Radius)=.*$/gm, "<scenario spawn>");
  assert.equal(withoutSpawn(settings), withoutSpawn(original));
  assert.equal(settings.match(/^Team[0-3]-(?:X|Y|Radius)=/gm).length, 12);
  assert.equal(crypto.createHash("sha256").update(settings).digest("hex"), fixture.settingsSha256);
  const map = loaded.provenance.map;
  assert.deepEqual(fs.readFileSync(path.join(directory, map)), fs.readFileSync(path.join(loaded.provenance.directory, map)));
  assert.match(settings, /^RadarMode=4$/m, "retain the actual team/radar rules");
});

test("Warp receipt setup declares its pickup-clear origin without enabling legacy team mode", t => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("  const warpSpawnFixture ="), end = source.indexOf("  const portalSpawnFixture =", start);
  assert(start >= 0 && end > start);
  const select = enabled => {
    const ctx = vm.createContext({ warpRelocationGate: enabled, arena: "chaos-zone", createSpawnRegionFixture });
    vm.runInContext(source.slice(start, end), ctx);
    return vm.runInContext("warpSpawnFixture", ctx);
  };
  assert.equal(select(false), null);
  const fixture = select(true);
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  assert.deepEqual(fixture.spawnRegion, { x: 512, y: 512, radius: 0 });
  const original = fs.readFileSync(path.join(loaded.provenance.directory, "arena.conf"), "utf8");
  const settings = fs.readFileSync(path.join(fixture.root, "chaos-zone/arena.conf"), "utf8");
  const withoutSpawn = text => text.replace(/^Team[0-3]-(?:X|Y|Radius)=.*$/gm, "<scenario spawn>");
  assert.equal(withoutSpawn(settings), withoutSpawn(original));
  assert.match(settings, /^WarpRadiusLimit=176$/m, "the shipping random Warp range is preserved");
  const teamMode = source.slice(source.indexOf("  // These retained lanes exercise"),
    source.indexOf("  if (movementDeviationGate || warpRelocationGate"));
  assert(!teamMode.includes("warpRelocationGate"), "Warp must retain production FFA authority");
});

test("Warp bypasses generic travel but ordinary encounters retain it", async () => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("    if (movementCorrectionGate || warpRelocationGate || portalPhaseTwoGate || chaosRouteReplanGate) {", source.indexOf("const warpPrizeStreamLogOffset"));
  const end = source.indexOf("\n    const shipChangeEnergy =", start);
  assert(start >= 0 && end > start);
  for (const warp of [false, true]) {
    const calls = [];
    const ctx = vm.createContext({ movementCorrectionGate: false, chaosRouteReplanGate: false, warpRelocationGate: warp, portalPhaseTwoGate: false, alpha: {}, beta: {}, server: {},
      waitForLivePlayerScopes: async () => calls.push("live"),
      rendezvousPlayers: async () => calls.push("generic"),
      waitForPlayersReady: async () => calls.push("visible") });
    await vm.runInContext(`(async()=>{${source.slice(start, end)}})()`, ctx);
    assert.deepEqual(calls, warp ? ["live"] : ["generic", "visible"]);
  }
});

test("Portal declares only initial Spawn geometry and skips unrelated encounter travel", async t => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("  const portalSpawnFixture ="), end = source.indexOf("  const aoiSpawnFixture =", start);
  assert(start >= 0 && end > start);
  const ctx = vm.createContext({ portalPhaseTwoGate: false, snapshotLossGate: false, arena: "chaos-zone", createSpawnRegionFixture });
  vm.runInContext(source.slice(start, end), ctx);
  assert.equal(vm.runInContext("portalSpawnFixture", ctx), null);
  const active = vm.createContext({ portalPhaseTwoGate: true, snapshotLossGate: false, arena: "chaos-zone", createSpawnRegionFixture });
  vm.runInContext(source.slice(start, end), active);
  const fixture = vm.runInContext("portalSpawnFixture", active);
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  assert.deepEqual(fixture.spawnRegion, { x: 512, y: 512, radius: 0 });
  const withoutSpawn = text => text.replace(/^Team[0-3]-(?:X|Y|Radius)=.*$/gm, "<scenario spawn>");
  assert.equal(withoutSpawn(fs.readFileSync(path.join(fixture.root, "chaos-zone/arena.conf"), "utf8")),
    withoutSpawn(fs.readFileSync(path.join(loaded.provenance.directory, "arena.conf"), "utf8")));
  const begin = source.indexOf("    if (movementCorrectionGate || warpRelocationGate || portalPhaseTwoGate || chaosRouteReplanGate) {", source.indexOf("const portalPrizeStreamLogOffset"));
  const finish = source.indexOf("\n    const shipChangeEnergy =", begin);
  const calls = [];
  const dispatch = vm.createContext({ movementCorrectionGate: false, chaosRouteReplanGate: false, warpRelocationGate: false,
    portalPhaseTwoGate: true, alpha: {}, beta: {}, server: {},
    waitForLivePlayerScopes: async () => calls.push("live"),
    rendezvousPlayers: async () => { throw new Error("unrelated encounter must not consume Portal TTL"); },
  });
  await vm.runInContext(`(async()=>{${source.slice(begin, finish)}})()`, dispatch);
  assert.deepEqual(calls, ["live"]);
});

for (const stale of [null, "expired", "claimed", "bricks"]) {
  test(`actual Portal pickup coordinator ${stale ? `refuses ${stale} setup` : "preserves the exact natural identity and15s bound"}`, async () => {
    const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
    const gate = source.indexOf("async function runPortalPhaseTwoGate(");
    const start = source.indexOf('  const arena = await actor.evaluate("window.__zubspaceArena");', gate);
    const end = source.indexOf('  await waitUntil("natural Portal acquisition reaches private client inventory"', start);
    assert(start > gate && end > start);
    const spawn = { x: 8200, y: 8232, collectibleId: 6, prizeId: 28, expiresAtTick: 6808 };
    const server = { logs: ["2000-01-01T16:06:17.353107Z DEBUG server hidden prize spawned x=8200 y=8232 collectible_id=6 prize_id=28 ttl_ticks=5642 expires_at_tick=6808\n"] };
    assert.equal(parseHiddenPrizeSpawns(server, server.logs[0].length).length, 0);
    const metrics = { playerX: 8216, playerY: 8200, latestSnapshotServerTick: stale === "expired" ? 6809 : 1200,
      localGameplayState: { brickTiles: [], brickTilesOverflow: stale === "bricks" } };
    let collected = stale === "claimed", navigationCalls = 0;
    const ctx = vm.createContext({ actor: { evaluate: async () => "chaos-zone", metricsState: async () => ({ uiMetrics: metrics }) },
      server, playerId: 1, logOffset: 0, path, process: { env: {} }, repoRoot: path.resolve(new URL("../..", import.meta.url).pathname),
      loadRendezvousMap, planNaturalPrizeNavigation, parseHiddenPrizeSpawns, exactLiveGameplayScope: () => true,
      parsePrizeClaims: () => collected ? [{ playerId: 1, ...spawn }] : [],
      waitUntil: async (_label, predicate, timeout) => { assert.equal(timeout, 15_000); if (!await predicate()) throw new Error("no qualified Portal"); },
      navigateAuthoredTarget: async (_actor, target, _server, options) => {
        navigationCalls++; assert.deepEqual(target, spawn);
        assert.equal(options.timeoutMs, 15_000); assert.equal(options.arrivalRadiusPx, 16);
        collected = true; assert(await options.complete()); return { reached: false, completed: true };
      },
    });
    const run = () => vm.runInContext(`(async()=>{${source.slice(start, end)};return {target,claim,route};})()`, ctx);
    if (stale) { await assert.rejects(run(), /no qualified Portal|incomplete Portal pickup brick geometry/); assert.equal(navigationCalls, 0); }
    else { const result = await run(); assert.equal(result.claim.collectibleId, 6); assert.equal(result.target.expiresAtTick, 6808); assert.equal(navigationCalls, 1); }
  });
}

test("the second retained random-spawn pair cannot reach Warp within its actual lifetime", () => {
  const initial = [{ x: 9448, y: 8200 }, { x: 7032, y: 9448 }];
  const prize = { ...warpSpawn, expiresAtTick: 5687 };
  assert.equal(planTimedPrizePreparation(loaded.geometry, initial, prize, 1188), null);
  // A native planning-only long deadline reveals the required detour. The
  // browser never receives this synthetic lifetime or altered collectible.
  const measured = planTimedPrizePreparation(loaded.geometry, initial, { ...prize, expiresAtTick: 10_000 }, 1188);
  assert.equal(measured.requiredTtlTicks, 7882);
  assert(measured.plan.routes[1].distancePx > 4008);
  assert.equal(planTimedPrizePreparation(loaded.geometry,
    [{ x: 9150.68, y: 9201.96 }, { x: 9190.6, y: 9242.21 }], prize, 5518), null);
  for (const origins of [
    [{ x: 8168, y: 8168 }, { x: 8232, y: 8232 }],
    [{ x: 8168, y: 8232 }, { x: 8232, y: 8168 }],
  ]) {
    const p = planTimedPrizePreparation(loaded.geometry, origins, prize, 1188);
    assert(p && p.requiredTtlTicks < 4499);
    assert(p.plan.routes.every(r => r.distancePx < 200));
  }
});

test("the retained outer-quadrant pair cannot be mislabeled a feasible central timed-prize route", () => {
  assert.throws(() => planTimedPrizePreparation(loaded.geometry,
    [{ x: 11230.61, y: 104.67 }, { x: 11324.3, y: 104 }],
    { collectibleId: 8, prizeId: 18, x: 8296, y: 8184, expiresAtTick: 8875 }, 1000),
  /no hull-clear authored approach route/);
});
for (const prize of [
  { collectibleId: 8, prizeId: 18, x: 8296, y: 8184, expiresAtTick: 8875 },
  { collectibleId: 5, prizeId: -14, x: 8136, y: 8280, expiresAtTick: 31511 },
]) {
  test(`both distant pilots prepare outside retained collectible ${prize.collectibleId} without changing its TTL`, () => {
    const original = structuredClone(prize);
    const prepared = planTimedPrizePreparation(loaded.geometry, origins, prize, 1000,
      { shooterStandoffPx: prize.prizeId === -14 ? 160 : null });
    assert(prepared);
    assert.deepEqual(prize, original);
    const distance = Math.hypot(prepared.plan.target.x - prize.x, prepared.plan.target.y - prize.y);
    assert(distance > 30 && distance <= 64, "prepared hull must be outside the actual22px pickup range");
    assert(prepared.plan.routes.every(route => route.distancePx > 500));
    assert(prepared.remainingTicks >= prepared.requiredTtlTicks);
    assert(prepared.setupBudgetMs <= 90_000);
    if (prize.prizeId === -14) {
      assert.equal(prepared.firingLine.routes[0].drivePoints.length, 1);
      assert.equal(prepared.firingLine.routes[0].distancePx, 160);
      assert.deepEqual(prepared.shooterRoute.target, prepared.shooterStandoff);
      assert(Math.hypot(prepared.shooterStandoff.x - prize.x, prepared.shooterStandoff.y - prize.y) >= 144);
    }
    for (const route of prepared.plan.routes) {
      let previous = route.from;
      for (const point of route.drivePoints) {
        const length = Math.hypot(point.x - previous.x, point.y - previous.y);
        for (let step = 0; step <= Math.ceil(length); step++) {
          const f = step / Math.max(1, Math.ceil(length));
          const x = previous.x + (point.x - previous.x) * f;
          const y = previous.y + (point.y - previous.y) * f;
          assert(loaded.geometry.authoredHullIsTraversable(x, y, 20));
          assert(Math.hypot(x - prize.x, y - prize.y) > 30);
        }
        previous = point;
      }
      assert.deepEqual(previous, prepared.plan.target);
    }
    assert.equal(planTimedPrizePreparation(loaded.geometry, origins, { ...prize, expiresAtTick: 1100 }, 1000), null,
      "a short remaining TTL cannot be rescued by moving or extending the prize");
  });
}

// Execute the actual pre-claim coordinator with a virtual browser adapter.
// This checks stale-prize guards and stage orchestration, not key delivery or
// gameplay physics; the real focused replay remains required.
async function coordinatePreparation({ stale = null } = {}) {
  const spawns = [
    { collectibleId: 8, prizeId: 18, x: 8296, y: 8184, expiresAtTick: 8875 },
    { collectibleId: 5, prizeId: -14, x: 8136, y: 8280, expiresAtTick: 31511 },
  ];
  const metrics = origins.map((origin, index) => ({ playerX: origin.x, playerY: origin.y,
    selfPlayerId: index, playerLifeSeq: 2, latestSnapshotServerTick: 1000,
    localGameplayState: { activePrizeCollectibleIds: [8, 5], brickTiles: [] } }));
  let staged = false, options;
  const clients = metrics.map(m => ({ metricsState: async () => ({ uiMetrics: structuredClone(m) }) }));
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("  const prepareTimedPrize = async"), end = source.indexOf("\n  const shieldPreparation =", start);
  assert(start >= 0 && end > start);
  const ctx = vm.createContext({ loadedMap: loaded, actor: clients[0], peer: clients[1], server: {}, logOffset: 0,
    planTimedPrizePreparation, exactLiveGameplayScope: () => true,
    parseHiddenPrizeSpawns: () => structuredClone(spawns),
    parsePrizeClaims: () => staged && stale === "claimed" ? [{ collectibleId: 8 }] : [],
    waitUntil: async (_label, predicate) => { assert.equal(await predicate(), true, "live fixture must select a feasible prize"); },
    rendezvousPlayers: async (_actor, _peer, _server, requested) => {
      options = structuredClone(requested); staged = true;
      for (const m of metrics) { m.playerX = requested.meetingPoint.x; m.playerY = requested.meetingPoint.y; }
      if (stale === "expired") metrics[0].latestSnapshotServerTick = 8876;
      if (stale === "missing") metrics[0].localGameplayState.activePrizeCollectibleIds = [5];
      return { keysOnly: true, status: "complete" };
    },
  });
  vm.runInContext(source.slice(start, end), ctx);
  try { return { result: await vm.runInContext("prepareTimedPrize(18)", ctx), options }; }
  catch (error) { return { error, options }; }
}

test("actual timed-prize coordinator stages the exact live collectible before claiming", async () => {
  const { result, error, options } = await coordinatePreparation();
  assert.ifError(error);
  assert.equal(result.spawn.collectibleId, 8);
  assert.equal(result.encounter.keysOnly, true);
  assert.equal(options.stageRadiusPx, 20);
  assert.deepEqual(options.meetingPoint, result.preparation.plan.target);
  assert(options.avoidTiles.some(tile => tile.x === 518 && tile.y === 511));
  assert(result.remainingTicksAfterSetup > 0);
});

test("actual timed-prize coordinator rejects expiry, disappearance or a claim during setup", async () => {
  for (const stale of ["expired", "missing", "claimed"]) {
    const { result, error } = await coordinatePreparation({ stale });
    assert.equal(result, undefined);
    assert.match(error.message, /no longer live and unclaimed/);
  }
});


test("retained Warp pickup reserves the full Repel runout instead of entering a dynamic door", () => {
  const initial = [{ x: 8200, y: 8200 }, { x: 8200, y: 8200 }];
  // Original westbound launch reaches the door margin1078px into its1125px impulse.
  assert.equal(loaded.geometry.authoredHullIsTraversable(7138, 8200, 20), false);
  const p = planTimedPrizePreparation(loaded.geometry, initial, warpSpawn, 1188, {
    repelPickup: true, avoidTiles: [{ x: 509, y: 512 }],
  });
  assert.ok(p);
  assert.equal(p.repelLaunch.clearRunoutPx, 1500);
  assert.deepEqual(p.repelLaunch.actor, { x: 8088, y: 8200 });
  for (let distance = 0; distance <= 1500; distance += 4)
    assert.equal(loaded.geometry.authoredHullIsTraversable(8088 + distance, 8200, 20), true);
  const boundedGeometry = { ...loaded.geometry, authoredHullIsTraversable: (x, y, radius) =>
    Math.abs(x - warpSpawn.x) < 700 && Math.abs(y - warpSpawn.y) < 700 &&
    loaded.geometry.authoredHullIsTraversable(x, y, radius) };
  assert.throws(() => planTimedPrizePreparation(boundedGeometry, initial, warpSpawn, 1188,
    { repelPickup: true, avoidTiles: [{ x: 509, y: 512 }] }), /no conservative timed-prize Repel launch/);
});
