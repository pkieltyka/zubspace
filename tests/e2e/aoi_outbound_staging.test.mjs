import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import { aoiOutboundProgress, stageAoiWatcher } from "./aoi_outbound_staging.mjs";
import { authoredMapGeometry } from "./authored_map_geometry.mjs";

const ship = (x, y = 8200, vx = 0) => ({
  playerX: x, playerY: y, playerAlive: true, playerVelocityX: vx, playerVelocityY: 0,
});

test("the real map rejects the captured random corridor and admits the declared central runway", () => {
  const map = fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url));
  const geometry = authoredMapGeometry(map);
  const open = (origin, dx, dy, length) => {
    for (let distance = 0; distance <= length; distance += 2) {
      if (!geometry.authoredHullIsTraversable(origin.x + dx * distance, origin.y + dy * distance, 14)) return false;
    }
    return true;
  };
  const failed = { x: 8279.55, y: 7254.24 };
  const central = { x: 8200, y: 8200 };
  const dx = Math.sin(0.28 * 2 * Math.PI), dy = -Math.cos(0.28 * 2 * Math.PI);
  assert.equal(open(failed, -1, 0, 450), false, "the old watcher origin cannot supply the required west leg");
  assert.equal(open(failed, dx, dy, 2816), false, "its old outbound corridor also meets terrain before Far");
  assert.equal(open(central, -1, 0, 500), true);
  assert.equal(open(central, dx, dy, 3100), true);
});

test("the retained 1140px staging failure is rejected before starting the outbound key hold", () => {
  assert.throws(() => aoiOutboundProgress(ship(8200), ship(7060.13, 8193.90)), /exhausted the Near motion runway/);
});

test("outbound completion requires a live actor's own travel and crossing the brake distance", () => {
  const leg = aoiOutboundProgress(ship(8200), ship(7700));
  assert.equal(leg.reached(ship(8200)), false);
  assert.equal(leg.reached(ship(8456)), false); // Travel alone is insufficient.
  assert.equal(leg.reached(ship(8600)), false); // Exact radius has not crossed.
  assert.equal(leg.reached(ship(8601)), true);
  assert.equal(leg.reached({ ...ship(8601), playerAlive: false }), false);
  assert.deepEqual(leg.measure(ship(8601)), { actorTravel: 401, watcherDistance: 901 });
  // Moving the caller's watcher sample cannot manufacture actor travel.
  const watcher = ship(7700);
  const fixed = aoiOutboundProgress(ship(8200), watcher);
  watcher.playerX = 6500;
  assert.equal(fixed.reached(ship(8200)), false);
});

test("staging preserves 256px runway and rejects missing geometry", () => {
  assert.equal(aoiOutboundProgress(ship(8200), ship(7556)).initialDistance, 644);
  assert.throws(() => aoiOutboundProgress(ship(8200), ship(7555)), /Near motion runway/);
  assert.throws(() => aoiOutboundProgress(ship(NaN), ship(7700)), /finite positions/);
  assert.throws(() => aoiOutboundProgress(ship(8200), {}), /finite positions/);
});

test("watcher staging brakes speed-limited legs and leaves both Near runway and a westward far-exit offset", async () => {
  let metrics = ship(8200);
  let aligns = 0;
  let brakes = 0;
  const result = await stageAoiWatcher({
    initial: metrics,
    alignWest: async () => { aligns += 1; },
    holdUntil: async (predicate) => {
      assert.equal(predicate({ uiMetrics: metrics }), false);
      assert.equal(predicate({ uiMetrics: { ...metrics, playerVelocityX: -49 } }), false);
      assert.equal(predicate({ uiMetrics: { ...metrics, playerVelocityX: -50 } }), true);
    },
    brake: async () => {
      brakes += 1;
      // Include cruise and braking drift, rather than pretending key-up stops motion.
      metrics = ship(metrics.playerX - 100);
      return { braked: true, metrics, speed: 0 };
    },
  });
  assert.equal(aligns, 5);
  assert.equal(brakes, 5);
  assert.equal(result.legs.length, 5);
  assert.equal(result.metrics.playerX, 7700);
  assert.equal(aoiOutboundProgress(ship(8200), result.metrics).initialDistance, 500);
  assert.ok(2600 + (8200 - result.metrics.playerX) > 2816, "east wall still lies beyond the AOI exit");
});

test("watcher staging rejects unsuccessful brakes and bounded attempts cannot spin at a wall", async () => {
  const callbacks = { initial: ship(8200), alignWest: async () => {}, holdUntil: async () => {} };
  await assert.rejects(stageAoiWatcher({ ...callbacks, brake: async () => ({ braked: false }) }), /could not stop/);
  await assert.rejects(stageAoiWatcher({
    ...callbacks,
    brake: async () => ({ braked: true, metrics: ship(8200), speed: 0 }),
  }), /insufficient westward staging progress/);
});
