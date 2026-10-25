// Real-key test-pilot planning only. Server collision, damage and lifecycle
// evidence remain the verdict; this constant-velocity forecast is not authority.
export function movingBombOwnerClearance(time, projectileSpeed, explosionRadius) {
  const separation = projectileSpeed * time;
  return { separation, clear: Number.isFinite(time) && time > 0 &&
    Number.isFinite(projectileSpeed) && projectileSpeed > 0 &&
    Number.isFinite(explosionRadius) && explosionRadius > 0 && separation > explosionRadius + 32 };
}

export function movingBombAttackPlan({
  relativeX, relativeY, targetVelocityX, targetVelocityY,
  playerVelocityX, playerVelocityY, projectileSpeed, maximumFlightSeconds,
  explosionRadius, proximityRadius, currentDirection, velocityPathIsClear = null,
  projectilePathIsClear = null,
}) {
  if (![relativeX, relativeY, targetVelocityX, targetVelocityY, playerVelocityX,
    playerVelocityY, projectileSpeed, maximumFlightSeconds, explosionRadius,
    proximityRadius].every(Number.isFinite) || projectileSpeed <= 0 ||
    maximumFlightSeconds <= 0 || explosionRadius <= 0 || proximityRadius <= 0 ||
    !Number.isInteger(currentDirection) || currentDirection < 0 || currentDirection >= 40) {
    throw new Error("invalid moving-bomb pilot geometry");
  }
  const distance = Math.hypot(relativeX, relativeY);
  const fireRange = projectileSpeed * maximumFlightSeconds;
  const desiredRange = Math.min(fireRange * 0.88, explosionRadius * 2 + 32);
  const radialSpeed = Math.max(-160, Math.min(200, (distance - desiredRange) * 1.2));
  let desiredVelocityX = targetVelocityX + (distance > 0 ? relativeX / distance : 1) * radialSpeed;
  let desiredVelocityY = targetVelocityY + (distance > 0 ? relativeY / distance : 0) * radialSpeed;
  const magnitude = Math.hypot(desiredVelocityX, desiredVelocityY);
  if (magnitude > 300) { desiredVelocityX *= 300 / magnitude; desiredVelocityY *= 300 / magnitude; }
  if (velocityPathIsClear != null && typeof velocityPathIsClear !== "function") {
    throw new Error("invalid moving-bomb pilot map predicate");
  }
  if (projectilePathIsClear != null && typeof projectilePathIsClear !== "function") {
    throw new Error("invalid moving-bomb pilot projectile map predicate");
  }
  const paths = [[desiredVelocityX, desiredVelocityY], [-desiredVelocityY, desiredVelocityX],
    [desiredVelocityY, -desiredVelocityX]];
  const selectedPath = paths.find(([x, y]) => !velocityPathIsClear || velocityPathIsClear(x, y));
  const pathBlocked = selectedPath == null;
  if (selectedPath) [desiredVelocityX, desiredVelocityY] = selectedPath;
  const forecast = direction => {
    const angle = direction / 40 * Math.PI * 2;
    const headingX = Math.sin(angle), headingY = -Math.cos(angle);
    const velocityX = playerVelocityX + headingX * projectileSpeed - targetVelocityX;
    const velocityY = playerVelocityY + headingY * projectileSpeed - targetVelocityY;
    const squaredSpeed = velocityX ** 2 + velocityY ** 2;
    const time = squaredSpeed > 0 ? (relativeX * velocityX + relativeY * velocityY) / squaredSpeed : NaN;
    const missDistance = Math.hypot(relativeX - velocityX * time, relativeY - velocityY * time);
    // Both source and bomb inherit the same initial source velocity. Their
    // separation at closest approach is therefore projectileSpeed*time.
    //32px is pilot headroom, not a changed game explosion/proximity radius.
    const clearance = movingBombOwnerClearance(time, projectileSpeed, explosionRadius);
    const predictedOwnerSeparation = clearance.separation;
    const projectileVelocityX = velocityX + targetVelocityX;
    const projectileVelocityY = velocityY + targetVelocityY;
    const clearTrajectory = time > 0 && time <= maximumFlightSeconds &&
      (!projectilePathIsClear || projectilePathIsClear(projectileVelocityX, projectileVelocityY, time));
    return { direction, orientation: (direction + 0.5) / 40, time, missDistance,
      projectileVelocityX, projectileVelocityY, clearTrajectory,
      predictedOwnerSeparation, safeToAttempt: time > 0 && time <= maximumFlightSeconds &&
        missDistance <= Math.min(32, proximityRadius - 8) && clearance.clear && clearTrajectory };
  };
  let candidate = null;
  for (let direction = 0; direction < 40; direction++) {
    const shot = forecast(direction);
    if (shot.safeToAttempt && (!candidate || shot.missDistance < candidate.missDistance)) candidate = shot;
  }
  const current = forecast(currentDirection);
  // Prefer a presently valid wire heading over turning toward a marginally
  // smaller miss in another cell. Both use the same 32px, lifetime, map and
  // owner-clearance limits; chasing the optimum can otherwise prevent fire
  // while the target crosses the boundary between two valid heading cells.
  if (current.safeToAttempt) candidate = current;
  return { distance, desiredRange, fireRange, explosionRadius, desiredVelocityX, desiredVelocityY, pathBlocked,
    velocityError: Math.hypot(desiredVelocityX - playerVelocityX, desiredVelocityY - playerVelocityY),
    candidate, current };
}

export function movingBombNeedsVelocityControl(plan, targetInFireRange) {
  // Desired velocity is a navigation aid, not a firing prerequisite. Do not
  // steer away from a map-clear, blast-safe firing solution to chase a new
  // standoff velocity. Let the ordinary aim branch align that solution first.
  // This does not authorize firing: both callers still require the current
  // wire heading to match the solved cell and pass its own safety forecast.
  const aimableShot = plan.candidate?.safeToAttempt === true;
  return !targetInFireRange || (plan.velocityError > 24 && !aimableShot);
}

export function movingBombControlStep(plan, metrics) {
  const accelX = plan.desiredVelocityX - metrics.playerVelocityX;
  const accelY = plan.desiredVelocityY - metrics.playerVelocityY;
  const error = Math.hypot(accelX, accelY);
  if (error <= 24) return { keys: [], holdMs: 40, action: "coast" };
  const desired = ((Math.atan2(accelX, -accelY) / (Math.PI * 2)) + 1) % 1;
  const delta = orientation => ((orientation - metrics.playerOrientation + 1.5) % 1) - 0.5;
  const forwardTurn = delta(desired), reverseTurn = delta((desired + 0.5) % 1);
  const reverse = Math.abs(reverseTurn) < Math.abs(forwardTurn);
  const turn = reverse ? reverseTurn : forwardTurn;
  if (Math.abs(turn) > 0.025) {
    const keys = [turn > 0 ? "d" : "a"];
    // An inertial ship can keep circling a moving acceleration target while
    // a turn-only controller waits for perfect alignment. Inside 45 degrees,
    // thrust has a positive projection on the desired velocity correction.
    // Steer and accelerate together; firing safety remains caller-owned.
    if (Math.abs(turn) <= 0.125) keys.push(reverse ? "s" : "w");
    return { keys,
      holdMs: Math.min(100, Math.max(24, Math.abs(turn) / 0.625 * 650)),
      action: keys.length > 1 ? "turn-thrust" : "turn" };
  }
  return { keys: [reverse ? "s" : "w"], holdMs: Math.min(55, Math.max(4, error * 2.7)), action: "thrust" };
}
