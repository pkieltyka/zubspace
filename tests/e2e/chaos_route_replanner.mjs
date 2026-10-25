// Keep the relocation recovery policy independently testable. The browser lane supplies real key-driven legs and
// a fresh route from the live position; focused tests can force the otherwise intermittent invalidation.
export async function driveRouteWithReplans({
  initialDrivePoints,
  maxReplans,
  driveLeg,
  currentPosition,
  planFrom,
}) {
  let drivePoints = initialDrivePoints.map((point) => ({ ...point }));
  let replans = 0;
  const legs = [];

  for (let index = 0; index < drivePoints.length; index += 1) {
    const waypoint = drivePoints[index];
    try {
      legs.push(await driveLeg(waypoint, index));
    } catch (error) {
      const from = await currentPosition();
      const replanned = replans < maxReplans && Number.isFinite(from?.x) && Number.isFinite(from?.y)
        ? await planFrom(from)
        : null;
      if (!replanned || !Array.isArray(replanned.drivePoints) || replanned.drivePoints.length === 0) {
        throw error;
      }

      replans += 1;
      legs.push({
        waypoint,
        reached: false,
        replanned: true,
        replan: replans,
        from: { x: from.x, y: from.y },
        reason: String(error?.message ?? error).slice(0, 400),
      });
      drivePoints = replanned.drivePoints.map((point) => ({ ...point }));
      index = -1;
    }
  }

  return { legs, replans };
}
