import assert from "node:assert/strict";
import test from "node:test";
import { lifecycleIsReady } from "./lifecycle_readiness.mjs";

function ready() {
  return { uiMetrics: { playerAlive: true, serverPlayerAlive: true,
    selfPlayerId: 7, playerLifeSeq: 1, serverPlayerEnergy: 1700,
    localGameplayState: { zoneFilesReady: true, zoneMapReady: true,
      pendingShipResetPosition: false, lifecycleBaselineReceiptsMatched: 1,
      lifecycleBaselineReceiptsUnmatched: 0, authorityPhase: "Live",
      authoritySegment: 1, authorityScopeServerSessionId: "123",
      authorityScopeArenaStreamId: "4", authorityScopePlayerId: 7,
      authorityScopeLifeSeq: 1, authorityScopeAlive: true,
      authorityReadinessJoined: true, authorityReadinessArenaEpoch: 1,
      authorityReadinessSettings: true, authorityReadinessSecurity: true,
      authorityReadinessMap: true, authorityInitialBaselineReadyToSubmit: false,
    } } };
}
test("Live/alive alone does not start the moving-load window", () => {
  assert.equal(lifecycleIsReady({uiMetrics: {serverPlayerAlive: true,
    localGameplayState: {authorityPhase: "Live"}}}), false);
});
test("shared startup predicate rejects unfinished publication and stale authority", () => {
  assert.equal(lifecycleIsReady(ready()), true);
  for (const patch of [{zoneFilesReady: false}, {zoneMapReady: false},
    {authorityReadinessSecurity: false}, {authorityScopeLifeSeq: 2},
    {lifecycleBaselineReceiptsMatched: 0}, {pendingShipResetPosition: true},
    {authorityTransitionKind: "Ship"}]) {
    const state = ready(); Object.assign(state.uiMetrics.localGameplayState, patch);
    assert.equal(lifecycleIsReady(state), false);
  }
});
