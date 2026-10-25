const DEFAULT_SAFE_EXIT_ATTEMPTS = 12;
const DEFAULT_SAFE_EXIT_THRUST_MS = 250;

// Passive wire evidence only. Keep this self-contained so the smoke driver can
// run it in-page without shipping the bounded packet history over DevTools.
function lifecycleBaselineWireEvidence(entries, lifeSeq) {
  let lastPacketSequence = -1, baseline = null;
  for (const entry of entries) {
    if (!Number.isSafeInteger(entry?.seq) || entry.seq < 0) continue;
    lastPacketSequence = Math.max(lastPacketSequence, entry.seq);
    const b = entry.bytes;
    if (entry.direction !== "received" || entry.detail?.channel !== "zubspace-reliable" ||
        entry.byteLength !== 26 || entry.bytesTruncated !== false ||
        !Array.isArray(b) || b.length !== 26 ||
        !b.every(v => Number.isInteger(v) && v >= 0 && v <= 255) ||
        b[0] !== 0 || b[1] !== 3 || b[6] !== 0x75 || b[7] !== 3) continue;
    const view = new DataView(Uint8Array.from(b).buffer);
    const receipt = { packetSequence: entry.seq, deliveryId: view.getUint32(8, true),
      lifeSeq: view.getUint32(12, true), positionSequence: view.getUint16(16, true),
      acceptedServerTick: view.getUint32(18, true), x: view.getUint16(22, true), y: view.getUint16(24, true), wireBytes: [...b] };
    if (receipt.lifeSeq !== lifeSeq || receipt.x >= 16384 || receipt.y >= 16384) continue;
    if (!baseline || receipt.packetSequence > baseline.packetSequence) baseline = receipt;
  }
  return { lastPacketSequence, baseline };
}

function shipResetBefore(metrics) {
  return {
    playerLifeSeq: metrics?.playerLifeSeq ?? 0,
    selfPlayerId: metrics?.selfPlayerId,
    serverSessionId: metrics?.localGameplayState?.authorityScopeServerSessionId,
    arenaStreamId: metrics?.localGameplayState?.authorityScopeArenaStreamId,
    lastPacketSequence: metrics?.lifecycleWireEvidence?.lastPacketSequence,
    lifecycleBaselineReceiptsMatched: metrics?.localGameplayState?.lifecycleBaselineReceiptsMatched ?? 0,
    lifecycleBaselineReceiptsUnmatched: metrics?.localGameplayState?.lifecycleBaselineReceiptsUnmatched ?? 0,
  };
}

function initialSpawnLifecycleEvidence(metrics) {
  const local = metrics?.localGameplayState, receipt = metrics?.lifecycleWireEvidence?.baseline;
  if (!local || !receipt || !Number.isInteger(metrics.selfPlayerId) ||
      !Number.isInteger(metrics.playerLifeSeq) || metrics.playerLifeSeq <= 0 ||
      local.authorityPhase !== "Live" || local.authorityScopeAlive !== true ||
      local.authorityScopePlayerId !== metrics.selfPlayerId ||
      local.authorityScopeLifeSeq !== metrics.playerLifeSeq ||
      local.authorityScopeServerSessionId == null || local.authorityScopeArenaStreamId == null ||
      local.authorityTransitionKind != null || local.authorityApplicationBarrierKind != null ||
      local.authorityApplicationBarrierCount !== 0 ||
      !(local.lifecycleBaselineReceiptsMatched > 0) ||
      local.lifecycleBaselineReceiptsSeen !== local.lifecycleBaselineReceiptsMatched ||
      local.lifecycleBaselineReceiptsUnmatched !== 0 ||
      receipt.lifeSeq !== metrics.playerLifeSeq ||
      !Number.isSafeInteger(receipt.packetSequence) || receipt.packetSequence < 0 ||
      !Array.isArray(receipt.wireBytes) || receipt.wireBytes.length !== 26 ||
      ![receipt.x, receipt.y].every(v => Number.isInteger(v) && v >= 0 && v < 16_384) ||
      ![metrics.playerX, metrics.playerY, metrics.playerVelocityX, metrics.playerVelocityY].every(Number.isFinite) ||
      Math.hypot(metrics.playerX - receipt.x, metrics.playerY - receipt.y) > 4 ||
      Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) > 64) return null;
  return { playerId: metrics.selfPlayerId, lifeSeq: metrics.playerLifeSeq,
    serverSessionId: local.authorityScopeServerSessionId, arenaStreamId: local.authorityScopeArenaStreamId,
    baseline: receipt, x: metrics.playerX, y: metrics.playerY };
}

function assertInitialSpawnPositionStream(label, summary, evidence) {
  const samples = summary.sentPositions.slice(0, 5);
  if (!evidence?.baseline || samples.length === 0 ||
      samples.some(p => !Number.isFinite(p.x) || !Number.isFinite(p.y) ||
        Math.abs(p.x - evidence.baseline.x) > 192 || Math.abs(p.y - evidence.baseline.y) > 192)) {
    throw new Error(`${label} client positions did not match the accepted initial spawn: ${JSON.stringify({evidence, samples})}`);
  }
}

function orderedZoneFilesSettledForLifecycle(metrics) {
  return (
    metrics?.localGameplayState?.zoneMapReady === true &&
    metrics.localGameplayState.zoneFilesReady === true
  );
}

function shipResetLifecycleSettled(before, metrics) {
  if (!before || !metrics) return false;
  const lifecycle = metrics.localGameplayState;
  if (!lifecycle) return false;
  const lifeAdvanced = metrics.playerLifeSeq > before.playerLifeSeq;
  const acceptedNewBaseline =
    lifecycle.lifecycleBaselineReceiptsMatched > before.lifecycleBaselineReceiptsMatched &&
    lifecycle.lifecycleBaselineReceiptsSeen === lifecycle.lifecycleBaselineReceiptsMatched &&
    lifecycle.lifecycleBaselineReceiptsUnmatched === before.lifecycleBaselineReceiptsUnmatched;
  const barrierDrained =
    lifecycle.authorityPhase === "Live" &&
    lifecycle.authorityTransitionKind == null &&
    lifecycle.authorityApplicationBarrierKind == null &&
    lifecycle.authorityApplicationBarrierCount === 0 &&
    lifecycle.pendingShipResetPosition === false &&
    lifecycle.pendingShipResetDeliveryId === null &&
    lifecycle.pendingShipResetLifeSeq === null &&
    lifecycle.pendingShipResetSequence === null &&
    lifecycle.pendingFrequencyBaselineDeliveryId === null;
  const receipt = metrics.lifecycleWireEvidence?.baseline;
  const exactScope = before.serverSessionId != null && before.arenaStreamId != null &&
    lifecycle.authorityScopeServerSessionId === before.serverSessionId &&
    lifecycle.authorityScopeArenaStreamId === before.arenaStreamId &&
    metrics.selfPlayerId === before.selfPlayerId &&
    lifecycle.authorityScopePlayerId === metrics.selfPlayerId &&
    lifecycle.authorityScopeLifeSeq === metrics.playerLifeSeq && lifecycle.authorityScopeAlive === true;
  const exactReceipt = Number.isSafeInteger(before.lastPacketSequence) && before.lastPacketSequence >= 0 &&
    Number.isSafeInteger(receipt?.packetSequence) && receipt.packetSequence > before.lastPacketSequence &&
    receipt.lifeSeq === metrics.playerLifeSeq;
  const canonicalSpawn = exactReceipt &&
    Math.hypot(metrics.playerX - receipt.x, metrics.playerY - receipt.y) <= 4 &&
    Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) <= 64;
  return exactScope && lifeAdvanced && acceptedNewBaseline && barrierDrained && canonicalSpawn;
}

function isExactPlayerEnteringPayload(payload) {
  if (payload == null || payload[0] !== 0x03) return false;
  if (payload.length === 64) return true;
  // Nonzero assists use the established roster tail: one versioned score
  // update for the same actor, with the same classic score/count prefix.
  if (payload.length !== 84 || payload[64] !== 0x09 || payload[79] !== 1) return false;
  for (const [rosterOffset, scoreOffset, size] of [
    [51, 65, 2], [43, 67, 4], [47, 71, 4], [55, 75, 2], [57, 77, 2],
  ]) {
    for (let index = 0; index < size; index += 1) {
      if (payload[rosterOffset + index] !== payload[scoreOffset + index]) return false;
    }
  }
  return true;
}

function selfJoinBootstrapSettled(summary, expectedName) {
  return (
    summary?.selfId != null &&
    (summary.receivedJoinGameCount || 0) >= 1 &&
    summary.rosterNames?.includes(expectedName) === true
  );
}

function mutualRosterConverged(alphaSummary, betaSummary) {
  return (
    alphaSummary?.rosterNames?.includes("alpha") === true &&
    alphaSummary.rosterNames.includes("beta") &&
    betaSummary?.rosterNames?.includes("alpha") === true &&
    betaSummary.rosterNames.includes("beta")
  );
}

const IMMUTABLE_PROJECTILE_SOURCE_FIELDS = Object.freeze([
  "playerId",
  "authenticated",
  "sessionId",
  "lifeSeq",
  "serverSessionId",
  "arenaStreamId",
  "kind",
  "fireX",
  "fireY",
  "fireVelocityX",
  "fireVelocityY",
  "fireOrientation",
  "frequency",
  "ship",
  "bombGravity",
  "bombBounces",
]);

function immutableProjectileSourceMatches(expected, actual) {
  return (
    !!expected &&
    !!actual &&
    IMMUTABLE_PROJECTILE_SOURCE_FIELDS.every(
      (field) => actual[field] === expected[field],
    )
  );
}

function retainedProjectileAfterDamageDeath(
  expected,
  replacementLifeSeq,
  activeSources,
) {
  if (
    !expected?.authenticated ||
    !expected.sessionId ||
    !expected.lifeSeq ||
    !replacementLifeSeq ||
    replacementLifeSeq === expected.lifeSeq ||
    !Array.isArray(activeSources)
  ) {
    return null;
  }
  return (
    activeSources.find((source) =>
      immutableProjectileSourceMatches(expected, source),
    ) || null
  );
}

function cancelledProjectileAfterRosterReplacement(expected, replacementLifeSeq, activeSources, cancellations) {
  const delta = (replacementLifeSeq - expected?.lifeSeq) >>> 0;
  if (!expected?.authenticated || !expected.sessionId || expected.sessionId === "0" || !expected.lifeSeq || !replacementLifeSeq || delta === 0 || delta >= 0x80000000 || !Array.isArray(activeSources) || !Array.isArray(cancellations)) return null;
  try {
    const group = BigInt(expected.shotGroupId);
    if (group <= 0n) return null;
    return cancellations.find(cut => {
      const cutDelta = (cut.ownerLifeSeq - expected.lifeSeq) >>> 0;
      if (cut.source !== expected.playerId || cut.sourceSessionId !== expected.sessionId || cut.serverSessionId !== expected.serverSessionId || cut.arenaStreamId !== expected.arenaStreamId || cutDelta === 0 || cutDelta > delta || BigInt(cut.throughShotGroupId) < group) return false;
      return !activeSources.some(source => source.playerId === expected.playerId && source.sessionId === expected.sessionId && (source.shotGroupId == null || BigInt(source.shotGroupId) <= BigInt(cut.throughShotGroupId)));
    }) || null;
  } catch { return null; }
}

async function moveOutOfSafeZoneForWeaponProof(
  client,
  {
    maxAttempts = DEFAULT_SAFE_EXIT_ATTEMPTS,
    thrustMs = DEFAULT_SAFE_EXIT_THRUST_MS,
  } = {},
) {
  for (let attempt = 0; attempt <= maxAttempts; attempt += 1) {
    const metrics = await client.uiMetrics();
    if (!metrics?.inSafeZone) return metrics;
    if (attempt === maxAttempts) break;
    // Keep this probe lightweight. A full screenshot/page-state capture while
    // an Extreme ship coasts can consume the entire 64-input witness window
    // and carry the client into geometry before the next keyboard action.
    await client.cruise(thrustMs);
  }
  throw new Error("keyboard client could not leave safe tiles within the bounded thrust schedule");
}

export {
  DEFAULT_SAFE_EXIT_ATTEMPTS,
  cancelledProjectileAfterRosterReplacement,
  DEFAULT_SAFE_EXIT_THRUST_MS,
  isExactPlayerEnteringPayload,
  immutableProjectileSourceMatches,
  moveOutOfSafeZoneForWeaponProof,
  mutualRosterConverged,
  orderedZoneFilesSettledForLifecycle,
  retainedProjectileAfterDamageDeath,
  selfJoinBootstrapSettled,
  shipResetLifecycleSettled,
  lifecycleBaselineWireEvidence,
  shipResetBefore,
  initialSpawnLifecycleEvidence,
  assertInitialSpawnPositionStream,
};
