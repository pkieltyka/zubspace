// This projection is only for the single-bullet scenario's live assertions.
// Cold artifacts retain the original complete browser state. Do not trim rows:
// damage indices and all post-hit/recharge samples must remain identical.
export function projectBulletFeedback(scope) {
  const fields = [
    'at', 'selfPlayerId', 'playerLifeSeq', 'playerAlive', 'serverPlayerAlive',
    'playerMaxEnergy', 'playerEnergy', 'serverPlayerEnergy', 'remoteEnergy',
    'remotePlayers', 'localShipLabelEnergy', 'visibleEnergyDigits',
    'lastSelfDamageSeq', 'lastNetworkDamageSeq',
    'lastSelfDamageServerSeq', 'lastSelfDamageServerTick',
    'lastSelfDamageVictimLifeSeq', 'lastSelfDamageEnergy',
    'lastNetworkDamageServerSeq', 'lastNetworkDamageServerTick',
    'lastNetworkDamagePlayerId', 'lastNetworkDamageVictimLifeSeq',
    'lastNetworkDamageEnergy',
  ];
  return {
    uiMetrics: scope.__zubspaceUiMetrics || null,
    damageTrace: scope.__zubspaceDamageTrace || [],
    metricsTrace: (scope.__zubspaceMetricsTrace || []).map(row => {
      const result = {};
      for (const key of fields) if (Object.hasOwn(row, key)) result[key] = row[key];
      return result;
    }),
  };
}

export function readBulletFeedback(client) {
  return client.evaluate(`(${projectBulletFeedback.toString()})(window)`);
}
