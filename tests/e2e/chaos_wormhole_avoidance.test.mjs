// The Chaos wormhole's gravity field is ~1225 px across and the safe-zone route planner used to fly straight
// through it. Measured: a route leg from (11250,2500) aimed the driver at (10120,4040), 459 px from
// the wormhole anchor; the field accelerated the pilot into the anchor, the wormhole relocated it ~4.1 km with
// the velocity zeroed, and the leg then needed ~110 s to fly the 4.4 km its 60 s budget allowed. That made
// `make test-e2e-immediate-keyboard-high-refresh` fail 2/2 in isolation.
//
// These tests pin the field model, pin the gravity constant against the shipped arena settings, and keep the
// old behaviour reachable (`wormholeAvoidRadiusPx: 0`) so the comparison cannot quietly stop proving anything.
import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { authoredMapGeometry, wormholeInfluenceRadiusPx } from "./authored_map_geometry.mjs";
import {
  CHAOS_WORMHOLE_AVOID_RADIUS_PX,
  CHAOS_WORMHOLE_CLEARANCE_PX,
  CHAOS_WORMHOLE_FIELD_AVOID_PX,
  CHAOS_WORMHOLE_GRAVITY,
  chaosSafeRouteToAuthoredSafety,
} from "./chaos_safe_route.mjs";

const chaosBytes = () =>
  fs.readFileSync(new URL("../../assets/zones/chaos-zone/bdhadar.lvl", import.meta.url));
const chaosArenaConf = () =>
  fs.readFileSync(new URL("../../assets/zones/chaos-zone/arena.conf", import.meta.url), "utf8");

// Distance from a point to the wormhole's authored 5x5 footprint, measured here rather than asked of the
// production mask: a test that validates the mask with the mask would pass while both were wrong.
function distanceToWormholeField(x, y, anchors) {
  let nearest = Number.POSITIVE_INFINITY;
  for (const anchor of anchors) {
    const minX = anchor.tileX * 16;
    const minY = anchor.tileY * 16;
    nearest = Math.min(nearest, Math.hypot(
      Math.max(minX - x, 0, x - (minX + 5 * 16)),
      Math.max(minY - y, 0, y - (minY + 5 * 16)),
    ));
  }
  return nearest;
}

test("the Chaos wormhole gravity pin still matches the shipped arena settings", () => {
  const conf = chaosArenaConf();
  const start = conf.indexOf("[AllShips]");
  const allShips = conf.slice(start, conf.indexOf("[", start + 1));
  const gravity = /^Gravity\s*=\s*(-?\d+)/m.exec(allShips);
  assert.ok(gravity, "assets/zones/chaos-zone/arena.conf [AllShips] must declare Gravity");
  assert.equal(Number(gravity[1]), CHAOS_WORMHOLE_GRAVITY,
    "the planner's gravity pin drifted from the shipped arena settings");
  // The reference game derives the reach: `sqrt(|gravity| * 1000)` px, the window `player_movement.rs` scans.
  assert.ok(Math.abs(wormholeInfluenceRadiusPx(1500) - 1224.744) < 0.01,
    `expected 1224.7 px for gravity 1500, got ${wormholeInfluenceRadiusPx(1500)}`);
  assert.equal(wormholeInfluenceRadiusPx(0), 0, "a ship with no wormhole gravity has no field");
  assert.equal(wormholeInfluenceRadiusPx(-1500), wormholeInfluenceRadiusPx(1500),
    "the field magnitude ignores the sign of the setting");
  assert.equal(CHAOS_WORMHOLE_AVOID_RADIUS_PX, wormholeInfluenceRadiusPx(CHAOS_WORMHOLE_GRAVITY));
  assert.equal(CHAOS_WORMHOLE_FIELD_AVOID_PX, CHAOS_WORMHOLE_AVOID_RADIUS_PX + CHAOS_WORMHOLE_CLEARANCE_PX);
});

test("the wormhole field blocks only the callers that ask for it", () => {
  const plain = authoredMapGeometry(chaosBytes());
  const avoiding = authoredMapGeometry(chaosBytes(), { wormholeAvoidRadiusPx: CHAOS_WORMHOLE_FIELD_AVOID_PX });
  assert.deepEqual(plain.wormholeAnchors, [{ tileX: 661, tileY: 256 }],
    "Chaos authors exactly one wormhole; a second one changes the geometry assumptions below");
  const anchorCentre = { x: (661 + 2.5) * 16, y: (256 + 2.5) * 16 };
  // The default stays compatible on purpose: the wormhole-authority lanes fly into a wormhole to test it.
  assert.equal(plain.inWormholeField(anchorCentre.x, anchorCentre.y), false,
    "avoidance must stay opt-in, or the wormhole-authority lanes can no longer reach one");
  assert.equal(avoiding.inWormholeField(anchorCentre.x, anchorCentre.y), true,
    "an avoiding geometry must recognise its own field");
  // A field, not a blanket block: beyond radius plus clearance the space is usable again.
  assert.equal(avoiding.inWormholeField(anchorCentre.x + CHAOS_WORMHOLE_FIELD_AVOID_PX + 96, anchorCentre.y), false);
  // The block is a route predicate, not terrain: the authored wormhole tile is still a wormhole tile.
  assert.equal(avoiding.tileAt(anchorCentre.x, anchorCentre.y), 220);
});

test("Chaos route legs stay out of the wormhole field from every origin that used to cross it", () => {
  const bytes = chaosBytes();
  const anchors = authoredMapGeometry(bytes).wormholeAnchors;
  const radius = CHAOS_WORMHOLE_AVOID_RADIUS_PX;
  // Origins north of the wormhole. Before the fix each planned a drive point 459 px from the anchor, deep
  // inside the field, which is the failure named at the top of this file.
  const origins = [{ x: 11250, y: 2500 }, { x: 10125, y: 2500 }, { x: 9500, y: 5000 }];
  let reproducedTheOldDefect = false;
  for (const from of origins) {
    const unfixed = chaosSafeRouteToAuthoredSafety(bytes, { from, wormholeAvoidRadiusPx: 0 });
    const unfixedInside = unfixed.drivePoints
      .filter((point) => distanceToWormholeField(point.x, point.y, anchors) <= radius);
    if (unfixedInside.length > 0) reproducedTheOldDefect = true;
    const fixed = chaosSafeRouteToAuthoredSafety(bytes, { from });
    const fixedInside = fixed.drivePoints
      .filter((point) => distanceToWormholeField(point.x, point.y, anchors) <= radius);
    assert.deepEqual(fixedInside, [],
      `route from (${from.x},${from.y}) still plans ${fixedInside.length} drive point(s) inside the field`);
    // Avoidance must not cost the destination: the route still stages on authored safety.
    assert.equal(fixed.stageTileId, 171,
      `route from (${from.x},${from.y}) no longer stages on authored safety`);
  }
  assert.ok(reproducedTheOldDefect,
    "the zero-radius planner must still reproduce the crossing, or this test proves nothing");
});

test("the field is a routing constraint, not terrain", () => {
  const geometry = authoredMapGeometry(chaosBytes(), { wormholeAvoidRadiusPx: CHAOS_WORMHOLE_FIELD_AVOID_PX });
  // Inside the field but outside the wormhole's 5x5 authored footprint, which the conservative harness rule
  // already treats as solid terrain.
  const inside = { x: 10098.76, y: 3598.35 };
  assert.ok(geometry.inWormholeField(inside.x, inside.y), "this point must be in the field");
  assert.equal(geometry.authoredHullIsTraversable(inside.x, inside.y), false);
  assert.equal(geometry.authoredTerrainHullIsTraversable(inside.x, inside.y), true,
    "a field must not masquerade as terrain: a ship the gravity pulled in is on legal ground");
  assert.equal(geometry.authoredTerrainLineIsClear(inside.x, inside.y, inside.x + 256, inside.y), true);
  assert.equal(geometry.authoredLineIsClear(inside.x, inside.y, inside.x + 256, inside.y), false,
    "the field-aware line test must still refuse a leg that crosses the field");
});

test("a pilot the field already contains is routed out of it, and cannot re-enter", () => {
  const bytes = chaosBytes();
  const anchors = authoredMapGeometry(bytes).wormholeAnchors;
  // The measured regression: lane 12 of a frozen matrix run began 690 px inside the field, and the
  // first version of the avoidance refused to route at all ("no hull-traversable tile exists within three
  // tiles"). A field is somewhere a ship really can be, because the field's own gravity puts it there.
  const from = { x: 10098.76, y: 3598.35 };
  assert.ok(distanceToWormholeField(from.x, from.y, anchors) <= CHAOS_WORMHOLE_AVOID_RADIUS_PX,
    "this origin must be inside the field, or the test does not pin the regression");
  const route = chaosSafeRouteToAuthoredSafety(bytes, { from });
  assert.equal(route.stageTileId, 171, "an in-field pilot must still be routable to authored safety");
  const distances = route.drivePoints.map((point) => distanceToWormholeField(point.x, point.y, anchors));
  // Leaving is necessary, so legs inside the field on the way out are expected. Coming back is not: the route
  // must be monotone about the field, and it must finish outside it.
  const lastInside = distances.reduce((acc, distance, index) =>
    (distance <= CHAOS_WORMHOLE_AVOID_RADIUS_PX ? index : acc), -1);
  assert.ok(lastInside >= 0, "the route must actually start inside the field");
  assert.deepEqual(distances.slice(lastInside + 1).filter((d) => d <= CHAOS_WORMHOLE_AVOID_RADIUS_PX), [],
    "the route re-entered the field after leaving it");
  assert.ok(distances.at(-1) > CHAOS_WORMHOLE_AVOID_RADIUS_PX, "the route must finish outside the field");
});

test("the canonical-spawn Chaos route is unchanged by the wormhole avoidance", () => {
  const bytes = chaosBytes();
  const unfixed = chaosSafeRouteToAuthoredSafety(bytes, { from: { x: 8_200, y: 8_200 }, wormholeAvoidRadiusPx: 0 });
  const avoiding = chaosSafeRouteToAuthoredSafety(bytes, { from: { x: 8_200, y: 8_200 } });
  // That route never nears the wormhole, so the avoidance must be free: identical hops and identical legs.
  // This is what keeps the fix honest - it changes the routes it must and nothing else.
  assert.equal(avoiding.hops, unfixed.hops);
  assert.deepEqual(avoiding.drivePoints, unfixed.drivePoints);
});
