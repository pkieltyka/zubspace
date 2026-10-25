import path from "node:path";
import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { exactLiveGameplayScope, waitUntil, driveClientToWorldTarget } from "./combat_replay.mjs";
import { loadRendezvousMap, planAuthoredApproach } from "./player_rendezvous.mjs";

// Setup flight only: real keys, exact life, original bounded leg driver and
// conservative map geometry. No position setter or client/server packet hook.
export async function navigateAuthoredTarget(client, destination, server = null, {
  timeoutMs = 90_000, arrivalRadiusPx = 8, complete = null,
  maximumDesiredSpeed = 42, desiredSpeedScale = 0.25,
  avoidTiles = [],
} = {}) {
  const startedAt = Date.now(), deadline = startedAt + timeoutMs;
  const read = async () => (await client.metricsState()).uiMetrics;
  let initial;
  await waitUntil("exact live scope before authored navigation", async () => {
    initial = await read(); return exactLiveGameplayScope(initial);
  }, Math.min(timeoutMs, 30_000));
  const scope = m => JSON.stringify([m.localGameplayState.authorityScopeServerSessionId,
    m.localGameplayState.authorityScopeArenaStreamId, m.selfPlayerId, m.playerLifeSeq]);
  const sourceScope = scope(initial);
  const verify = async () => {
    const m = await read();
    if (!exactLiveGameplayScope(m) || scope(m) !== sourceScope) throw new Error("authored navigation crossed a lifecycle boundary");
    return m;
  };
  if (initial.localGameplayState.brickTilesOverflow) throw new Error("incomplete brick geometry for authored navigation");
  const arena = await client.evaluate("window.__zubspaceArena");
  const loaded = loadRendezvousMap(arena, [
    ...((server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT) ?
      [server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
    path.join(repoRoot, "assets/zones"),
  ]);
  const plan = planAuthoredApproach(loaded.geometry, { x: initial.playerX, y: initial.playerY }, destination,
    { arrivalRadiusPx, blockedTiles: (initial.localGameplayState.brickTiles || []).concat(avoidTiles) });
  const evidence = { startedAt, keysOnly: true, scope: sourceScope, map: loaded.provenance, plan, legs: [] };
  (client.rendezvousTrace ||= []).push(evidence);
  try {
    for (const target of plan.routes[0].drivePoints) {
      await verify();
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new Error("authored navigation exceeded its setup budget");
      const leg = await driveClientToWorldTarget(client, target, Math.min(30_000, remaining), true, 3, async () => {
        await verify(); return complete ? await complete() : false;
      }, maximumDesiredSpeed, desiredSpeedScale);
      evidence.legs.push(leg);
      if (!leg.reached) throw new Error("genuine authored navigation did not reach its planned leg");
      if (leg.completed) break;
    }
    const final = await verify();
    evidence.final = { x: final.playerX, y: final.playerY,
      velocityX: final.playerVelocityX, velocityY: final.playerVelocityY };
    evidence.reached = Math.hypot(final.playerX - destination.x, final.playerY - destination.y) <= arrivalRadiusPx + 4;
    evidence.completed = complete ? Boolean(await complete()) : false;
    if (!evidence.reached && !evidence.completed) throw new Error("authored navigation missed its destination");
    if (Date.now() > deadline) throw new Error("authored navigation exceeded its setup budget");
    evidence.status = "complete"; return evidence;
  } catch (error) {
    evidence.status = "failed"; evidence.error = error.message; throw error;
  } finally { evidence.finishedAt = Date.now(); }
}
