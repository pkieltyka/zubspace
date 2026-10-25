// Read-only geometry setup for the controlled brick scenario. Clearance is
// measured before driving; neither safety tiles nor weapon rules are changed.
export function brickWeaponStage(geometry, wall, x = 8200) {
  if (wall.length !== 10 || new Set(wall.map(tile => `${tile.x},${tile.y}`)).size !== 10 ||
      !wall.every(tile => Number.isInteger(tile.x) && Number.isInteger(tile.y) && tile.y === wall[0].y)) {
    throw new Error("brick weapon staging needs one ten-tile horizontal wall");
  }
  const wallTop = wall[0].y * 16, wallBottom = wallTop + 16;
  const wallLeft = Math.min(...wall.map(tile => tile.x)) * 16;
  const wallRight = (Math.max(...wall.map(tile => tile.x)) + 1) * 16;
  if (wallRight - wallLeft !== 160 || x - 24 <= wallLeft || x + 24 >= wallRight) {
    throw new Error("brick weapon staging needs an interior firing column");
  }
  const clear = point => {
    if (!geometry.authoredHullIsTraversable(point.x, point.y, 32)) return false;
    for (let y = point.y - 32; y <= point.y + 32; y += 16) {
      for (let x = point.x - 32; x <= point.x + 32; x += 16) {
        if (geometry.tileAt(x, y) !== 0) return false;
      }
    }
    return true;
  };
  const gunner = { x, y: wallTop - 48 }, hostile = { x, y: wallBottom + 96 };
  if (!clear(gunner) || !clear(hostile)) throw new Error("brick firing stage intersects authored safety or terrain");
  // Keep at least the original144px far-side sensor margin. If that point
  // crosses the unchanged safety strip, move farther away, never closer.
  let thorTarget = null;
  for (let offset = 0; offset <= 128; offset += 16) {
    const point = { x, y: wallTop - 144 - offset };
    if (clear(point)) { thorTarget = point; break; }
  }
  if (!thorTarget) throw new Error("no clear Thor target beyond the original sensor margin");
  return { wallTop, wallBottom, wallLeft, wallRight, gunner, hostile, thorTarget };
}
