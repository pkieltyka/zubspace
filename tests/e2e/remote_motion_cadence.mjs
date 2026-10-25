// Core-test kinematic bound, not gameplay interpolation or authority policy.
// Two integer-pixel rounded endpoints contribute at most sqrt(2) pixels of
// difference error; two hundredth-pixel diagnostic endpoints add sqrt(2)/100.
const positionDifferenceQuantization = Math.SQRT2 * 1.01;

export function remoteDisplayStepEvidence(previous, current) {
  const start = previous.monotonicMs ?? previous.at;
  const end = current.monotonicMs ?? current.at;
  const dtMs = end - start;
  const finite = [start, end, previous.displayX, previous.displayY,
    current.displayX, current.displayY, previous.velocityX, previous.velocityY,
    current.velocityX, current.velocityY].every(Number.isFinite);
  if (!finite || dtMs <= 0) return { ok: false, reason: "invalid-render-interval", dtMs };
  // Same render-stall limit as the existing sustained presentation check.
  if (dtMs >= 80) return { ok: false, reason: "render-stall", dtMs };
  const speed = Math.max(Math.hypot(previous.velocityX, previous.velocityY),
    Math.hypot(current.velocityX, current.velocityY));
  const seconds = dtMs / 1000;
  const expectedTravel = speed * seconds;
  // Snapshot velocities have tenth-pixel/s precision; bound both components.
  const quantization = positionDifferenceQuantization + Math.SQRT2 * 0.1 * seconds;
  const limit = Math.max(6, expectedTravel + quantization);
  const distance = Math.hypot(current.displayX - previous.displayX,
    current.displayY - previous.displayY);
  return { ok: distance <= limit, reason: distance <= limit ? null : "unexplained-displacement",
    dtMs, speed, expectedTravel, quantization, limit, distance };
}
