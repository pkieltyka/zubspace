// Diagnostic correlation, not a replacement for gameplay/damage acceptance.
// Every match uses exact prediction and authenticated projectile identities.
const u32 = value => Number.isInteger(value) && value >= 0 && value <= 0xffffffff;
const u64 = value => typeof value === "string" && /^(0|[1-9][0-9]*)$/.test(value)
  && BigInt(value) <= 0xffffffffffffffffn;
const finitePose = row => [row?.x, row?.y, row?.velocityX, row?.velocityY,
  row?.animationSeconds, row?.bouncesRemaining].every(Number.isFinite);
const completeRows = rows => rows?.overflow === 0 && Number.isSafeInteger(rows.total)
  && Array.isArray(rows.entries) && rows.total === rows.entries.length;

export function ownerBombHandoffEvidence({ frames, admissions }) {
  const ordered = [...frames].sort((a, b) => a.monotonicMs - b.monotonicMs);
  const cases = [];
  const incomplete = [];
  for (const handoff of admissions.filter(row => row.phase === "owner-bomb-handoff")) {
    const identity = { activation: handoff.activation, authorityProjectileId: handoff.authorityProjectileId,
      inputSequence: handoff.inputSequence, sourceLifeSeq: handoff.sourceLifeSeq,
      serverSessionId: handoff.serverSessionId, arenaStreamId: handoff.arenaStreamId,
      sourcePlayerId: handoff.sourcePlayerId, sourceSessionId: handoff.sourceSessionId,
      shotGroupId: handoff.shotGroupId, sourceProjectileIndex: handoff.sourceProjectileIndex };
    const reject = reason => incomplete.push({ ...identity, reason });
    if (![handoff.serverSessionId, handoff.arenaStreamId, handoff.sourceSessionId,
      handoff.shotGroupId, handoff.activation].every(u64)
      || ![handoff.sourcePlayerId, handoff.sourceLifeSeq, handoff.sourceProjectileIndex,
        handoff.authorityProjectileId, handoff.inputSequence].every(u32)
      || handoff.sourcePlayerId > 0xffff || handoff.inputSequence > 0xffff || handoff.sourceProjectileIndex > 0xff
      || ![3, 4].includes(handoff.weaponData & 0x1f)
      || !Number.isFinite(handoff.monotonicMs)) {
      reject("missing-or-invalid-exact-identity"); continue;
    }
    if (handoff.terminal) { reject("terminal-retirement-has-no-live-handoff"); continue; }
    const sameFrameScope = frame => frame.serverSessionId === handoff.serverSessionId
      && frame.arenaStreamId === handoff.arenaStreamId
      && frame.viewerPlayerId === handoff.sourcePlayerId;
    const predicted = row => row.activation === handoff.activation
      && row.sourceLifeSeq === handoff.sourceLifeSeq
      && row.sourceProjectileIndex === handoff.sourceProjectileIndex
      && (row.inputSequence == null || row.inputSequence === handoff.inputSequence);
    const canonical = row => row.sourcePlayerId === handoff.sourcePlayerId
      && row.sourceSessionId === handoff.sourceSessionId
      && row.sourceLifeSeq === handoff.sourceLifeSeq
      && row.shotGroupId === handoff.shotGroupId
      && row.sourceProjectileIndex === handoff.sourceProjectileIndex
      && row.authorityProjectileId === handoff.authorityProjectileId;
    const before = ordered.filter(frame => sameFrameScope(frame) && frame.monotonicMs <= handoff.monotonicMs
      && frame.predictedBombs?.entries?.some(predicted)).at(-1);
    const after = ordered.find(frame => sameFrameScope(frame) && frame.monotonicMs >= handoff.monotonicMs
      && frame.projectiles?.entries?.some(canonical));
    if (!before || !after) { reject("missing-last-predicted-or-first-canonical-frame"); continue; }
    const window = ordered.filter(frame => frame.monotonicMs >= before.monotonicMs
      && frame.monotonicMs <= after.monotonicMs);
    if (window.length < 2 || window.some((frame, index) => !sameFrameScope(frame)
      || !completeRows(frame.predictedBombs) || !completeRows(frame.projectiles)
      || !Number.isSafeInteger(frame.spriteFrameOrdinal)
      || (index > 0 && (frame.spriteFrameOrdinal !== window[index - 1].spriteFrameOrdinal + 1
        || frame.monotonicMs <= window[index - 1].monotonicMs)))) {
      reject("incomplete-or-cross-scope-render-window"); continue;
    }
    const localRows = before.predictedBombs.entries.filter(predicted);
    const exactRows = after.projectiles.entries.filter(canonical);
    if (localRows.length !== 1 || exactRows.length !== 1) {
      reject("ambiguous-draw-identity"); continue;
    }
    const local = localRows[0], confirmed = exactRows[0];
    if (local.drawn !== true || confirmed.drawn !== true || !finitePose(local) || !finitePose(confirmed)) {
      reject("missing-visible-finite-draw-pose"); continue;
    }
    const dx = confirmed.x - local.x, dy = confirmed.y - local.y;
    const speed = Math.hypot(local.velocityX, local.velocityY);
    const elapsedSeconds = (after.monotonicMs - before.monotonicMs) / 1000;
    const comparable = !handoff.motionFrozen && !confirmed.motionFrozen
      && local.bouncesRemaining === confirmed.bouncesRemaining
      && Math.hypot(local.velocityX - confirmed.velocityX, local.velocityY - confirmed.velocityY) < 0.01;
    const alongVelocityPx = speed > 0 ? (dx * local.velocityX + dy * local.velocityY) / speed : null;
    cases.push({ ...identity, beforeOrdinal: before.spriteFrameOrdinal,
      afterOrdinal: after.spriteFrameOrdinal, elapsedSeconds,
      maximumFrameGapMs: Math.max(...window.slice(1).map((frame, index) => frame.monotonicMs - window[index].monotonicMs)),
      before: local, after: confirmed, dx, dy, alongVelocityPx,
      animationAgeDelta: confirmed.animationSeconds - local.animationSeconds,
      freeFlightComparable: comparable,
      backward: comparable && alongVelocityPx !== null && alongVelocityPx < -0.1,
      linearResidualPx: comparable ? Math.hypot(dx - local.velocityX * elapsedSeconds,
        dy - local.velocityY * elapsedSeconds) : null });
  }
  return { cases, incomplete, scope: "exact last-predicted/first-canonical bomb draw correlation; diagnostic only, not physics/damage or production-incident attribution" };
}

// Verify the entire bounded transition, rather than moving a backward jump to
// the next frame. A missing draw, scope cut or lost frame cannot prove recovery.
export function ownerBombConvergenceEvidence({ frames }, handoff) {
  const ordered = [...frames].sort((a, b) => a.monotonicMs - b.monotonicMs);
  const start = ordered.findIndex(frame => frame.spriteFrameOrdinal === handoff.afterOrdinal);
  const reject = reason => ({ ok: false, reason, activation: handoff.activation });
  if (start < 0) return reject("missing-first-canonical-frame");
  const finish = ordered.findIndex((frame, index) => index > start
    && frame.monotonicMs >= ordered[start].monotonicMs + 300);
  if (finish < 0) return reject("missing-complete-convergence-window");
  const window = ordered.slice(start, finish + 1), draws = [];
  for (const [index, frame] of window.entries()) {
    if (frame.serverSessionId !== handoff.serverSessionId || frame.arenaStreamId !== handoff.arenaStreamId
      || frame.viewerPlayerId !== handoff.sourcePlayerId || !completeRows(frame.projectiles)
      || !Number.isFinite(frame.monotonicMs) || !Number.isSafeInteger(frame.spriteFrameOrdinal)
      || (index > 0 && (frame.spriteFrameOrdinal !== window[index - 1].spriteFrameOrdinal + 1
        || frame.monotonicMs <= window[index - 1].monotonicMs))) return reject("incomplete-or-cross-scope-convergence-window");
    const rows = frame.projectiles.entries.filter(row => row.authorityProjectileId === handoff.authorityProjectileId
      && row.sourcePlayerId === handoff.sourcePlayerId && row.sourceSessionId === handoff.sourceSessionId
      && row.sourceLifeSeq === handoff.sourceLifeSeq && row.shotGroupId === handoff.shotGroupId
      && row.sourceProjectileIndex === handoff.sourceProjectileIndex);
    if (rows.length !== 1 || !finitePose(rows[0]) || !rows[0].drawn || rows[0].motionFrozen)
      return reject("missing-exact-visible-free-flight-draw");
    const row = rows[0], lead = row.ownerPredictionLeadSeconds;
    if (!Number.isFinite(lead) || lead < 0 || lead > 0.200001
      || !Number.isFinite(row.age) || !Number.isFinite(row.posePartialSeconds)
      || Math.abs(row.animationSeconds - row.age - row.posePartialSeconds - lead) > 0.001)
      return reject("missing-or-invalid-cosmetic-lead");
    if (index > 0) {
      const previous = draws[index - 1];
      if (Math.hypot(row.velocityX - previous.velocityX, row.velocityY - previous.velocityY) > 0.01
        || row.bouncesRemaining !== previous.bouncesRemaining) return reject("authoritative-motion-cut-during-convergence");
      const speed = Math.hypot(row.velocityX, row.velocityY);
      const forward = speed > 0 ? ((row.x - previous.x) * row.velocityX + (row.y - previous.y) * row.velocityY) / speed : 0;
      if (forward < -0.1 || row.animationSeconds + 0.00001 < previous.animationSeconds
        || lead > previous.ownerPredictionLeadSeconds + 0.0001) return reject("backward-or-restarted-convergence");
    }
    draws.push(row);
  }
  if (!(draws[0].ownerPredictionLeadSeconds > 0) || draws.at(-1).ownerPredictionLeadSeconds !== 0)
    return reject("lead-not-observed-to-fully-converge");
  return { ok: true, activation: handoff.activation, frames: window.length,
    firstOrdinal: window[0].spriteFrameOrdinal, lastOrdinal: window.at(-1).spriteFrameOrdinal,
    durationMs: window.at(-1).monotonicMs - window[0].monotonicMs,
    initialLeadSeconds: draws[0].ownerPredictionLeadSeconds, finalLeadSeconds: 0,
    maximumFrameGapMs: Math.max(...window.slice(1).map((frame, index) => frame.monotonicMs - window[index].monotonicMs)) };
}
