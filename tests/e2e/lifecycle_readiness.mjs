// Shared unchanged startup acceptance predicate; no service/browser side effects.
export function lifecycleIsReady(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return metrics.playerAlive === true &&
    metrics.serverPlayerAlive === true &&
    Number.isInteger(metrics.selfPlayerId) &&
    Number.isInteger(metrics.playerLifeSeq) &&
    Number.isFinite(metrics.serverPlayerEnergy) &&
    local.zoneFilesReady === true &&
    local.zoneMapReady === true &&
    local.pendingShipResetPosition === false &&
    local.pendingShipResetDeliveryId == null &&
    (local.lifecycleBaselineReceiptsMatched ?? 0) >= 1 &&
    (local.lifecycleBaselineReceiptsUnmatched ?? 0) === 0 &&
    local.authorityPhase === "Live" &&
    local.authorityTransitionKind == null &&
    Number.isInteger(local.authoritySegment) &&
    typeof local.authorityScopeServerSessionId === "string" &&
    local.authorityScopeServerSessionId !== "0" &&
    typeof local.authorityScopeArenaStreamId === "string" &&
    local.authorityScopeArenaStreamId !== "0" &&
    local.authorityScopePlayerId === metrics.selfPlayerId &&
    local.authorityScopeLifeSeq === metrics.playerLifeSeq &&
    local.authorityScopeAlive === true &&
    local.authorityReadinessJoined === true &&
    Number.isInteger(local.authorityReadinessArenaEpoch) &&
    local.authorityReadinessSettings === true &&
    local.authorityReadinessSecurity === true &&
    local.authorityReadinessMap === true &&
    local.authorityInitialBaselineReadyToSubmit === false &&
    local.lifecycleBarrierElapsed == null;
}
