// Shot-planning evidence only. The final server/browser collision correlation
// remains the acceptance check; current movement cannot prove a future hit.
export const MATERIAL_BOMB_MOTION_PIXELS = 64;

export function movingBombShotDisposition(anchor, target) {
  if (anchor == null) return {action: 'first-shot', ready: true};
  if (target?.id !== anchor.target_id || target?.lifeSeq !== anchor.target_life_seq) {
    return {action: 'different-life', ready: false};
  }
  const values = [anchor.collision_anchor_x, anchor.collision_anchor_y,
    target.displayX, target.displayY];
  if (!values.every(Number.isFinite)) return {action: 'missing-position', ready: false};
  const travel = Math.hypot(target.displayX - anchor.collision_anchor_x,
    target.displayY - anchor.collision_anchor_y);
  return {action: travel >= MATERIAL_BOMB_MOTION_PIXELS ? 'aim' : 'separate',
    ready: travel >= MATERIAL_BOMB_MOTION_PIXELS, travel};
}
