import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { brickWeaponStage } from "./brick_weapon_stage.mjs";

const wall = (x, y) => Array.from({ length: 10 }, (_, i) => ({ x: x - 4 + i, y }));

test("the retained row511 failure is rejected before driving a shooter into safety", t => {
  const fixture = createWeaponLifecycleFixture();
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const { geometry } = loadRendezvousMap("chaos-zone", [fixture.root]);
  assert.equal(geometry.tileAt(8205, 8119), 171, "actual failed owner pose is a safety tile");
  assert.throws(() => brickWeaponStage(geometry, wall(510, 511)), /intersects authored safety/);
});

test("every authored brick-weapon spawn has the original firing clearances outside safety", t => {
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 514, radius: 2 } });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const { geometry } = loadRendezvousMap("chaos-zone", [fixture.root]);
  for (let x = 510; x < 514; x++) for (let sourceY = 512; sourceY < 516; sourceY++) {
    const stage = brickWeaponStage(geometry, wall(x, sourceY + 1));
    assert.equal(stage.wallTop - stage.gunner.y, 48);
    assert.equal(stage.hostile.y - stage.wallBottom, 96);
    assert(stage.wallTop - stage.thorTarget.y >= 144);
    for (const point of [stage.gunner, stage.hostile, stage.thorTarget]) {
      assert.equal(geometry.tileAt(point.x, point.y), 0);
      assert(geometry.authoredHullIsTraversable(point.x, point.y, 32));
    }
  }
});

test("a malformed or edge-aligned wall cannot certify a firing stage", () => {
  const geometry = { authoredHullIsTraversable: () => true, tileAt: () => 0 };
  assert.throws(() => brickWeaponStage(geometry, wall(512, 513).slice(1)), /ten-tile/);
  assert.throws(() => brickWeaponStage(geometry, [...wall(512, 513).slice(1), { x: 509, y: 513 }]), /ten-tile/);
  assert.throws(() => brickWeaponStage(geometry, wall(512, 513), 8128), /interior firing column/);
});
