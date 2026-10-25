import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { test } from "node:test";

import {
  authoredMapGeometry,
  authoredSafetyMargin,
  deepestStagingPoint,
} from "./authored_map_geometry.mjs";
import {
  CHAOS_WORMHOLE_AVOID_RADIUS_PX,
  CHAOS_WORMHOLE_FIELD_AVOID_PX,
  chaosSafeRouteToAuthoredSafety,
} from "./chaos_safe_route.mjs";
import { createChaosRouteReplanZone } from "./wormhole_fixture.mjs";

test("Chaos route fixture forces a portal crossing and leaves an authored-safe replacement route", () => {
  const fixture = createChaosRouteReplanZone();
  try {
    const initial = chaosSafeRouteToAuthoredSafety(fixture.originalMap, { from: fixture.spawn });
    const first = fixture.spawn;
    const second = initial.drivePoints[0];
    const portal = fixture.wormholeTarget;
    const dx = second.x - first.x;
    const dy = second.y - first.y;
    const length = Math.hypot(dx, dy);
    const projection = Math.max(0, Math.min(1,
      ((portal.x - first.x) * dx + (portal.y - first.y) * dy) / (length * length)));
    const closestX = first.x + projection * dx;
    const closestY = first.y + projection * dy;
    const segmentDistance = Math.hypot(portal.x - closestX, portal.y - closestY);

    assert.equal(fixture.originalMap.length + 4, fixture.map.length);
    assert.equal(initial.drivePoints.length >= 2, true);
    assert.ok(Math.hypot(first.x - portal.x, first.y - portal.y) > fixture.wormholeGravityRadiusPx,
      "the pilot begins outside the Chaos wormhole gravity field");
    assert.ok(segmentDistance < fixture.wormholeGravityRadiusPx,
      "the first genuine-key route leg enters the Chaos wormhole gravity field");
    assert.equal(fixture.wormholeGravityRadiusPx, CHAOS_WORMHOLE_AVOID_RADIUS_PX);
    assert.equal(fixture.wormholeAvoidRadiusPx, CHAOS_WORMHOLE_FIELD_AVOID_PX);

    const settings = fixture.settings.toString("utf8");
    for (let team = 0; team < 4; team++) {
      assert.equal(settings.match(new RegExp(`^Team${team}-Radius=(.+)$`, "m"))?.[1], "0",
        "the declared canonical-origin fixture must not inherit the shipping random region");
      assert.match(settings, new RegExp(`^Team${team}-X=-512$`, "m"));
      assert.match(settings, new RegExp(`^Team${team}-Y=-512$`, "m"));
    }
    assert.match(settings, /^Gravity=1500$/m);
    assert.match(settings, /^GravityTopSpeed=100$/m);
    assert.equal(fixture.evidence.wormholeGravity, 1500);

    const geometry = authoredMapGeometry(fixture.map, {
      wormholeAvoidRadiusPx: fixture.wormholeAvoidRadiusPx,
    });
    assert.deepEqual(geometry.wormholeAnchors, [
      { tileX: 661, tileY: 256 },
      { tileX: 596, tileY: 506 },
    ]);
    assert.equal(geometry.inWormholeField(fixture.spawn.x, fixture.spawn.y), false,
      "the authoritative landing starts outside the fixture portal's live gravity field");

    const replanned = chaosSafeRouteToAuthoredSafety(fixture.map, {
      from: fixture.spawn,
      wormholeAvoidRadiusPx: fixture.wormholeAvoidRadiusPx,
    });
    let from = replanned.start;
    for (const waypoint of replanned.drivePoints) {
      assert.equal(geometry.authoredLineIsClear(from.x, from.y, waypoint.x, waypoint.y), true,
        `replacement route segment must stay out of the portal field: ${JSON.stringify({ from, waypoint })}`);
      from = waypoint;
    }
    assert.equal(geometry.tileAt(replanned.stage.x, replanned.stage.y), 171);

    const deepest = deepestStagingPoint({
      isSafe: (x, y) => geometry.tileAt(x, y) === 171,
      isHullClear: geometry.authoredTerrainHullIsTraversable,
      from: replanned.stage,
    });
    assert.ok(deepest);
    assert.ok(deepest.safetyMarginPx >= 16);
    assert.ok(deepest.hullMarginPx >= 16);
    const safetyMargin = authoredSafetyMargin((x, y) => geometry.tileAt(x, y) === 171);
    const hullMargin = authoredSafetyMargin(geometry.authoredTerrainHullIsTraversable);
    assert.ok(safetyMargin(deepest.x, deepest.y) >= 16);
    assert.ok(hullMargin(deepest.x, deepest.y) >= 16);
  } finally {
    fixture.cleanup();
  }
});


test("the real Chrome coordinator preserves the declared Chaos route origin before its gate", async () => {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("    if (movementCorrectionGate || warpRelocationGate", source.indexOf("const portalPrizeStreamLogOffset"));
  const end = source.indexOf("\n    const shipChangeEnergy", start);
  assert(start > 0 && end > start);
  let live = 0, travelled = 0;
  const ctx = vm.createContext({ movementCorrectionGate: false, warpRelocationGate: false,
    portalPhaseTwoGate: false, chaosRouteReplanGate: true, alpha: {}, beta: {}, server: {},
    waitForLivePlayerScopes: async () => { live++; },
    rendezvousPlayers: async () => { travelled++; }, waitForPlayersReady: async () => {},
  });
  await vm.runInContext(`(async () => {${source.slice(start, end)}})()`, ctx);
  assert.equal(live, 1); assert.equal(travelled, 0);
});
