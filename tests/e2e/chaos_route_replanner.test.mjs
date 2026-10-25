import assert from "node:assert/strict";
import test from "node:test";

import { driveRouteWithReplans } from "./chaos_route_replanner.mjs";

test("a mid-leg relocation replans from the live position and drives the replacement route", async () => {
  let position = { x: 100, y: 100 };
  const attempted = [];
  const plannerOrigins = [];
  const result = await driveRouteWithReplans({
    initialDrivePoints: [{ x: 300, y: 100 }, { x: 500, y: 100 }],
    maxReplans: 2,
    currentPosition: async () => ({ ...position }),
    planFrom: async (from) => {
      plannerOrigins.push({ ...from });
      return { drivePoints: [{ x: 900, y: 850 }, { x: 1_000, y: 850 }] };
    },
    driveLeg: async (waypoint) => {
      attempted.push({ ...waypoint });
      if (attempted.length === 1) {
        // Model a server-observed relocation that invalidates the in-flight route leg.
        position = { x: 850, y: 900 };
        throw new Error("wormhole relocation invalidated the current leg");
      }
      position = { ...waypoint };
      return { waypoint, reached: true };
    },
  });

  assert.equal(result.replans, 1);
  assert.deepEqual(plannerOrigins, [{ x: 850, y: 900 }]);
  assert.deepEqual(attempted, [
    { x: 300, y: 100 },
    { x: 900, y: 850 },
    { x: 1_000, y: 850 },
  ]);
  assert.deepEqual(result.legs[0], {
    waypoint: { x: 300, y: 100 },
    reached: false,
    replanned: true,
    replan: 1,
    from: { x: 850, y: 900 },
    reason: "wormhole relocation invalidated the current leg",
  });
  assert.equal(result.legs.slice(1).every((leg) => leg.reached), true);
});

test("route replanning is bounded and preserves the last leg failure", async () => {
  let position = { x: 0, y: 0 };
  let plans = 0;
  const terminalFailure = new Error("third relocated leg failed");

  await assert.rejects(driveRouteWithReplans({
    initialDrivePoints: [{ x: 10, y: 0 }],
    maxReplans: 2,
    currentPosition: async () => ({ ...position }),
    planFrom: async () => {
      plans += 1;
      return { drivePoints: [{ x: plans * 100, y: 0 }] };
    },
    driveLeg: async (waypoint) => {
      position = { x: waypoint.x + 1, y: waypoint.y + 1 };
      throw plans === 2 ? terminalFailure : new Error("leg invalidated by relocation");
    },
  }), (error) => error === terminalFailure);

  assert.equal(plans, 2);
});
