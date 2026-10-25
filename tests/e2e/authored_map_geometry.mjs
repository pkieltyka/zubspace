// Tile id 171 is the authored safe surface: traversable, thrust-damped, and
// excluded as a firing or motion-measurement position.
export function authoredSafeTiles(bytes) {
  const offset = bytes.subarray(0, 2).toString() === "BM" ? bytes.readUInt32LE(2) : 0;
  const safeTiles = [];
  for (let cursor = offset; cursor + 4 <= bytes.length; cursor += 4) {
    const packed = bytes.readUInt32LE(cursor);
    if ((packed >>> 24) !== 171) continue;
    const tileX = packed & 0x0fff;
    const tileY = (packed >>> 12) & 0x0fff;
    if (tileX < 1_024 && tileY < 1_024) safeTiles.push({ tileX, tileY });
  }
  return safeTiles;
}

export function authoredSafetyDistance(safeTiles) {
  return (x, y) => {
    let nearest = Number.POSITIVE_INFINITY;
    for (const tile of safeTiles) {
      const minX = tile.tileX * 16;
      const minY = tile.tileY * 16;
      const dx = Math.max(minX - x, 0, x - (minX + 16));
      const dy = Math.max(minY - y, 0, y - (minY + 16));
      nearest = Math.min(nearest, Math.hypot(dx, dy));
    }
    return nearest;
  };
}
// The server's runtime map is not the record list: `animated_tile_size` in
// `src/bin/zubspace_server/zone.rs` expands authored animated tiles into their whole
// footprint - id 217 covers 2x2, id 219 covers 6x6 and the wormhole covers 5x5 - and
// stamps the id into every covered cell. A model that paints only the anchor tile
// therefore believes the trailing columns of a blob are open space. That is not
// cosmetic: a boost-staging runway was declared clear through a wall the server really
// blocked (Chrome/Extreme `test-e2e-immediate-keyboard-chords` - the pilot
// bounced at x=10431 off a 219 blob whose face is x=10416, and the lane then measured a
// wall bounce and failed its own motion floor). Keep this table equal to the server's.
const ANIMATED_TILE_FOOTPRINT = new Map([[217, 2], [219, 6], [220, 5]]);

// The wormhole gravity field reaches `sqrt(|gravity| * 1000)` px (`src/gameplay/player_movement.rs:519-528`,
// `wormhole_tile_window`, matching the reference game's `ShipController`). Callers that must keep a route *out* of
// that field pass the ship's derived radius to `authoredMapGeometry`; 0 means "no field", which is what every
// caller that does not ask for avoidance keeps - including the wormhole-authority lanes, which fly into one
// on purpose.
export function wormholeInfluenceRadiusPx(gravity) {
  const magnitude = Math.abs(Number(gravity) || 0);
  return magnitude === 0 ? 0 : Math.sqrt(magnitude * 1000);
}

// Read-only E2E geometry, never gameplay authority. Doors are conservatively
// blocked; safety is traversable but cannot be selected as a firing position.
//
// `wormholeAvoidRadiusPx` adds the wormhole gravity field to that blocked set. Measured: a Chaos
// route from (11250,2500) put a drive point at (10120,4040), 459 px from the wormhole at tile (661,256) and
// well inside its 1225 px field; the field accelerated the pilot into the anchor, the wormhole relocated it
// 4.1 km with the velocity zeroed, and the leg then needed ~110 s for 4.4 km inside a 60 s budget. The hull
// predicate is the tile-based one the BFS uses, so the field is resolved at tile resolution.
export function authoredMapGeometry(bytes, { wormholeAvoidRadiusPx = 0 } = {}) {
  const tiles = new Uint8Array(1024 * 1024);
  const wormholeTiles = [];
  const wormholeAnchors = [];
  const offset = bytes.subarray(0, 2).toString() === "BM" ? bytes.readUInt32LE(2) : 0;
  for (let cursor = offset; cursor + 4 <= bytes.length; cursor += 4) {
    const packed = bytes.readUInt32LE(cursor);
    const x = packed & 4095;
    const y = (packed >>> 12) & 4095;
    const id = packed >>> 24;
    if (x >= 1024 || y >= 1024) continue;
    const size = ANIMATED_TILE_FOOTPRINT.get(id) ?? 1;
    for (let dy = 0; dy < size; dy += 1) {
      for (let dx = 0; dx < size; dx += 1) {
        const stampedX = x + dx;
        const stampedY = y + dy;
        if (stampedX >= 1024 || stampedY >= 1024) continue;
        tiles[stampedY * 1024 + stampedX] = id;
        if (id !== 220) continue;
        wormholeTiles.push({ tileX: stampedX, tileY: stampedY });
        if (dx === 0 && dy === 0) wormholeAnchors.push({ tileX: stampedX, tileY: stampedY });
      }
    }
  }
  // Precomputed mask, so the tile-based BFS (which makes a hull query per map tile) stays O(1) per query.
  // A tile is in-field when its centre sits within the radius of any stamped wormhole cell - the same cell
  // grid the server samples, so tile resolution is the resolution the physics actually has.
  const wormholeFieldTiles = new Uint8Array(1024 * 1024);
  if (wormholeAvoidRadiusPx > 0) {
    const tileRadius = Math.ceil(wormholeAvoidRadiusPx / 16);
    for (const cell of wormholeTiles) {
      for (let ty = Math.max(0, cell.tileY - tileRadius); ty <= Math.min(1023, cell.tileY + tileRadius); ty += 1) {
        for (let tx = Math.max(0, cell.tileX - tileRadius); tx <= Math.min(1023, cell.tileX + tileRadius); tx += 1) {
          if (Math.hypot((tx - cell.tileX) * 16, (ty - cell.tileY) * 16) <= wormholeAvoidRadiusPx) {
            wormholeFieldTiles[ty * 1024 + tx] = 1;
          }
        }
      }
    }
  }
  const inWormholeField = (x, y) => {
    if (wormholeAvoidRadiusPx <= 0) return false;
    if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0 || x >= 16384 || y >= 16384) return false;
    return wormholeFieldTiles[Math.floor(y / 16) * 1024 + Math.floor(x / 16)] === 1;
  };
  const solid = id => id !== 0 && (id < 170 || (id >= 192 && id <= 240) || (id >= 242 && id <= 252));
  const tileAt = (x, y) => {
    if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0 || x >= 16384 || y >= 16384) return 1;
    return tiles[Math.floor(y / 16) * 1024 + Math.floor(x / 16)];
  };
  // Terrain only. The field is a routing constraint, not terrain: a ship the field's own gravity pulled into a
  // wormhole is still standing on legal ground. The two predicates are therefore separate, and the callers that
  // ask "can the hull be here" or "does this leg have a corridor ahead of it" use the terrain one - in one
  // measured run, lane 12 began at (10098.76,3598.35), 689 px inside the Chaos field, and a field-aware start
  // search refused to route at all instead of flying out.
  const authoredTerrainHullIsTraversable = (x, y, radius = 14) => {
    if (!Number.isFinite(x) || !Number.isFinite(y) || !Number.isFinite(radius) || radius < 0) return false;
    if (x - radius < 0 || y - radius < 0 || x + radius >= 16384 || y + radius >= 16384) return false;
    for (let ty = Math.floor((y - radius) / 16); ty <= Math.floor((y + radius) / 16); ty += 1) {
      for (let tx = Math.floor((x - radius) / 16); tx <= Math.floor((x + radius) / 16); tx += 1) {
        if (solid(tiles[ty * 1024 + tx])) return false;
      }
    }
    return true;
  };
  const authoredHullIsTraversable = (x, y, radius = 14) =>
    authoredTerrainHullIsTraversable(x, y, radius) && !inWormholeField(x, y);
  const authoredTerrainLineIsClear = (x, y, toX, toY) => {
    const distance = Math.hypot(toX - x, toY - y);
    if (!Number.isFinite(distance) || distance <= 0.01) return false;
    for (let traveled = 0; traveled <= distance; traveled += 4) {
      const phase = traveled / distance;
      if (solid(tileAt(x + (toX - x) * phase, y + (toY - y) * phase))) return false;
    }
    return !solid(tileAt(toX, toY));
  };
  const authoredLineIsClear = (x, y, toX, toY) => {
    if (!authoredTerrainLineIsClear(x, y, toX, toY)) return false;
    if (inWormholeField(x, y) || inWormholeField(toX, toY)) return false;
    const distance = Math.hypot(toX - x, toY - y);
    for (let traveled = 4; traveled < distance; traveled += 4) {
      const phase = traveled / distance;
      if (inWormholeField(x + (toX - x) * phase, y + (toY - y) * phase)) return false;
    }
    return true;
  };
  return { tiles, tileAt, wormholeAnchors, wormholeTiles, wormholeAvoidRadiusPx, inWormholeField,
    authoredHullIsTraversable, authoredLineIsClear,
    authoredTerrainHullIsTraversable, authoredTerrainLineIsClear,
    // Terrain only, deliberately: this answers "is this a valid measurement position" and answered the same
    // before the field existed, so adding the field here would change callers the defect never touched.
    authoredHullIsOpen: (x, y) => authoredTerrainHullIsTraversable(x, y) && tileAt(x, y) !== 171 };
}

// A harness that measures thrust needs a heading whose authored corridor is
// long enough for the whole measurement window and does not cross into safety,
// where the server damps thrust. Pure geometry: the caller supplies the hull
// predicate and its own safety-distance model.
// One heading's runway: how far the authored hull can travel along `(dx, dy)` before terrain, and the smallest
// distance from authored safety held anywhere on that path. `dx`/`dy` are world components (y down), the same
// space `orientClientForProtocolDirection` expects, so a caller can hand the result straight to it.
//
// Extracted from `bestBoostHeading` so a caller can ask about the heading the pilot *already holds*.
// `bestBoostHeading` only answers "which heading is best", which is the wrong question when the current one is
// already good and the caller wants to avoid touching the keyboard at all.
export function runwayAlongHeading({
  hullIsTraversable,
  safetyDistance,
  x,
  y,
  dx,
  dy,
  travelPx = 512,
  stepPx = 16,
} = {}) {
  if (typeof hullIsTraversable !== "function" || typeof safetyDistance !== "function") return null;
  if (![x, y, dx, dy].every(Number.isFinite)) return null;
  let runwayPx = 0;
  let heldClearancePx = Number.POSITIVE_INFINITY;
  for (let traveled = 0; traveled <= travelPx; traveled += stepPx) {
    const probeX = x + dx * traveled;
    const probeY = y + dy * traveled;
    if (!hullIsTraversable(probeX, probeY)) break;
    runwayPx = traveled;
    heldClearancePx = Math.min(heldClearancePx, safetyDistance(probeX, probeY));
  }
  return {
    dx,
    dy,
    runwayPx,
    clearancePx: heldClearancePx,
    endClearancePx: safetyDistance(x + dx * runwayPx, y + dy * runwayPx),
  };
}

export function bestBoostHeading({
  hullIsTraversable,
  safetyDistance,
  x,
  y,
  travelPx = 512,
  clearancePx = 128,
  bins = 40,
  stepPx = 16,
} = {}) {
  if (typeof hullIsTraversable !== "function" || typeof safetyDistance !== "function") return null;
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
  let best = null;
  for (let index = 0; index < bins; index += 1) {
    const angle = (index / bins) * Math.PI * 2;
    const dx = Math.cos(angle);
    const dy = Math.sin(angle);
    const probe = runwayAlongHeading({
      hullIsTraversable, safetyDistance, x, y, dx, dy, travelPx, stepPx,
    });
    if (!probe || probe.runwayPx < travelPx || probe.clearancePx < clearancePx) continue;
    if (!best || probe.clearancePx > best.clearancePx ||
      (probe.clearancePx === best.clearancePx && probe.endClearancePx > best.endClearancePx)) {
      best = {
        dx,
        dy,
        direction: index,
        runwayPx: probe.runwayPx,
        clearancePx: probe.clearancePx,
        endClearancePx: probe.endClearancePx,
      };
    }
  }
  return best;
}

// Distance from a point still inside authored safety to the nearest point that is not, measured as the shortest
// of a fixed set of straight walks. authoredSafetyDistance answers "how far to reach safety", which is zero
// everywhere inside it, so it cannot express the quantity these lanes actually depend on: how much room a pilot
// has before an assertion about being inside safety becomes a measurement of boundary jitter. Returns null when
// the point is not safe, and the cap when every walk stayed inside, which is reported as a cap rather than a
// distance because "at least 480" and "exactly 480" are different claims.
export function authoredSafetyMargin(isSafe, { directions = 32, stepPx = 1, capPx = 480 } = {}) {
  const unit = [];
  for (let index = 0; index < directions; index += 1) {
    const angle = (index / directions) * Math.PI * 2;
    unit.push([Math.cos(angle), Math.sin(angle)]);
  }
  return (x, y) => {
    if (!isSafe(x, y)) return null;
    let least = Number.POSITIVE_INFINITY;
    for (const [ux, uy] of unit) {
      let travelled = 0;
      while (travelled + stepPx <= capPx && isSafe(x + ux * (travelled + stepPx), y + uy * (travelled + stepPx))) {
        travelled += stepPx;
      }
      least = Math.min(least, travelled + stepPx);
      if (least <= stepPx) break;
    }
    return least >= capPx ? capPx : Number(least.toFixed(2));
  };
}

// Where inside authored safety a point has the most room. A lane that parks wherever a route first touches the
// safe region inherits whatever thickness that entry edge happens to have, and the Chaos entry edge is a few px
// - which turns every later assertion about being inside safety into a measurement of boundary jitter. This
// searches a square around an entry point and returns the spot with the largest margin, so a lane can state a
// floor and be held to it. Returns marginCap: true when the best spot's margin reached the search cap, because
// that is a lower bound, not a measurement.
export function deepestAuthoredPoint(isSafe, { from, radiusPx = 256, stepPx = 8, capPx = 480 } = {}) {
  const margin = authoredSafetyMargin(isSafe, { capPx });
  let best = null;
  for (let x = from.x - radiusPx; x <= from.x + radiusPx; x += stepPx) {
    for (let y = from.y - radiusPx; y <= from.y + radiusPx; y += stepPx) {
      const value = margin(x, y);
      if (value === null) continue;
      if (best === null || value > best.marginPx) best = { x, y, marginPx: value, marginCap: value >= capPx };
      if (best.marginPx >= capPx) break;
    }
  }
  return best;
}

// Where to park a pilot that has to assert something about being inside authored safety AND then move.
// deepestAuthoredPoint alone is not enough: authored safety and hull-traversable space are different surfaces,
// and a point can sit deep inside the first while its hull overlaps solid tiles of the second. The distinction is
// not hypothetical - a Chaos chord release staged 23 px inside authored safety travelled 2.9 px in ten seconds
// with thrust held, reversing every tenth of a second, which is a hull that fits a pocket but cannot leave it.
// So the search maximises the BINDING clearance, min(safety, hull), and reports both numbers so a caller can
// state a floor against each and a failure can name whichever one the region cannot supply.
export function deepestStagingPoint({ isSafe, isHullClear, from, radiusPx = 256, stepPx = 8, capPx = 96 } = {}) {
  if (typeof isSafe !== "function" || typeof isHullClear !== "function") {
    throw new TypeError("deepestStagingPoint needs both isSafe and isHullClear; a staging spot chosen against "
      + "one surface is how a pilot ends up parked in a pocket");
  }
  const safetyMargin = authoredSafetyMargin(isSafe, { capPx });
  const hullMargin = authoredSafetyMargin(isHullClear, { capPx });
  let best = null;
  // Anchor candidates to world coordinates. An8px grid anchored to a fractional
  // entry phase can miss the centre of a32px strip and falsely report14px as
  // its maximum clearance even though the authored centre supplies17px.
  const firstX = Math.ceil((from.x - radiusPx) / stepPx) * stepPx;
  const firstY = Math.ceil((from.y - radiusPx) / stepPx) * stepPx;
  for (let x = firstX; x <= from.x + radiusPx; x += stepPx) {
    for (let y = firstY; y <= from.y + radiusPx; y += stepPx) {
      const safety = safetyMargin(x, y);
      if (safety === null) continue;
      const hull = hullMargin(x, y);
      if (hull === null) continue;
      const binding = Math.min(safety, hull);
      if (best === null || binding > best.bindingMarginPx) {
        best = { x, y, safetyMarginPx: safety, hullMarginPx: hull, bindingMarginPx: binding };
      }
    }
  }
  return best;
}

// A heading the pilot can actually leave authored safety along, judged at the point it is standing. Two separate
// questions get conflated without this: "how far is the corridor clear" and "does thrusting that way take me out
// of the green". A long hull-clear corridor can run entirely inside safety, and a heading that exits safety in
// 40 px can be blocked by terrain at 20 px - which is the second failure, the one that kept a Chaos release
// bouncing with x fixed for ten seconds while boundaryPx flipped instead of shrinking.
export function authoredExitHeading({ isSafe, isHullClear, from, headings = [[0, -1], [0, 1], [-1, 0], [1, 0]],
  maxPx = 1024, stepPx = 8 } = {}) {
  if (typeof isSafe !== "function" || typeof isHullClear !== "function") {
    throw new TypeError("authoredExitHeading needs both isSafe and isHullClear; a corridor measured on one "
      + "surface is how a heading gets chosen that exits the green behind a wall");
  }
  const candidates = [];
  for (const [x, y] of headings) {
    let exitDistancePx = null;
    let hullClearPx = 0;
    for (let travelled = stepPx; travelled <= maxPx; travelled += stepPx) {
      const px = from.x + x * travelled;
      const py = from.y + y * travelled;
      if (exitDistancePx === null && !isSafe(px, py)) exitDistancePx = travelled;
      if (isHullClear(px, py)) hullClearPx = travelled;
      else if (exitDistancePx === null) break;
      else break;
    }
    candidates.push({
      heading: { x, y }, exitDistancePx, hullClearPx,
      usable: exitDistancePx !== null && hullClearPx + stepPx >= exitDistancePx,
    });
  }
  const usable = candidates.filter((candidate) => candidate.usable);
  // Soonest exit wins: the release case has to cross, and a short proven corridor is a stronger guarantee than a
  // long one whose far half the pilot never reaches.
  usable.sort((a, b) => a.exitDistancePx - b.exitDistancePx);
  return { chosen: usable[0] ?? null, candidates };
}

// Classify only the narrow static-route mismatch: a hull overlapping doors
// which the fresh authoritative door clock currently reports open. Real walls,
// closed doors and missing/stale clock evidence remain blocked.
export function hullOverlapsOnlyOpenDoors(geometry, metrics, radius = 14) {
  const { playerX: x, playerY: y, doorSeed: seed, doorClockFresh } = metrics;
  if (!Number.isFinite(x) || !Number.isFinite(y) || doorClockFresh !== true ||
      !Number.isInteger(seed) || seed < 0 || seed > 255 ||
      x - radius < 0 || y - radius < 0 || x + radius >= 16384 || y + radius >= 16384) return false;
  let openDoor = false;
  for (let ty = Math.floor((y - radius) / 16); ty <= Math.floor((y + radius) / 16); ty++) {
    for (let tx = Math.floor((x - radius) / 16); tx <= Math.floor((x + radius) / 16); tx++) {
      const id = geometry.tileAt(tx * 16 + 8, ty * 16 + 8);
      if (id >= 162 && id <= 169) {
        if ((seed & (1 << (id - 162))) !== 0) return false;
        openDoor = true;
      } else if (id !== 0 && (id < 170 || (id >= 192 && id <= 240) || (id >= 242 && id <= 252))) {
        return false;
      }
    }
  }
  return openDoor;
}
