import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { exactLiveGameplayScope, waitUntil } from "./combat_replay.mjs";

export function botEncounterStageState(metrics, botIds, {
  requireEveryBot = false, allowSafeSource = false, radiusPx = 300,
  readiness = "sprite",
} = {}) {
  if (!Number.isFinite(radiusPx) || radiusPx <= 0 || radiusPx > 300 || !botIds.length ||
      !["sprite", "radar"].includes(readiness)) {
    throw new Error("invalid bot encounter staging bounds");
  }
  const targets = (metrics.remotePlayers || [])
    .filter((candidate) => botIds.includes(candidate.id) &&
      Number.isInteger(candidate.lifeSeq) && candidate.energy > 0 &&
      Number.isFinite(candidate.positionX) && Number.isFinite(candidate.positionY))
    .map((candidate) => ({ ...candidate,
      distance: Math.hypot(candidate.positionX - metrics.playerX, candidate.positionY - metrics.playerY) }))
    .sort((left, right) => left.distance - right.distance);
  const radarIds = new Set(metrics.radarPlayerIds || []);
  const readyIds = new Set(targets.filter(target => readiness === "radar"
    ? radarIds.has(target.id) && (metrics.authoritativeSnapshotActors || []).some(actor =>
        actor.id === target.id && actor.lifeSeq === target.lifeSeq)
    : target.distance <= radiusPx).map(target => target.id));
  return { targets, ready: metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
    Number.isFinite(metrics.playerX) && Number.isFinite(metrics.playerY) &&
    (allowSafeSource || metrics.inSafeZone !== true) &&
    (readiness === "radar" || requireEveryBot
      ? botIds.every(id => readyIds.has(id)) : readyIds.size > 0) };
}

// Encounter preparation only. Actual sprite/Repel/combat proofs retain their
// own unmodified scopes, durations, geometry, exact-life and delivery floors.
export async function approachLiveBotEncounter({ actor, server, botIds, label,
  timeoutMs = 90_000, requireEveryBot = false, allowSafeSource = false,
  readiness = "sprite",
  positionIsOnAuthoredSafeTile = () => false,
  searchPoint = null,
}) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 90_000) {
    throw new Error("invalid bot encounter approach budget");
  }
  if (searchPoint !== null && (![searchPoint.x, searchPoint.y].every(Number.isFinite) ||
    searchPoint.x < 0 || searchPoint.y < 0 || searchPoint.x >= 16_384 || searchPoint.y >= 16_384)) {
    throw new Error("invalid authored bot encounter search point");
  }
  const startedAt = Date.now(), deadline = startedAt + timeoutMs;
  const approaches = [];
  const read = async () => (await actor.metricsState()).uiMetrics || {};
  const stage = metrics => {
    const current = botEncounterStageState({ ...metrics,
      remotePlayers: (metrics.remotePlayers || []).filter(target =>
        !positionIsOnAuthoredSafeTile(target.positionX, target.positionY)),
    }, botIds, { requireEveryBot, allowSafeSource, readiness });
    current.ready &&= exactLiveGameplayScope(metrics);
    return current;
  };
  await waitUntil(`${label} approaches a live bot encounter`, async () => {
    const metrics = await read(), current = stage(metrics);
    if (current.ready) return true;
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) return false;
    const searching = current.targets.length === 0;
    if (searching && searchPoint === null) return false;
    const points = requireEveryBot ? current.targets : [current.targets[0]];
    // A declared search point is only a real-key route towards an authored
    // combat region. It never substitutes for an actually presented live bot.
    const destination = searching ? searchPoint : {
      x: points.reduce((total, target) => total + target.positionX, 0) / points.length,
      y: points.reduce((total, target) => total + target.positionY, 0) / points.length,
    };
    const remaining = deadline - Date.now();
    if (remaining <= 0) throw new Error(`${label} exceeded its bounded encounter approach budget`);
    try {
      const navigation = await navigateAuthoredTarget(actor, destination, server, {
        timeoutMs: remaining, arrivalRadiusPx: 96,
        maximumDesiredSpeed: searching ? 250 : 100, desiredSpeedScale: searching ? 0.8 : 0.5,
        complete: async () => {
          const latest = stage(await read());
          return latest.ready || (searching && latest.targets.length > 0);
        },
      });
      approaches.push({ ...navigation, purpose: searching ? "authored-region-search" : "live-bot-approach" });
    } catch (error) {
      const after = await read();
      const retired = after.playerLifeSeq !== metrics.playerLifeSeq ||
        after.playerAlive !== true || after.serverPlayerAlive !== true;
      if (error.message !== "authored navigation crossed a lifecycle boundary" || !retired) throw error;
    }
    return stage(await read()).ready;
  }, timeoutMs);
  if (Date.now() > deadline) throw new Error(`${label} exceeded its bounded encounter approach budget`);
  return { startedAt, finishedAt: Date.now(), keysOnly: true, requireEveryBot, readiness,
    searchPoint: searchPoint === null ? null : { x: searchPoint.x, y: searchPoint.y }, approaches };
}
