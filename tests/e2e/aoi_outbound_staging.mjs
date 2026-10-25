const brakeStartDistance = 900;
const minimumActorTravel = 256;

function position(metrics) {
  const { playerX: x, playerY: y } = metrics || {};
  if (!Number.isFinite(x) || !Number.isFinite(y)) {
    throw new Error(`lossy AOI staging requires finite positions: ${JSON.stringify(metrics)}`);
  }
  return { x, y };
}

// Limit each westward leg's speed before turning to brake. A full-speed
// 650px cruise drifts another ~490px while braking, exhausting the Near leg.
export async function stageAoiWatcher({ initial, alignWest, holdUntil, brake }) {
  const origin = position(initial);
  let metrics = initial;
  const legs = [];
  for (let attempt = 0; origin.x - position(metrics).x < 450; attempt += 1) {
    if (attempt >= 20) {
      throw new Error(`lossy AOI watcher made insufficient westward staging progress: ${JSON.stringify({ origin, legs })}`);
    }
    await alignWest(metrics);
    await holdUntil((state) => {
      const current = state.uiMetrics;
      return current?.playerAlive === true && (
        origin.x - position(current).x >= 450 ||
        Math.hypot(current.playerVelocityX, current.playerVelocityY) >= 50
      );
    });
    const stopped = await brake();
    if (!stopped.braked) {
      throw new Error(`lossy AOI watcher could not stop while staging: ${JSON.stringify(stopped)}`);
    }
    metrics = stopped.metrics;
    legs.push({ ...position(metrics), speed: stopped.speed });
  }
  return { origin, metrics, legs };
}

export function aoiOutboundProgress(actorMetrics, watcherMetrics) {
  const actorStart = position(actorMetrics);
  const watcher = position(watcherMetrics);
  const initialDistance = Math.hypot(actorStart.x - watcher.x, actorStart.y - watcher.y);
  if (initialDistance > brakeStartDistance - minimumActorTravel) {
    throw new Error(`lossy AOI outbound staging exhausted the Near motion runway: ${JSON.stringify({
      actorStart, watcher, initialDistance, brakeStartDistance, minimumActorTravel,
    })}`);
  }
  const measure = (metrics) => {
    const current = position(metrics);
    return {
      actorTravel: Math.hypot(current.x - actorStart.x, current.y - actorStart.y),
      watcherDistance: Math.hypot(current.x - watcher.x, current.y - watcher.y),
    };
  };
  return {
    actorStart, watcher, initialDistance, brakeStartDistance, minimumActorTravel,
    measure,
    reached(metrics) {
      const { actorTravel, watcherDistance } = measure(metrics);
      return metrics.playerAlive === true && actorTravel >= minimumActorTravel && watcherDistance > brakeStartDistance;
    },
  };
}
