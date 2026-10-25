// Encounter setup, not a delivery verdict. The unchanged smoke assertions
// still require original authenticated facts on both game and event carriers.
export async function prepareSmokeWeaponEncounter(alpha, beta, server, { navigation = null } = {}) {
  const {
    rendezvousPlayers, driveClientToWorldTarget, orientClientForProtocolDirection,
    exactLiveGameplayScope,
  } = navigation || await import("./combat_replay.mjs");
  const startedAt = Date.now(), deadline = startedAt + 180_000;
  const evidence = { startedAt, keysOnly: true, status: "pending", setupBudgetMs: 180_000 };
  for (const client of [alpha, beta]) (client.smokeEncounterTrace ||= []).push(evidence);
  const scope = metrics => JSON.stringify([metrics?.localGameplayState?.authorityScopeServerSessionId,
    metrics?.localGameplayState?.authorityScopeArenaStreamId, metrics?.selfPlayerId, metrics?.playerLifeSeq]);
  const read = async () => Promise.all([alpha.uiMetrics(), beta.uiMetrics()]);
  try {
    // This certifies a full384px clear authored stage and stops both real-key
    // pilots. Never substitute a fixed-spawn override for a shipped-zone test.
    evidence.rendezvous = await rendezvousPlayers(alpha, beta, server, { stageRadiusPx: 384, recoverOrigins: true });
    const before = await read();
    const scopes = before.map(scope);
    if (!before.every(exactLiveGameplayScope)) throw new Error("smoke encounter lacks live source scopes");
    const stage = evidence.rendezvous.plan.target;
    if (!before.every(metrics => !metrics.inSafeZone &&
      Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) <= 3.5 &&
      Math.hypot(metrics.playerX - stage.x, metrics.playerY - stage.y) <= 16)) {
      throw new Error("smoke encounter did not retain its certified initial stage");
    }
    // Keep hulls and mines well apart, but both peers unambiguously Near.
    // The entire256px leg lies within the certified clear stage.
    const destination = { x: stage.x + 256, y: stage.y };
    const targets = [stage, destination];
    evidence.stationKeeping = [[], []];
    let jointArrival = false, routeFailure = null;
    const retainedScope = (metrics, index) => exactLiveGameplayScope(metrics) &&
      scope(metrics) === scopes[index] && !metrics.inSafeZone;
    const settled = (metrics, index) => Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) <= 3.5 &&
      Math.hypot(metrics.playerX - targets[index].x, metrics.playerY - targets[index].y) <= 4;
    const observeJointArrival = async () => {
      if (routeFailure) throw routeFailure;
      if (Date.now() >= deadline) throw new Error("smoke encounter exhausted its setup budget");
      if (jointArrival) return true;
      const states = await read();
      if (jointArrival) return true;
      if (!states.every(retainedScope)) throw new Error("smoke encounter station keeping crossed a lifecycle boundary");
      jointArrival = states.every(settled);
      if (jointArrival) evidence.jointArrival = states.map((metrics, index) => ({
        scope: scope(metrics), x: metrics.playerX, y: metrics.playerY,
        velocityX: metrics.playerVelocityX, velocityY: metrics.playerVelocityY, target: targets[index] }));
      return jointArrival;
    };
    const flights = await Promise.allSettled([alpha, beta].map(async (client, index) => {
      try {
        while (!jointArrival) {
          if (await observeJointArrival()) break;
          const current = await client.uiMetrics();
          if (!retainedScope(current, index)) throw new Error("smoke encounter lost a live station-keeping scope");
          if (settled(current, index)) {
            await new Promise(resolve => setTimeout(resolve, 50));
            continue;
          }
          const remaining = deadline - Date.now();
          if (remaining <= 0) throw new Error("smoke encounter exhausted its setup budget");
          const leg = await driveClientToWorldTarget(client, targets[index],
            Math.min(30_000, remaining), true, 4, observeJointArrival, 160, 0.5);
          evidence.stationKeeping[index].push(leg);
          if (!leg.reached) throw new Error("smoke encounter station-keeping flight did not settle");
          if (!leg.completed) {
            const stopped = await client.uiMetrics();
            if (!retainedScope(stopped, index) || Math.hypot(stopped.playerVelocityX, stopped.playerVelocityY) > 3.5) {
              throw new Error("smoke encounter driver did not preserve its stopped live receipt");
            }
          }
        }
      } catch (error) { routeFailure ||= error; throw error; }
    }));
    const failures = flights.filter(result => result.status === "rejected").map(result => result.reason);
    if (failures.length) throw new AggregateError(failures, "smoke encounter joint station keeping failed");
    evidence.separationFlight = evidence.stationKeeping[1].at(-1) ?? null;
    // Parallel firing lanes prevent the transport probe itself from killing
    // a peer. These are genuine heading inputs, not altered aim/weapon state.
    evidence.aim = await Promise.all([alpha, beta].map(client =>
      orientClientForProtocolDirection(client, 0, -1, Math.min(4_000, Math.max(1, deadline - Date.now())))));
    const after = await read();
    evidence.final = after.map(metrics => ({ scope: scope(metrics), x: metrics.playerX, y: metrics.playerY,
      velocityX: metrics.playerVelocityX, velocityY: metrics.playerVelocityY,
      orientation: metrics.playerOrientation }));
    evidence.separationPx = Math.hypot(after[0].playerX - after[1].playerX, after[0].playerY - after[1].playerY);
    if (Date.now() > deadline || !evidence.aim.every(aim => aim.oriented) ||
        !after.every((metrics, index) => exactLiveGameplayScope(metrics) && scope(metrics) === scopes[index] &&
          !metrics.inSafeZone && Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) <= 3.5 &&
          Math.hypot(metrics.playerX - (index ? destination.x : stage.x), metrics.playerY - stage.y) <= 16) ||
        evidence.separationPx < 224 || evidence.separationPx > 288) {
      throw new Error(`smoke encounter did not preserve stopped separated live peers: ${JSON.stringify(evidence.final)}`);
    }
    evidence.status = "complete";
    return evidence;
  } catch (error) {
    evidence.status = "failed";
    evidence.error = String(error.message);
    throw error;
  } finally {
    evidence.finishedAt = Date.now();
  }
}
