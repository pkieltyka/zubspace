// The runway probe the Chaos and Extreme chord staging both select a heading with. Extracted from
// `bestBoostHeading` so a caller can ask about the heading the pilot *already holds* - which is what lets the
// Chaos staging cost one sample instead of a brake/turn/re-brake cycle immediately before the lane's 90 Hz draw
// cadence probe. These cases pin the probe's own arithmetic, and that `bestBoostHeading` still agrees with it.
import assert from "node:assert/strict";
import test from "node:test";
import { bestBoostHeading, runwayAlongHeading } from "./authored_map_geometry.mjs";

// Terrain at x >= 300: a westward corridor of unbounded length, an eastward one that stops inside the window.
const wall = { hullIsTraversable: (x) => x < 300, safetyDistance: () => 0 };

test("runwayAlongHeading measures one heading and reports where terrain stops it", () => {
  const east = runwayAlongHeading({ ...wall, x: 0, y: 0, dx: 1, dy: 0, travelPx: 512 });
  // Probes at 0, 16, ... 288 are clear and 304 is not, so the measured runway is 288 rather than "about 300".
  assert.equal(east.runwayPx, 288, JSON.stringify(east));
  const west = runwayAlongHeading({ ...wall, x: 0, y: 0, dx: -1, dy: 0, travelPx: 512 });
  assert.equal(west.runwayPx, 512, JSON.stringify(west));
});

test("bestBoostHeading only returns headings that clear the whole travel, and agrees with the probe", () => {
  const best = bestBoostHeading({ ...wall, x: 0, y: 0, travelPx: 512, clearancePx: 0, bins: 4, stepPx: 16 });
  assert.ok(best, "a westward runway exists");
  const probe = runwayAlongHeading({
    ...wall, x: 0, y: 0, dx: best.dx, dy: best.dy, travelPx: 512,
  });
  assert.equal(probe.runwayPx, 512, JSON.stringify({ best, probe }));
});

test("runwayAlongHeading reports the clearance it held, and bestBoostHeading enforces it", () => {
  const open = { hullIsTraversable: () => true, safetyDistance: (x, y) => Math.hypot(x, y) };
  const out = runwayAlongHeading({ ...open, x: 0, y: 0, dx: 1, dy: 0, travelPx: 512 });
  assert.equal(out.runwayPx, 512);
  // The path starts where the pilot stands, so the held minimum is taken at x = 0.
  assert.equal(out.clearancePx, 0, JSON.stringify(out));
  // Requiring clearance therefore rejects every heading from a point that has none.
  assert.equal(bestBoostHeading({ ...open, x: 0, y: 0, travelPx: 512, clearancePx: 128 }), null);
  // ... and accepts them all from a point that does.
  assert.ok(bestBoostHeading({ ...open, x: 400, y: 400, travelPx: 512, clearancePx: 128, bins: 4 }));
});

test("runwayAlongHeading refuses non-finite geometry instead of inventing a runway", () => {
  assert.equal(runwayAlongHeading({ ...wall, x: Number.NaN, y: 0, dx: 1, dy: 0 }), null);
  assert.equal(runwayAlongHeading({ x: 0, y: 0, dx: 1, dy: 0 }), null);
});
