// Read-only test setup. Dynamic doors are blocked by the authored geometry,
// regardless of their current open phase. A free-motion measurement must not
// accidentally measure the legitimate relocation caused by a closing door.
export function uninterruptedMotionStage(geometry, origins, { marginPx = 384, searchPx = 768 } = {}) {
  if (!origins?.length || !origins.every(p => Number.isFinite(p.x) && Number.isFinite(p.y))) {
    throw new Error("motion staging needs finite pilot origins");
  }
  const center = origins[0];
  const candidates = [];
  for (let y = Math.floor((center.y - searchPx) / 16) * 16 + 8;
      y <= center.y + searchPx; y += 16) {
    for (let x = Math.floor((center.x - searchPx) / 16) * 16 + 8;
        x <= center.x + searchPx; x += 16) {
      candidates.push({ x, y, distance: Math.hypot(x - center.x, y - center.y) });
    }
  }
  candidates.sort((a, b) => a.distance - b.distance || a.y - b.y || a.x - b.x);
  for (const candidate of candidates) {
    if (!geometry.authoredHullIsTraversable(candidate.x, candidate.y, marginPx)) continue;
    // Safety changes thrust, so even traversable safe tiles are unsuitable.
    let empty = true;
    for (let y = candidate.y - marginPx; empty && y <= candidate.y + marginPx; y += 16) {
      for (let x = candidate.x - marginPx; x <= candidate.x + marginPx; x += 16) {
        if (geometry.tileAt(x, y) !== 0) { empty = false; break; }
      }
    }
    if (!empty || !origins.every(p => Math.hypot(p.x - candidate.x, p.y - candidate.y) < 1 ||
        geometry.authoredLineIsClear(p.x, p.y, candidate.x, candidate.y))) continue;
    return { x: candidate.x, y: candidate.y, marginPx };
  }
  throw new Error("no authored uninterrupted-motion staging area is reachable");
}
