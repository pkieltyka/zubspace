// The normal original-bitmap HUD has no diagnostic text rows. Diagnostics retain
// their existing four-to-nine-line contract only in explicit debug mode.
export function hudLineCountMatchesMode(metrics) {
  const debug = metrics?.localGameplayState?.debugMode;
  const count = metrics?.hudLineCount;
  if (typeof debug !== "boolean" || !Number.isInteger(count)) return false;
  return debug ? count >= 4 && count <= 9 : count === 0;
}

// Read only fields used by live readiness checks. WebDriver recursively
// serializes returned objects on the browser thread; copying packet/sprite/
// metrics histories here produced measured 0.98–1.29 second frame pauses.
const liveMetricFields = [
  "energyInside", "energyCenterDelta", "energyDigitsInside", "energyDigitsRightAligned",
  "hudTextInside", "hudRadarOverlap", "radarBottomRight", "radarLabelInside",
  "rightStatusPanelInside", "rightStatusPanelClearOfRadar", "leftItemPanelInside",
  "shipLabelInside", "notificationInside", "textPositiveScale", "hudTextScale",
  "shipLabelScale", "notificationScale", "hudLineCount", "remoteVisiblePlayers",
  "selfPlayerId", "playerAlive", "serverPlayerAlive", "playerEnergy",
];
const liveLocalFields = ["gameplayStepReady", "debugMode", "zoneMapReady", "zoneFilesReady", "authorityReadinessSecurity", "authorityPhase"];
export const smokeLiveStateExpression = `(() => {
  const metrics = window.__zubspaceUiMetrics;
  if (!metrics) return { networkLog: window.__zubspaceNetworkLog || [], uiMetrics: null };
  const uiMetrics = {};
  for (const key of ${JSON.stringify(liveMetricFields)}) uiMetrics[key] = metrics[key];
  uiMetrics.localGameplayState = {};
  for (const key of ${JSON.stringify(liveLocalFields)}) {
    uiMetrics.localGameplayState[key] = metrics.localGameplayState?.[key];
  }
  return { networkLog: window.__zubspaceNetworkLog || [], uiMetrics };
})()`;

export function smokeGameplayReady(metrics) {
  const local = metrics?.localGameplayState;
  return local?.gameplayStepReady === true && metrics?.playerAlive === true &&
    metrics?.serverPlayerAlive === true && local?.zoneMapReady === true &&
    local?.zoneFilesReady === true && local?.authorityReadinessSecurity === true &&
    local?.authorityPhase === "Live";
}
