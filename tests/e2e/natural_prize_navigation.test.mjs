import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { authoredMapGeometry } from "./authored_map_geometry.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";

const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);

test("the retained randomized Chaos spawn reaches the same natural Brick around terrain", () => {
  const { geometry } = loadRendezvousMap("chaos-zone", [root]);
  const origin = { x: 8488, y: 10152 };
  const prize = { x: 8248, y: 8136, collectibleId: 3, expiresAtTick: 8338 };
  const original = structuredClone(prize);
  const navigation = planNaturalPrizeNavigation(geometry, origin, prize, 1182);
  assert(navigation);
  assert.deepEqual(prize, original, "selection must not alter the natural prize or its lifetime");
  assert.equal(geometry.authoredHullIsTraversable(prize.x, prize.y, 14), false,
    "the old centre destination is not a valid ship centre");
  const { plan } = navigation;
  assert(plan.routes[0].distancePx > 3000);
  assert(plan.routes[0].distancePx > Math.hypot(prize.x - origin.x, prize.y - origin.y) * 1.5);
  assert(navigation.setupBudgetMs <= 90_000);
  assert(navigation.requiredTtlTicks <= navigation.remainingTicks);
  let prior = origin;
  for (const point of plan.routes[0].drivePoints) {
    const distance = Math.hypot(point.x - prior.x, point.y - prior.y);
    for (let step = 0; step <= Math.ceil(distance); step++) {
      const fraction = step / Math.max(1, Math.ceil(distance));
      assert(geometry.authoredHullIsTraversable(prior.x + (point.x - prior.x) * fraction,
        prior.y + (point.y - prior.y) * fraction, plan.hullRadiusPx));
    }
    prior = point;
  }
  // Match the real client/server AABB pickup rule with the entire3px final
  // driver tolerance, rather than treating endpoint arrival as a claim.
  for (const dx of [-3, 3]) for (const dy of [-3, 3]) {
    const x = plan.target.x + dx, y = plan.target.y + dy;
    assert(x + 14 >= prize.x - 8 && x - 14 < prize.x + 8);
    assert(y + 14 >= prize.y - 8 && y - 14 < prize.y + 8);
  }
});

test("TTL admission counts the terrain detour rather than only straight-line distance", () => {
  const { geometry } = loadRendezvousMap("chaos-zone", [root]);
  const origin = { x: 8488, y: 10152 };
  const prize = { x: 8248, y: 8136, expiresAtTick: 6000 };
  const straightLineTicks = Math.ceil(Math.hypot(prize.x - origin.x, prize.y - origin.y) / 60 * 100) + 800;
  assert(straightLineTicks < prize.expiresAtTick - 1182);
  assert.equal(planNaturalPrizeNavigation(geometry, origin, prize, 1182), null);
});

test("expired, half-range-ambiguous and too-short lived prizes cannot become targets", () => {
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  const origin = { x: 8200, y: 8200 };
  for (const expiresAtTick of [1000, 999, 1000 + 0x40000000, 2199]) {
    assert.equal(planNaturalPrizeNavigation(geometry, origin,
      { x: 8248, y: 8200, expiresAtTick }, 1000), null);
  }
  const wrapped = planNaturalPrizeNavigation(geometry, origin,
    { x: 8248, y: 8200, expiresAtTick: 1200 }, 0x7fffffff);
  assert(wrapped);
  assert.equal(wrapped.remainingTicks, 1201);
  assert.equal(wrapped.requiredTtlTicks, 1200);
  assert.throws(() => planNaturalPrizeNavigation(geometry, origin,
    { x: 8248, y: 8200, expiresAtTick: null }, 1000), /exact lifetime ticks/);
});

test("a long route cannot borrow an unlimited setup budget from a long-lived prize", () => {
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  assert.equal(planNaturalPrizeNavigation(geometry, { x: 8200, y: 8200 },
    { x: 14008, y: 8200, expiresAtTick: 30_000 }, 1000), null);
});

test("preclaim staging stays on the selected pickup side of the retained Chaos wall", async () => {
  const { naturalPrizePreclaimStage } = await import("./natural_prize_navigation.mjs");
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const { geometry } = loadRendezvousMap("chaos-zone", [root]);
  const origin = { x: 9832, y: 6952 }, prize = { x: 8248, y: 8136, expiresAtTick: 8333 };
  const navigation = planNaturalPrizeNavigation(geometry, origin, prize, 1400);
  const original = structuredClone(navigation);
  const stage = naturalPrizePreclaimStage(geometry, navigation);
  assert.deepEqual(stage, { x: 8296, y: 8152 });
  const fine = planAuthoredApproach(geometry, stage, prize, { arrivalRadiusPx: 16 });
  assert.equal(fine.routes[0].drivePoints.length, 1);
  assert.equal(fine.routes[0].distancePx, 48);
  const oldCoarse = planAuthoredApproach(geometry, origin, prize, { arrivalRadiusPx: 64 });
  const oldFine = planAuthoredApproach(geometry, oldCoarse.target, prize, { arrivalRadiusPx: 16 });
  assert.equal(oldFine.routes[0].drivePoints.length, 3);
  assert(oldFine.routes[0].distancePx > fine.routes[0].distancePx * 2);
  assert.deepEqual(navigation, original, "staging cannot move or prolong the selected collectible");
});

test("preclaim staging skips only a genuinely short final approach, including single-leg routes", async () => {
  const { naturalPrizePreclaimStage } = await import("./natural_prize_navigation.mjs");
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  for (const origin of [{ x: 8200, y: 8200 }, { x: 8008, y: 8200 }]) {
    const navigation = planNaturalPrizeNavigation(geometry, origin,
      { x: 8248, y: 8200, expiresAtTick: 12_000 }, 1000);
    assert(navigation);
    const stage = naturalPrizePreclaimStage(geometry, navigation);
    if (origin.x === 8200) assert.equal(stage, null);
    else {
      assert(stage);
      assert(Math.hypot(stage.x - 8248, stage.y - 8200) <= 64);
      assert(Math.hypot(stage.x - 8248, stage.y - 8200) > 22 + 3);
    }
  }
});

test("long routes stage near pickup on the selected approach side", async () => {
  const { naturalPrizePreclaimStage } = await import("./natural_prize_navigation.mjs");
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  const navigation = planNaturalPrizeNavigation(geometry, { x: 6200, y: 8200 },
    { x: 8248, y: 8200, expiresAtTick: 12_000 }, 1000);
  const stage = naturalPrizePreclaimStage(geometry, navigation);
  assert(stage);
  assert(geometry.authoredHullIsTraversable(stage.x, stage.y, 20));
  assert(Math.hypot(stage.x - 8248, stage.y - 8200) <= 64);
  assert(Math.hypot(stage.x - 8248, stage.y - 8200) > 22 + 3,
    "coarse staging must leave the target unclaimed with endpoint tolerance");
  assert(Math.hypot(stage.x - navigation.plan.target.x, stage.y - navigation.plan.target.y) <= navigation.plan.maxLegPx);
});

test("the later lossy Chaos route collects the intervening Repel before its Brick baseline", async () => {
  const { naturalPrizePreclaimStage } = await import("./natural_prize_navigation.mjs");
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const { geometry } = loadRendezvousMap("chaos-zone", [root]);
  const prize = { x: 8248, y: 8136, expiresAtTick: 8333 };
  const navigation = planNaturalPrizeNavigation(geometry, { x: 9128, y: 9544 }, prize, 1400);
  const stage = naturalPrizePreclaimStage(geometry, navigation);
  assert.deepEqual(stage, { x: 8248, y: 8200 });
  const fine = planAuthoredApproach(geometry, stage, prize, { arrivalRadiusPx: 16 });
  assert.equal(fine.routes[0].distancePx, 48);
  // Actual incidental Repel6 at(8264,8200). Coarse arrival overlaps it while
  // remaining well outside Brick3, so it precedes the exact +1 baseline.
  assert(Math.abs(stage.x - 8264) < 22 && Math.abs(stage.y - 8200) < 22);
  assert(Math.hypot(stage.x - prize.x, stage.y - prize.y) > 22 + 3);
});

test("the retained Firefox Extreme single-leg branch stages before its exact pickup baseline", async () => {
  const { naturalPrizePreclaimStage } = await import("./natural_prize_navigation.mjs");
  const { geometry } = loadRendezvousMap("extreme", [root]);
  const navigation = planNaturalPrizeNavigation(geometry, { x: 8168, y: 8168 },
    { x: 8280, y: 7624, expiresAtTick: 6536 }, 1200);
  assert.equal(navigation.plan.routes[0].drivePoints.length, 1);
  const stage = naturalPrizePreclaimStage(geometry, navigation);
  assert(stage);
  assert(geometry.authoredHullIsTraversable(stage.x, stage.y, 20));
  const distance = Math.hypot(stage.x - 8280, stage.y - 7624);
  assert(distance <= 64 && distance > 22 + 3);
});


test("the retained682px direct Firefox Chaos route stages after incidental prizes and before the Brick baseline", async () => {
  const { naturalPrizePreclaimStage } = await import("./natural_prize_navigation.mjs");
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const { geometry } = loadRendezvousMap("chaos-zone", [root]);
  const origin = { x: 7816, y: 8680 }, prize = { x: 8248, y: 8136, expiresAtTick: 8553 };
  const navigation = planNaturalPrizeNavigation(geometry, origin, prize, 1200);
  assert.equal(navigation.plan.routes[0].drivePoints.length, 1);
  const stage = naturalPrizePreclaimStage(geometry, navigation);
  assert(stage, "a direct route still crosses unrelated natural greens before the target");
  assert(Math.hypot(stage.x - prize.x, stage.y - prize.y) > 22 + 3);
  const fine = planAuthoredApproach(geometry, stage, prize, { arrivalRadiusPx: 16 });
  assert(fine.routes[0].distancePx < 64);
  // Retained incidental Cloak5 is at(8200,8232),107px from Brick3.
  // The short measured pickup segment must no longer cross its overlap box.
  let previous = stage;
  for (const point of fine.routes[0].drivePoints) {
    const distance = Math.hypot(point.x - previous.x, point.y - previous.y);
    for (let i = 0; i <= Math.ceil(distance); i++) {
      const t = i / Math.max(1, Math.ceil(distance));
      const x = previous.x + (point.x - previous.x) * t, y = previous.y + (point.y - previous.y) * t;
      assert(Math.abs(x - 8200) >= 22 || Math.abs(y - 8232) >= 22);
      assert(geometry.authoredHullIsTraversable(x, y, 20));
    }
    previous = point;
  }
});
