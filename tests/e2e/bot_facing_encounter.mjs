import { facingBearingDiversityStep } from './bot_facing.mjs';
import { movingHullPathIsClear, routedMapCounterfireWaypoint } from './bot_combat_evidence.mjs';

// Read-only setup geometry, matching the existing stationary-fixture lane.
// The native bot's avoidance probe must allow its firing heading; an empty
// centre ray alone is insufficient beside a wall.
export function facingBotAimIsClear(geometry, fromX, fromY, toX, toY) {
  const dx = toX - fromX, dy = toY - fromY;
  if (![fromX, fromY, toX, toY].every(Number.isFinite) || Math.hypot(dx, dy) <= 0.01) return false;
  const turns = Math.atan2(dx, -dy) / (2 * Math.PI);
  const direction = Math.round(((turns % 1) + 1) % 1 * 40) % 40;
  const angle = direction / 40 * 2 * Math.PI;
  const radius = 14, distance = radius + 40, steps = Math.ceil(distance / radius);
  for (let step = 1; step <= steps; step++) {
    if (!geometry.authoredHullIsOpen(fromX + Math.sin(angle) * distance * step / steps,
      fromY - Math.cos(angle) * distance * step / steps, radius)) return false;
  }
  return true;
}

export function facingLaunchStep({metrics, target, cycle, geometry}) {
  const distance = Math.hypot(target.positionX - metrics.playerX, target.positionY - metrics.playerY);
  const botAim = (x, y, toX, toY) => facingBotAimIsClear(geometry, x, y, toX, toY);
  // A bot already outside safety may still be a stationary fixture hundreds
  // of pixels beyond its bullet lifetime. Approach before orbiting or waiting.
  if (metrics.inSafeZone === true || distance > 180 ||
      !geometry.authoredLineIsClear(metrics.playerX, metrics.playerY, target.positionX, target.positionY) ||
      !botAim(target.positionX, target.positionY, metrics.playerX, metrics.playerY)) {
    const waypoint = routedMapCounterfireWaypoint({metrics, target, ...geometry,
      authoredTargetAimIsClear: botAim});
    return {phase: waypoint ? 'approach' : 'blocked', distance, waypoint};
  }
  const diversity = facingBearingDiversityStep({cycle, playerX: metrics.playerX, playerY: metrics.playerY,
    targetX: target.positionX, targetY: target.positionY});
  if (diversity && geometry.authoredHullIsOpen(diversity.x, diversity.y) &&
      geometry.authoredLineIsClear(target.positionX, target.positionY, diversity.x, diversity.y) &&
      botAim(target.positionX, target.positionY, diversity.x, diversity.y) &&
      movingHullPathIsClear({x: metrics.playerX, y: metrics.playerY,
        velocityX: diversity.x - metrics.playerX, velocityY: diversity.y - metrics.playerY,
        seconds: 1, hullIsOpen: geometry.authoredHullIsTraversable})) {
    return {phase: 'diversity', distance, waypoint: diversity};
  }
  return {phase: 'observe', distance, waypoint: null};
}
