import { runwayAlongHeading } from "./authored_map_geometry.mjs";
import { planAuthoredApproach, planPlayerRendezvous } from "./player_rendezvous.mjs";
import { planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";

// Keep both pilots outside the exact collectible until all long travel is
// finished. This plans ordinary keyboard setup; it never moves or extends a
// prize, changes its identity, or starts a gameplay/application timer.
export function planTimedPrizePreparation(geometry, origins, prize, currentTick, {
  blockedTiles = [], avoidTiles = [], shooterStandoffPx = null, repelPickup = false,
} = {}) {
  const prizeTile = { x: Math.floor(prize.x / 16), y: Math.floor(prize.y / 16) };
  const otherAvoidTiles = avoidTiles.filter(tile => tile.x !== prizeTile.x || tile.y !== prizeTile.y);
  const navigation = planNaturalPrizeNavigation(geometry, origins[0], prize, currentTick,
    blockedTiles.concat(otherAvoidTiles));
  if (!navigation) return null;
  const preparationTiles = blockedTiles.concat(avoidTiles, [prizeTile]);
  let stage = planAuthoredApproach(geometry, origins[0], prize, {
    arrivalRadiusPx: 64, blockedTiles: preparationTiles,
  }).target;
  let repelLaunch = null;
  if (repelPickup) {
    if (shooterStandoffPx !== null) throw new Error("conflicting timed-prize launch geometry");
    // Repel pushes away from its source. Overlapping pilots produce an
    // arbitrary launch angle and a500px/s detour. Prepare a real, clear line
    // through the collectible instead; no impulse or position is injected.
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const actor = { x: prize.x + dx * 64, y: prize.y + dy * 64 };
      const peer = { x: prize.x + dx * 160, y: prize.y + dy * 160 };
      try {
        const pickup = planAuthoredApproach(geometry, actor, prize, {
          arrivalRadiusPx: 16, blockedTiles: blockedTiles.concat(otherAvoidTiles),
        });
        const source = planAuthoredApproach(geometry, actor, peer, {
          arrivalRadiusPx: 0, blockedTiles: preparationTiles,
        });
        if (pickup.routes[0].drivePoints.length !== 1 || source.routes[0].drivePoints.length !== 1) continue;
        // The receipt stays held after contact. Reserve1500px of unchanged
        // terrain for the stock1125px Repel impulse plus braking/coast; the
        // short pickup ray alone can end inside a naturally open door whose
        // later close legitimately relocates the target before Repel B.
        const clearRunoutPx = 1500;
        const runout = runwayAlongHeading({
          hullIsTraversable: (x, y) => geometry.authoredHullIsTraversable(x, y, 20),
          safetyDistance: () => Infinity, x: actor.x, y: actor.y,
          dx: -dx, dy: -dy, travelPx: clearRunoutPx, stepPx: 4,
        });
        if (runout?.runwayPx !== clearRunoutPx) continue;
        stage = actor; repelLaunch = { actor, peer, source, pickup, clearRunoutPx }; break;
      } catch (error) {
        if (!/no hull-clear authored approach route|origin lacks conservative hull clearance/.test(error.message)) throw error;
      }
    }
    if (!repelLaunch) throw new Error("no conservative timed-prize Repel launch");
  }
  const plan = planPlayerRendezvous(geometry, origins, {
    hullRadiusPx: 20, stageRadiusPx: 20, meetingPoint: stage, blockedTiles: preparationTiles,
  });
  const pickupPlan = planAuthoredApproach(geometry, stage, prize, {
    arrivalRadiusPx: 16, blockedTiles: blockedTiles.concat(otherAvoidTiles),
  });
  let shooterStandoff = null, shooterRoute = null, firingLine = null;
  if (shooterStandoffPx !== null) {
    if (!Number.isInteger(shooterStandoffPx) || shooterStandoffPx < 96 || shooterStandoffPx > 384 || shooterStandoffPx % 16)
      throw new Error("invalid timed-prize shooter standoff");
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const candidate = { x: pickupPlan.target.x + dx * shooterStandoffPx,
        y: pickupPlan.target.y + dy * shooterStandoffPx };
      try {
        const line = planAuthoredApproach(geometry, pickupPlan.target, candidate, {
          arrivalRadiusPx: 0, blockedTiles: blockedTiles.concat(otherAvoidTiles),
        });
        // A detour is a valid travel route but not an unobstructed bomb line.
        if (line.routes[0].drivePoints.length !== 1) continue;
        const route = planAuthoredApproach(geometry, stage, candidate, {
          arrivalRadiusPx: 0, blockedTiles: preparationTiles,
        });
        shooterStandoff = candidate; shooterRoute = route; firingLine = line; break;
      } catch (error) {
        if (!/no hull-clear authored approach route|origin lacks conservative hull clearance/.test(error.message)) throw error;
      }
    }
    if (!shooterStandoff) throw new Error("no conservative timed-prize shooter standoff");
  }
  const distancePx = Math.max(...plan.routes.map(route => route.distancePx));
  // Conservative60px/s plus12s for turns/settling and the short final pickup.
  // The two pilots travel concurrently. Both the original prize TTL and the
  // existing90s navigation bound must cover setup; no deadline is prolonged.
  const requiredTtlTicks = Math.max(navigation.requiredTtlTicks, Math.ceil(distancePx / 60 * 100) + 1_200 +
    (shooterRoute ? Math.ceil(shooterRoute.routes[0].distancePx / 60 * 100) + 800 : 0) +
    (repelLaunch ? Math.ceil(repelLaunch.source.routes[0].distancePx / 60 * 100) + 800 : 0));
  if (requiredTtlTicks > 9_000 || navigation.remainingTicks < requiredTtlTicks) return null;
  return { plan, navigation, pickupPlan, shooterStandoff, shooterRoute, firingLine, repelLaunch,
    requiredTtlTicks, remainingTicks: navigation.remainingTicks,
    setupBudgetMs: requiredTtlTicks * 10, avoidTiles: avoidTiles.concat([prizeTile]),
    pickupAvoidTiles: otherAvoidTiles };
}
