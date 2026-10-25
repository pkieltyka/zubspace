import { planAuthoredApproach } from "./player_rendezvous.mjs";

// The fixture pilots a Warbird (14px hull). A point within16px of a prize,
// plus the driver's3px endpoint tolerance, is inside the actual22px overlap
// range. The prize centre itself can be too close to terrain for a ship.
export const NATURAL_PRIZE_ARRIVAL_RADIUS_PX = 16;

export function planNaturalPrizeNavigation(geometry, origin, prize, currentTick, blockedTiles = []) {
  if (!Number.isInteger(currentTick) || !Number.isInteger(prize.expiresAtTick)) {
    throw new Error("natural prize route requires exact lifetime ticks");
  }
  const remainingTicks = (prize.expiresAtTick - currentTick) & 0x7fffffff;
  if (remainingTicks === 0 || remainingTicks >= 0x40000000) return null;
  const plan = planAuthoredApproach(geometry, origin, prize, {
    arrivalRadiusPx: NATURAL_PRIZE_ARRIVAL_RADIUS_PX, blockedTiles,
  });
  // Budget the real detour at a conservative60px/s plus8sec for turns and
  // settling. Keep the existing12sec minimum TTL, and bound setup at90sec.
  // This never extends the collectible's original server lifetime.
  const requiredTtlTicks = Math.max(1_200, Math.ceil(plan.routes[0].distancePx / 60 * 100) + 800);
  if (requiredTtlTicks > 9_000 || remainingTicks < requiredTtlTicks) return null;
  return { plan, remainingTicks, requiredTtlTicks, setupBudgetMs: requiredTtlTicks * 10 };
}

// First choose the correct approach side from the TTL-qualified full route,
// then stage close to the pickup from that side. A radial search from the
// original spawn can stop across a wall; stopping at a distant waypoint can
// instead collect unrelated greens after the target-specific baseline.
export function naturalPrizePreclaimStage(geometry, navigation, blockedTiles = []) {
  const { plan } = navigation;
  // A single bounded leg can still be hundreds of pixels long and cross
  // unrelated greens. Its real origin supplies the selected approach side
  // when there is no intermediate waypoint; only a genuinely near origin
  // may skip this setup before the target-specific baseline.
  const side = plan.routes[0].drivePoints.slice(0, -1)
    .reverse().concat([plan.routes[0].from]).find(point =>
      Math.hypot(point.x - plan.destination.x, point.y - plan.destination.y) > 96
    );
  if (!side) return null;
  return planAuthoredApproach(geometry, side, plan.destination, {
    hullRadiusPx: plan.hullRadiusPx, arrivalRadiusPx: 64, blockedTiles,
  }).target;
}
