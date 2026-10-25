import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { authoredMapGeometry } from "./authored_map_geometry.mjs";
import { uninterruptedMotionStage } from "./motion_staging.mjs";

test("Chaos motion staging avoids the real door row reached by the failed trace", () => {
  const geometry = authoredMapGeometry(fs.readFileSync(new URL(
    "../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url)));
  const stage = uninterruptedMotionStage(geometry, [{ x: 8200, y: 8200 }, { x: 8200, y: 8200 }]);
  assert.equal(geometry.tileAt(8150, 8120) >= 162 && geometry.tileAt(8150, 8120) <= 169, true);
  assert.notDeepEqual(stage, { x: 8200, y: 8200, marginPx: 384 });
  assert(geometry.authoredHullIsTraversable(stage.x, stage.y, stage.marginPx));
  assert(geometry.authoredLineIsClear(8200, 8200, stage.x, stage.y));
  assert(stage.y - stage.marginPx >= 8128, "the full measurement area stays below the authored door row");
});

test("motion staging fails rather than borrowing blocked or safe terrain", () => {
  const open = { authoredHullIsTraversable: () => true, authoredLineIsClear: () => true, tileAt: () => 0 };
  assert.throws(() => uninterruptedMotionStage(open, [{ x: NaN, y: 0 }]), /finite pilot/);
  for (const geometry of [
    { ...open, authoredHullIsTraversable: () => false },
    { ...open, authoredLineIsClear: () => false },
    { ...open, tileAt: () => 171 },
    { ...open, tileAt: () => 162 },
  ]) assert.throws(() => uninterruptedMotionStage(geometry, [{ x: 0, y: 0 }],
    { marginPx: 16, searchPx: 16 }), /no authored/);
});
