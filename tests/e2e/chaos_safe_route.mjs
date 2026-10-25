// Genuine-key navigation into authored Chaos safety.
//
// Chaos is not Extreme: its canonical spawn is not on the safe surface, and the authored
// safe tiles (id 171) sit in disconnected pockets - only two of the map's 353 are
// reachable by a ship hull from the spawn, 195 corridor hops away. A Chaos safe-zone
// proof therefore has to *fly* there, and it has to fly from wherever the pilot actually
// is: the phase that needs safety runs after a matrix that has already moved the pilot a
// couple of kilometres, and a spawn-relative route aimed it at nothing.
//
// This module computes the route from the shipped map bytes - the same tile grid the
// server admits against - and returns the waypoints a real keyboard driver must follow.
// Read-only E2E geometry, never gameplay authority: the case still asserts the client's
// own `inSafeZone` predicate before it keys anything.
import { authoredMapGeometry, authoredSafeTiles, wormholeInfluenceRadiusPx } from "./authored_map_geometry.mjs";

const TILE_PX = 16;
const GRID = 1024;

// Chaos' `[AllShips] Gravity` (`assets/zones/chaos-zone/arena.conf:4`), which puts its wormhole's gravity
// field at `sqrt(1500 * 1000)` = 1224.7 px. `chaos_wormhole_avoidance.test.mjs` reads the shipped
// `arena.conf` and asserts this constant still equals it: a silent drift here would put the planner back
// inside the field, which is exactly the defect this exists to prevent.
export const CHAOS_WORMHOLE_GRAVITY = 1500;
export const CHAOS_WORMHOLE_AVOID_RADIUS_PX = wormholeInfluenceRadiusPx(CHAOS_WORMHOLE_GRAVITY);

// The BFS is 4-connected and runs along tile centres, turning on tile boundaries, while simplification flies
// straight between those corners - so a leg can cut across the very detour the planner just built. Measured
// over 112 origins on the shipped Chaos map: a zero margin still leaves 25 origins with a leg inside the true
// field (deepest 11.0 px), 8 px leaves 24 (2.8 px), and 16 px leaves none. Two tiles ships as headroom beyond
// the one that measured clean, so every leg stays clear of the field instead of grazing its edge.
export const CHAOS_WORMHOLE_CLEARANCE_PX = 32;
export const CHAOS_WORMHOLE_FIELD_AVOID_PX = CHAOS_WORMHOLE_AVOID_RADIUS_PX + CHAOS_WORMHOLE_CLEARANCE_PX;

// Waypoint legs are subdivided so a single closed-loop drive never aims past a
// corridor bend: the driver steers by velocity error, and a target two corners away
// is not a target.
const MAX_LEG_PX = 256;

const NEIGHBOURS = [[1, 0], [-1, 0], [0, 1], [0, -1]];

function tileCenterWorld(tileX, tileY) {
  return { x: (tileX + 0.5) * TILE_PX, y: (tileY + 0.5) * TILE_PX };
}

// Which way a held thrust on the stage tile can travel without hitting authored terrain.
// The held sets hold thrust for seconds while the safe-zone stop stair-steps the ship, so
// a stage whose facing runs into a wall would measure a wall bounce. Probes are descending
// so the reported figure is a lower bound that is definitely clear.
// How deep the authored safe surface actually runs from the stage tile. Chaos' reachable
// green is four tiles deep, and the measured physics is what makes this the limiting number
// for a held thrust: the authored stop caps speed but not position, so a held thrust
// stair-steps about 21px per 0.4s cooldown cycle (attempt 5 drifted 315px in a 15s hold).
// The thrust samples below are therefore sized from this depth, not from a wish.
export function safeRunFromStage(geometry, stageWorld) {
  const tile = (x, y) => geometry.tileAt(x * TILE_PX + TILE_PX / 2, y * TILE_PX + TILE_PX / 2);
  const origin = { x: Math.floor(stageWorld.x / TILE_PX), y: Math.floor(stageWorld.y / TILE_PX) };
  let best = null;
  for (const [x, y] of [[0, -1], [0, 1], [-1, 0], [1, 0]]) {
    let tiles = 1;
    while (tile(origin.x + x * (tiles + 1), origin.y + y * (tiles + 1)) === 171) tiles += 1;
    if (best === null || tiles > best.tiles) best = { x, y, tiles };
  }
  return { x: best.x, y: best.y, runPx: best.tiles * TILE_PX };
}

export function escapeHeadingFromStage(geometry, stageWorld) {
  let best = null;
  for (const [x, y] of [[0, -1], [0, 1], [-1, 0], [1, 0]]) {
    let clearPx = 0;
    for (const probe of [1024, 512, 256, 128]) {
      if (geometry.authoredLineIsClear(stageWorld.x, stageWorld.y,
        stageWorld.x + x * probe, stageWorld.y + y * probe)) {
        clearPx = probe;
        break;
      }
    }
    if (best === null || clearPx > best.clearPx) best = { x, y, clearPx };
  }
  return best;
}

// Breadth-first over tile centres the authored hull can occupy, from the pilot's tile to the nearest
// authored safe tile. Returns `null` when Chaos grows a map where safety is unreachable, which must fail
// the case rather than skip it.
//
// The wormhole field is a *layer*, not a blocked tile, because the pilot can start inside one: the field's
// own gravity pulls a ship in and the matrix's earlier phases leave it there (in one measured run, lane 12
// began at (10098.76,3598.35), 689 px from the anchor). Layer 1 means "started inside the field and has not
// left it yet" and may move within the field or step out; layer 0 means "outside" and may never step back
// in, so outside is absorbing. Blocking the field outright could not route such a pilot at all - the escape
// that exits at the nearest field edge landed in a pocket the field walled off, and the route then had no
// green to reach - while ignoring it entirely is the measured defect where a route crossed the field and
// the wormhole moved the ship ~4.1 km with the velocity zeroed.
const BFS_LAYERS = 2;

function bfsToSafety(geometry, safeTiles, spawnTile) {
  const startCell = spawnTile.x + spawnTile.y * GRID;
  const startWorld = tileCenterWorld(spawnTile.x, spawnTile.y);
  const startState = startCell * BFS_LAYERS + (geometry.inWormholeField(startWorld.x, startWorld.y) ? 1 : 0);
  const distance = new Int32Array(GRID * GRID * BFS_LAYERS).fill(-1);
  const previous = new Int32Array(GRID * GRID * BFS_LAYERS).fill(-1);
  const safeTileKeys = new Set(safeTiles.map((tile) => tile.tileX + tile.tileY * GRID));
  const queue = [startState];
  distance[startState] = 0;
  let found = -1;
  for (let head = 0; head < queue.length && found < 0; head += 1) {
    const state = queue[head];
    const layer = state % BFS_LAYERS;
    const cell = (state / BFS_LAYERS) | 0;
    const cx = cell % GRID;
    const cy = (cell / GRID) | 0;
    if (state !== startState && safeTileKeys.has(cell)) {
      found = state;
      break;
    }
    for (const [dx, dy] of NEIGHBOURS) {
      const nx = cx + dx;
      const ny = cy + dy;
      if (nx < 0 || ny < 0 || nx >= GRID || ny >= GRID) continue;
      const nextWorld = tileCenterWorld(nx, ny);
      const nextLayer = geometry.inWormholeField(nextWorld.x, nextWorld.y) ? 1 : 0;
      // Leaving the field is one-way, so the route cannot cross it and come back.
      if (layer === 0 && nextLayer === 1) continue;
      const nextState = (nx + ny * GRID) * BFS_LAYERS + nextLayer;
      if (distance[nextState] !== -1) continue;
      // Terrain only: the field is the layer above, and the pilot may legitimately be standing in it.
      if (!geometry.authoredTerrainHullIsTraversable(nextWorld.x, nextWorld.y)) continue;
      distance[nextState] = distance[state] + 1;
      previous[nextState] = state;
      queue.push(nextState);
    }
  }
  if (found < 0) return null;
  const tiles = [];
  for (let state = found; state !== startState && state !== -1; state = previous[state]) {
    const cell = (state / BFS_LAYERS) | 0;
    tiles.push({ tileX: cell % GRID, tileY: (cell / GRID) | 0 });
  }
  tiles.reverse();
  return { tiles, hops: distance[found], stage: tiles.at(-1) };
}

// Two different point lists come out of one polyline. `waypoints` keeps the 256px leg cap
// the velocity-error steering driver needs; `drivePoints` is the same certified corner path
// with a 900px cap, because the Chaos approach brakes at every point it is given and a
// straight corridor of eight tiles should cost one brake, not four.
const MAX_DRIVE_POINT_PX = 900;

function simplifyToWaypoints(tiles, spawnWorld, maxLegPx = MAX_LEG_PX) {
  const corners = [];
  for (let i = 1; i < tiles.length - 1; i += 1) {
    const incomingDx = tiles[i].tileX - tiles[i - 1].tileX;
    const incomingDy = tiles[i].tileY - tiles[i - 1].tileY;
    const outgoingDx = tiles[i + 1].tileX - tiles[i].tileX;
    const outgoingDy = tiles[i + 1].tileY - tiles[i].tileY;
    if (incomingDx !== outgoingDx || incomingDy !== outgoingDy) {
      corners.push(tileCenterWorld(tiles[i].tileX, tiles[i].tileY));
    }
  }
  const ordered = [
    ...(corners.length > 0 ? [corners[0]] : []),
    ...corners.slice(1),
    tileCenterWorld(tiles.at(-1).tileX, tiles.at(-1).tileY),
  ];
  const waypoints = [];
  let from = spawnWorld;
  for (const target of ordered) {
    const dx = target.x - from.x;
    const dy = target.y - from.y;
    const steps = Math.max(1, Math.ceil(Math.hypot(dx, dy) / maxLegPx));
    for (let step = 1; step <= steps; step += 1) {
      waypoints.push({
        x: Math.round(from.x + (dx * step) / steps),
        y: Math.round(from.y + (dy * step) / steps),
      });
    }
    from = target;
  }
  return waypoints;
}

// The pilot is never on a tile centre, and the tile it overlaps may be one the hull
// cannot legally occupy - a ship is 28px across and a corridor can be one tile wide. BFS
// needs a start the hull can occupy, so take the nearest one within three tiles. Terrain
// only: the wormhole field is not a legal-occupancy question, and using the field-aware
// predicate here was the measured regression where a pilot inside a field could
// not be routed at all.
function nearestTraversableTile(geometry, from) {
  const centre = { x: Math.floor(from.x / TILE_PX), y: Math.floor(from.y / TILE_PX) };
  for (let radius = 0; radius <= 3; radius += 1) {
    let best = null;
    let bestDistance = Number.POSITIVE_INFINITY;
    for (let dy = -radius; dy <= radius; dy += 1) {
      for (let dx = -radius; dx <= radius; dx += 1) {
        const tile = { x: centre.x + dx, y: centre.y + dy };
        if (tile.x < 0 || tile.y < 0 || tile.x >= GRID || tile.y >= GRID) continue;
        const centreWorld = tileCenterWorld(tile.x, tile.y);
        if (!geometry.authoredTerrainHullIsTraversable(centreWorld.x, centreWorld.y)) continue;
        const distance = Math.hypot(dx, dy);
        if (distance < bestDistance) {
          best = tile;
          bestDistance = distance;
        }
      }
    }
    if (best !== null) return best;
  }
  return null;
}

export function chaosSafeRouteToAuthoredSafety(bytes, {
  from,
  // Avoid the wormhole field by default. A route that crosses it is not merely risky: in one measured run, a
  // leg from (11250,2500) aimed a drive point at (10120,4040), 459 px from the wormhole, the field
  // accelerated the pilot into the anchor, the relocation moved it 4.1 km with the velocity zeroed, and the
  // leg could not recover inside its budget. The zero radius reproduces the old planning for measurement.
  wormholeAvoidRadiusPx = CHAOS_WORMHOLE_FIELD_AVOID_PX,
} = {}) {
  const geometry = authoredMapGeometry(bytes, { wormholeAvoidRadiusPx });
  const safeTiles = authoredSafeTiles(bytes);
  const startTile = nearestTraversableTile(geometry, from);
  if (startTile === null) {
    throw new Error(
      `no hull-traversable tile exists within three tiles of ${JSON.stringify(from)}; ` +
        "the Chaos safe-zone case cannot route a pilot that is not in authored open space",
    );
  }
  const start = tileCenterWorld(startTile.x, startTile.y);
  const route = bfsToSafety(geometry, safeTiles, startTile);
  if (route === null) {
    throw new Error(
      `no authored Chaos safe tile is reachable by the ship hull from ` +
        `${JSON.stringify(from)} (tile ${JSON.stringify(startTile)}); a Chaos safe-zone ` +
        "gate needs a reachable green, not a skipped assertion",
    );
  }
  const stageWorld = tileCenterWorld(route.stage.tileX, route.stage.tileY);
  const waypoints = simplifyToWaypoints(route.tiles, start);
  const drivePoints = simplifyToWaypoints(route.tiles, start, MAX_DRIVE_POINT_PX);
  return {
    from,
    fromOnSafety: geometry.tileAt(from.x, from.y) === 171,
    startTile,
    start,
    safeTileCount: safeTiles.length,
    hops: route.hops,
    stageTile: route.stage,
    stage: stageWorld,
    stageTileId: geometry.tileAt(stageWorld.x, stageWorld.y),
    escapeHeading: escapeHeadingFromStage(geometry, stageWorld),
    safeRun: safeRunFromStage(geometry, stageWorld),
    waypoints,
    drivePoints,
    legLengths: waypoints.map((waypoint, index, all) => Math.round(Math.hypot(
      waypoint.x - (index === 0 ? start.x : all[index - 1].x),
      waypoint.y - (index === 0 ? start.y : all[index - 1].y),
    ))),
  };
}
