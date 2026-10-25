// Read-only encounter setup for real-key browser drivers. Random lifecycle
// spawns are not a promise of overlap or even mutual AOI visibility.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { authoredMapGeometry, wormholeInfluenceRadiusPx } from "./authored_map_geometry.mjs";

const SIZE = 1024;
const TILE = 16;
const center = cell => ({ x: (cell % SIZE + 0.5) * TILE, y: (Math.floor(cell / SIZE) + 0.5) * TILE });
const cellAt = p => Math.floor(p.x / TILE) + Math.floor(p.y / TILE) * SIZE;
const finitePoint = p => Number.isFinite(p?.x) && Number.isFinite(p?.y) &&
  p.x >= 0 && p.y >= 0 && p.x < SIZE * TILE && p.y < SIZE * TILE;

export function loadRendezvousMap(arena, roots) {
  if (!/^[a-z0-9-]{1,32}$/.test(arena)) throw new Error("invalid rendezvous arena identity");
  for (const root of roots) {
    if (!fs.existsSync(root)) continue;
    const matches = fs.readdirSync(root, { withFileTypes: true }).filter(entry => entry.isDirectory())
      .map(entry => path.join(root, entry.name)).filter(directory => {
        const manifest = path.join(directory, "zone.bundle");
        return fs.existsSync(manifest) && fs.readFileSync(manifest, "utf8").match(/^id=(.+)$/m)?.[1]?.trim() === arena;
      });
    if (matches.length > 1) throw new Error(`ambiguous rendezvous map for ${arena}`);
    if (!matches.length) continue;
    const directory = matches[0];
    const manifest = fs.readFileSync(path.join(directory, "zone.bundle"), "utf8");
    const read = key => {
      const name = manifest.match(new RegExp(`^${key}=(.+)$`, "m"))?.[1]?.trim();
      const wanted = manifest.match(new RegExp(`^${key}_sha256=(.+)$`, "m"))?.[1]?.trim();
      if (!name || path.basename(name) !== name || name.includes("\\") || !/^[a-f0-9]{64}$/.test(wanted || "")) {
        throw new Error(`invalid ${key} in rendezvous manifest`);
      }
      const bytes = fs.readFileSync(path.join(directory, name));
      const digest = crypto.createHash("sha256").update(bytes).digest("hex");
      if (digest !== wanted) throw new Error(`rendezvous ${key} checksum mismatch`);
      return { name, bytes, sha256: digest };
    };
    const map = read("map"), settings = read("settings");
    // Conservative across every ship's authored gravity. A route is setup,
    // never a measurement of entering the wormhole's acceleration field.
    const gravity = Math.max(0, ...[...settings.bytes.toString().matchAll(/^Gravity\s*=\s*(-?\d+)\s*$/gm)]
      .map(match => Math.abs(Number(match[1]))));
    const avoidRadius = gravity > 0 ? wormholeInfluenceRadiusPx(gravity) + 32 : 0;
    return { geometry: authoredMapGeometry(map.bytes, { wormholeAvoidRadiusPx: avoidRadius }),
      provenance: { arena, directory, map: map.name, mapSha256: map.sha256,
        settingsSha256: settings.sha256, wormholeAvoidRadiusPx: avoidRadius } };
  }
  throw new Error(`no authored rendezvous map for ${arena}`);
}

function routeSearch(from, accepts, traversable) {
  const start = cellAt(from), startWorld = center(start);
  if (!traversable.segment(from, startWorld)) return null;
  const previous = new Int32Array(SIZE * SIZE).fill(-2);
  const queue = new Int32Array(SIZE * SIZE);
  let tail = 1;
  queue[0] = start;
  previous[start] = -1;
  for (let head = 0; head < tail; head++) {
    const cell = queue[head], point = center(cell);
    if (accepts(point)) {
      const cells = [];
      for (let cursor = cell; cursor !== -1; cursor = previous[cursor]) cells.push(cursor);
      return cells.reverse().map(center);
    }
    const x = cell % SIZE, y = Math.floor(cell / SIZE);
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const nx = x + dx, ny = y + dy;
      if (nx < 0 || ny < 0 || nx >= SIZE || ny >= SIZE) continue;
      const next = nx + ny * SIZE;
      if (previous[next] !== -2 || !traversable.cell(next)) continue;
      previous[next] = cell;
      queue[tail++] = next;
    }
  }
  return null;
}

function simplify(from, points, traversable, maxLegPx) {
  const result = [];
  let origin = from, cursor = -1;
  while (cursor < points.length - 1) {
    let nextIndex = -1;
    // A four-connected path along a diagonal wall is a staircase, not a
    // requirement to stop/turn at each 16px corner. Choose the furthest later
    // node whose entire swept hull is clear. Keep the same leg cap and hull
    // predicate; a centre-line shortcut would cut real collision corners.
    for (let i = points.length - 1; i > cursor; i--) {
      if (Math.hypot(points[i].x - origin.x, points[i].y - origin.y) > maxLegPx) continue;
      if (traversable.segment(origin, points[i])) { nextIndex = i; break; }
    }
    if (nextIndex < 0) throw new Error("rendezvous simplification found no bounded clear leg");
    const next = points[nextIndex];
    if (Math.hypot(next.x - origin.x, next.y - origin.y) > 0.01) result.push(next);
    origin = next;
    cursor = nextIndex;
  }
  return result;
}

function routeGeometry(geometry, hullRadiusPx, blockedTiles) {
  const blocked = new Set(blockedTiles.map(p => p.x + p.y * SIZE));
  const clear = (p, radius = hullRadiusPx) => {
    if (!geometry.authoredHullIsTraversable(p.x, p.y, radius)) return false;
    for (let y = Math.floor((p.y - radius - 0.001) / TILE); y <= Math.floor((p.y + radius) / TILE); y++) {
      for (let x = Math.floor((p.x - radius - 0.001) / TILE); x <= Math.floor((p.x + radius) / TILE); x++) {
        if (blocked.has(x + y * SIZE)) return false;
      }
    }
    return true;
  };
  const cache = new Uint8Array(SIZE * SIZE);
  const obstruction = new Uint8Array(SIZE * SIZE);
  const blockedCell = cell => {
    if (!obstruction[cell]) {
      const point = center(cell);
      obstruction[cell] = blocked.has(cell) || !geometry.authoredHullIsTraversable(point.x, point.y, 0) ? 2 : 1;
    }
    return obstruction[cell] === 2;
  };
  const crossesBox = (from, to, minX, minY, maxX, maxY) => {
    let enter = 0, leave = 1;
    for (const [origin, delta, min, max] of [
      [from.x, to.x - from.x, minX, maxX], [from.y, to.y - from.y, minY, maxY],
    ]) {
      if (Math.abs(delta) < 1e-9) { if (origin < min || origin > max) return false; continue; }
      const a = (min - origin) / delta, b = (max - origin) / delta;
      enter = Math.max(enter, Math.min(a, b)); leave = Math.min(leave, Math.max(a, b));
      if (enter > leave) return false;
    }
    return true;
  };
  const traversable = {
    cell: cell => {
      if (!cache[cell]) cache[cell] = traversable.segment(center(cell), center(cell)) ? 1 : 2;
      return cache[cell] === 1;
    },
    segment: (from, to) => {
      if (!clear(from) || !clear(to)) return false;
      // Exact segment versus expanded tile boxes: sampling every four pixels
      // can miss a tiny diagonal corner intersection. Include tangent contact
      // too, retaining conservative steering headroom beyond the game hull.
      const minX = Math.max(0, Math.floor((Math.min(from.x, to.x) - hullRadiusPx - 0.001) / TILE));
      const minY = Math.max(0, Math.floor((Math.min(from.y, to.y) - hullRadiusPx - 0.001) / TILE));
      const maxX = Math.min(SIZE - 1, Math.floor((Math.max(from.x, to.x) + hullRadiusPx) / TILE));
      const maxY = Math.min(SIZE - 1, Math.floor((Math.max(from.y, to.y) + hullRadiusPx) / TILE));
      for (let y = minY; y <= maxY; y++) {
        for (let x = minX; x <= maxX; x++) {
          if (blockedCell(x + y * SIZE) && crossesBox(from, to,
            x * TILE - hullRadiusPx, y * TILE - hullRadiusPx,
            (x + 1) * TILE + hullRadiusPx, (y + 1) * TILE + hullRadiusPx)) return false;
        }
      }
      return true;
    },
  };
  return { clear, traversable };
}

export function planPlayerRendezvous(geometry, origins, {
  hullRadiusPx = 24, stageRadiusPx = 48, maxLegPx = 768, blockedTiles = [], meetingPoint = null,
} = {}) {
  if (origins?.length !== 2 || !origins.every(finitePoint)) throw new Error("rendezvous needs two finite map origins");
  if (![hullRadiusPx, stageRadiusPx, maxLegPx].every(Number.isFinite) || hullRadiusPx < 14 ||
      stageRadiusPx < hullRadiusPx || maxLegPx <= 0) throw new Error("invalid rendezvous clearance or leg limit");
  const { clear, traversable } = routeGeometry(geometry, hullRadiusPx, blockedTiles);
  if (!origins.every(p => clear(p))) throw new Error("rendezvous origin lacks conservative hull clearance");
  const isStage = point => {
    if (!clear(point, stageRadiusPx)) return false;
    for (let y = point.y - stageRadiusPx; y <= point.y + stageRadiusPx; y += TILE) {
      for (let x = point.x - stageRadiusPx; x <= point.x + stageRadiusPx; x += TILE) {
        if (geometry.tileAt(x, y) !== 0) return false;
      }
    }
    return true;
  };
  if (meetingPoint !== null) {
    const gridPoint = finitePoint(meetingPoint) ? center(cellAt(meetingPoint)) : null;
    if (!gridPoint || gridPoint.x !== meetingPoint.x || gridPoint.y !== meetingPoint.y || !isStage(meetingPoint))
      throw new Error("requested rendezvous meeting point is not a clear tile-centred stage");
    const routes = origins.map(origin => {
      const points = routeSearch(origin, point => point.x === meetingPoint.x && point.y === meetingPoint.y, traversable);
      if (!points) throw new Error("no conservative route to requested rendezvous meeting point");
      const drivePoints = simplify(origin, points, traversable, maxLegPx);
      let previous = origin, distancePx = 0;
      for (const point of drivePoints) { distancePx += Math.hypot(point.x - previous.x, point.y - previous.y); previous = point; }
      return { from: { ...origin }, drivePoints, distancePx };
    });
    return { target: { ...meetingPoint }, hullRadiusPx, stageRadiusPx, maxLegPx, routes };
  }
  const first = routeSearch(origins[0], isStage, traversable);
  if (!first) throw new Error("no reachable open rendezvous stage");
  const anchor = first.at(-1);
  const second = routeSearch(origins[1], point => point.x === anchor.x && point.y === anchor.y, traversable);
  if (!second) throw new Error("players have no common conservative rendezvous route");
  // Meet along the already connected route, rather than leave one fresh
  // pilot waiting while the other flies its entire length. Choose a genuine
  // open firing area nearest the path midpoint, not a geometric midpoint
  // which can be inside a wall or another disconnected pocket.
  let middle = second.length - 1, middleDistance = Infinity;
  for (let i = 0; i < second.length; i++) {
    const distance = Math.abs(i - (second.length - 1) / 2);
    if (distance < middleDistance && isStage(second[i])) { middle = i; middleDistance = distance; }
  }
  const target = second[middle];
  const firstToMiddle = first.concat(second.slice(middle).reverse().slice(1));
  const routes = [firstToMiddle, second.slice(0, middle + 1)].map((points, index) => {
    const drivePoints = simplify(origins[index], points, traversable, maxLegPx);
    let previous = origins[index], distancePx = 0;
    for (const point of drivePoints) { distancePx += Math.hypot(point.x - previous.x, point.y - previous.y); previous = point; }
    return { from: { ...origins[index] }, drivePoints, distancePx };
  });
  return { target, hullRadiusPx, stageRadiusPx, maxLegPx, routes };
}

// A fixed authored destination (prize or stationary witness) must not be
// replaced by a shared midpoint or by a guessed straight line through walls.
export function planAuthoredApproach(geometry, origin, destination, {
  hullRadiusPx = 20, maxLegPx = 768, arrivalRadiusPx = 8, blockedTiles = [],
} = {}) {
  if (![origin, destination].every(finitePoint) ||
      ![hullRadiusPx, maxLegPx, arrivalRadiusPx].every(Number.isFinite) ||
      hullRadiusPx < 14 || maxLegPx <= 0 || arrivalRadiusPx < 0 || arrivalRadiusPx > 96) {
    throw new Error("invalid authored approach geometry");
  }
  const { clear, traversable } = routeGeometry(geometry, hullRadiusPx, blockedTiles);
  if (!clear(origin)) throw new Error("authored approach origin lacks conservative hull clearance");
  const points = routeSearch(origin, point =>
    Math.hypot(point.x - destination.x, point.y - destination.y) <= arrivalRadiusPx, traversable);
  if (!points) throw new Error("no hull-clear authored approach route");
  const drivePoints = simplify(origin, points, traversable, maxLegPx);
  let previous = origin, distancePx = 0;
  for (const point of drivePoints) {
    distancePx += Math.hypot(point.x - previous.x, point.y - previous.y); previous = point;
  }
  return { target: points.at(-1), destination: { ...destination }, hullRadiusPx, maxLegPx,
    routes: [{ from: { ...origin }, drivePoints, distancePx }] };
}
