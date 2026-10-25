import assert from "node:assert/strict";
import test from "node:test";
import { hudLineCountMatchesMode, smokeLiveStateExpression, smokeGameplayReady } from "../tools/smoke_ui_contract.mjs";
import vm from "node:vm";

test("live smoke polls never read packet, render, or nested diagnostic histories", () => {
  const window = { __zubspaceNetworkLog: [{ message: "rtc answer accepted" }],
    __zubspaceUiMetrics: { selfPlayerId: 0, hudLineCount: 0,
      localGameplayState: { debugMode: false, authorityPhase: "Live" } } };
  const forbidden = () => assert.fail("live polling attempted an artifact-history read");
  for (const key of ["__zubspacePacketLog", "__zubspacePacketLogFirst", "__zubspaceMetricsTrace", "__zubspaceRemoteSpriteTrace"]) {
    Object.defineProperty(window, key, { get: forbidden });
  }
  Object.defineProperty(window.__zubspaceUiMetrics, "authoritativeSnapshotActorStream", { get: forbidden });
  Object.defineProperty(window.__zubspaceUiMetrics.localGameplayState, "brickTiles", { get: forbidden });
  const result = vm.runInNewContext(smokeLiveStateExpression, { window });
  assert.equal(result.uiMetrics.selfPlayerId, 0);
  assert.equal(result.uiMetrics.hudLineCount, 0);
  assert.equal(result.networkLog[0].message, "rtc answer accepted");
  assert.equal(hudLineCountMatchesMode(result.uiMetrics), true);
  assert.equal("authoritativeSnapshotActorStream" in result.uiMetrics, false);
});

test("live smoke state tolerates startup and requires actual playable readiness", () => {
  assert.equal(vm.runInNewContext(smokeLiveStateExpression, { window: {} }).uiMetrics, null);
  // The retained client exports numeric 1 at the legacy root and the
  // canonical scheduler boolean under localGameplayState.
  const metrics = { gameplayStepReady: 1, playerAlive: true, serverPlayerAlive: true,
    localGameplayState: { gameplayStepReady: true, zoneMapReady: true, zoneFilesReady: true, authorityReadinessSecurity: true, authorityPhase: "Live" } };
  assert.equal(smokeGameplayReady(metrics), true);
  assert.equal(smokeGameplayReady({ ...metrics, localGameplayState: { ...metrics.localGameplayState, gameplayStepReady: false } }), false);
  for (const field of ["zoneMapReady", "zoneFilesReady", "authorityReadinessSecurity"]) {
    assert.equal(smokeGameplayReady({ ...metrics, localGameplayState: { ...metrics.localGameplayState, [field]: false } }), false);
  }
  assert.equal(smokeGameplayReady({ ...metrics, localGameplayState: { ...metrics.localGameplayState, authorityPhase: "AwaitingReadiness" } }), false);
});

const hud = (debugMode, hudLineCount) => ({ localGameplayState: { debugMode }, hudLineCount });

test("normal play requires zero diagnostic text rows above the original bitmap HUD", () => {
  assert.equal(hudLineCountMatchesMode(hud(false, 0)), true);
  for (const count of [1, 2, 4, 9]) assert.equal(hudLineCountMatchesMode(hud(false, count)), false);
});

test("explicit debug mode retains its full diagnostic-line bounds", () => {
  for (const count of [4, 5, 6, 7, 8, 9]) assert.equal(hudLineCountMatchesMode(hud(true, count)), true);
  for (const count of [0, 1, 3, 10]) assert.equal(hudLineCountMatchesMode(hud(true, count)), false);
});

test("missing, mistyped and fractional mode/count evidence fails closed", () => {
  for (const metrics of [null, {}, { hudLineCount: 1 }, { localGameplayState: { debugMode: false } }])
    assert.equal(hudLineCountMatchesMode(metrics), false);
  for (const debug of [undefined, null, 0, 1, "false", "true"])
    assert.equal(hudLineCountMatchesMode(hud(debug, 1)), false);
  for (const count of [undefined, null, "1", NaN, Infinity, 1.5])
    assert.equal(hudLineCountMatchesMode(hud(false, count)), false);
});
