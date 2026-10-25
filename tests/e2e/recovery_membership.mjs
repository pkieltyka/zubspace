// Actor membership is spatial. Distant live spawns may correctly see only
// themselves; require the peer after the genuine-key fixture approach, before
// measured impairment, and match its exact current life rather than any ID.
export function recoveryMembershipReady(states, expected) {
  if (!Array.isArray(states) || states.length !== 2 ||
      !Array.isArray(expected) || expected.length !== 2 ||
      !expected.every(scope => Number.isInteger(scope?.id) && Number.isInteger(scope.lifeSeq)) ||
      expected[0].id === expected[1].id) return false;
  return states.every((state, index) => {
    const metrics = state?.uiMetrics || {}, self = expected[index], peer = expected[1 - index];
    return metrics.selfPlayerId === self.id && metrics.playerLifeSeq === self.lifeSeq &&
      metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
      (metrics.authoritativeSnapshotActors || []).some(actor =>
        actor.id === peer.id && actor.lifeSeq === peer.lifeSeq);
  });
}
