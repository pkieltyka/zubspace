// Narrow explanations for retained motion findings. Missing/ambiguous draw or
// server evidence never turns a finding into a pass. Raw findings stay intact.
const tick = value => Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff;
const forward = (a, b) => (a - b) >>> 0;
function sameClock(a, b) {
  return Number.isInteger(a.combatViewTimeline?.generation) &&
    a.combatViewTimeline.generation > 0 &&
    a.combatViewTimeline.generation === b.combatViewTimeline?.generation;
}
function matchingDoor(events, botId, lifeSeq, predicate) {
  const matches = events.filter(event => event.playerId === botId &&
    event.lifeSeq === lifeSeq && tick(event.serverTick) &&
    Number.isFinite(event.destinationX) && Number.isFinite(event.destinationY) && predicate(event));
  return matches.length === 1 ? matches[0] : null;
}
// A directed source can report Distance (2) when its first accepted post-door
// sample omits Flash (3). Neither reason is sufficient on its own: require the
// unique native relocation straddled by these exact source ticks/destination.
export function continuousDoorReceipt(snap, botId, events) {
  const {previous:a, current:b} = snap;
  if (!(snap.dtMs > 0 && snap.dtMs <= 50) || snap.snapDelta !== 1 || ![2, 3].includes(snap.snapReason) ||
      a.initialCount !== b.initialCount || !Number.isInteger(a.initialCount) ||
      !sameClock(a,b) || !tick(a.positionTick) || !tick(b.positionTick)) return null;
  const span = forward(b.positionTick,a.positionTick);
  if (span === 0 || span > 10) return null;
  const event = matchingDoor(events,botId,snap.lifeSeq,e =>
    forward(e.serverTick,a.positionTick) > 0 && forward(e.serverTick,a.positionTick) <= span &&
    Math.hypot(b.positionX-e.destinationX,b.positionY-e.destinationY) <= 32);
  return event ? {kind:'continuous-door-receipt',snap,serverRelocation:event} : null;
}

// The long-horizon audit omits actors without a renderable pose. A native
// door relocation can remove a bot from AOI, then its reentry waits for a
// presentable pose. Require all three retained boundaries and the unique
// committed relocation before explaining a jump across that invisible gap.
export function reenteredDoorReceipt(snap, botId, events, frames) {
  const a = snap.previous, b = snap.current;
  if (snap.snapDelta !== 1 || snap.snapReason !== 3 || !sameClock(a, b) ||
      !Number.isInteger(a.initialCount) || b.initialCount !== a.initialCount + 1 ||
      !tick(a.positionTick) || !tick(b.positionTick) ||
      !Number.isSafeInteger(a.renderOrdinal) || !Number.isSafeInteger(b.renderOrdinal)) return null;
  const span = forward(b.positionTick, a.positionTick);
  if (span === 0 || span >= 0x8000_0000) return null;
  const event = matchingDoor(events, botId, snap.lifeSeq, e =>
    forward(e.serverTick, a.positionTick) > 0 && forward(e.serverTick, a.positionTick) <= span);
  if (!event) return null;
  const facts = frames.filter(f => f.renderOrdinal > a.renderOrdinal && f.renderOrdinal < b.renderOrdinal &&
    f.combatViewTimeline?.generation === a.combatViewTimeline.generation)
    .map(f => ({ frame: f, actor: f.remoteActorPresentation?.find(p => p.id === botId && p.lifeSeq === snap.lifeSeq) }))
    .filter(f => f.actor && !f.frame.remotePlayers?.some(p => p.id === botId))
    .sort((x, y) => x.frame.renderOrdinal - y.frame.renderOrdinal);
  const omitted = facts.find(f => f.actor.initialCount === a.initialCount &&
    f.actor.snapCount === a.snapCount && f.actor.positionPresent === false && f.actor.renderPositionPresent === false);
  const reentered = omitted && facts.find(f => f.frame.renderOrdinal > omitted.frame.renderOrdinal &&
    f.actor.initialCount === b.initialCount && f.actor.snapCount === a.snapCount &&
    f.actor.positionPresent === true && f.actor.renderPositionPresent === false);
  const relocated = reentered && facts.find(f => f.frame.renderOrdinal > reentered.frame.renderOrdinal &&
    f.actor.initialCount === b.initialCount && f.actor.snapCount === b.snapCount &&
    f.actor.lastSnapReason === 3 && f.actor.positionPresent === true && f.actor.renderPositionPresent === false);
  return relocated ? { kind: 'door-relocation-across-unrenderable-reentry', snap,
    serverRelocation: event, boundaries: [omitted, reentered, relocated] } : null;
}
export function continuousDoorDisplayJump(jump, botId, events) {
  const {previous:a, current:b} = jump;
  const ta=a.combatViewTimeline?.presentationRenderTick, tb=b.combatViewTimeline?.presentationRenderTick;
  if (!(jump.dtMs > 0 && jump.dtMs <= 50) || !sameClock(a,b) ||
      !tick(ta) || !tick(tb) || forward(tb,ta) === 0 || forward(tb,ta) > 10 ||
      a.lifeSeq !== b.lifeSeq || a.snapCount !== b.snapCount || !(b.snapCount > 0) ||
      a.initialCount !== b.initialCount || b.lastSnapReason !== 3) return null;
  const event=matchingDoor(events,botId,b.lifeSeq,e =>
    forward(e.serverTick,ta) > 0 && forward(e.serverTick,ta) <= forward(tb,ta) &&
    Math.hypot(b.displayX-e.destinationX,b.displayY-e.destinationY) <= 32 &&
    Math.hypot(b.positionX-e.destinationX,b.positionY-e.destinationY) <= 32 &&
    tick(b.positionTick) && forward(b.positionTick,e.serverTick) <= 10);
  return event ? {kind:'continuous-door-display',jump,serverRelocation:event} : null;
}
export function offscreenMotionFinding(finding, samples, botId, spriteFrames) {
  const selected=finding.previous && finding.current ? [finding.previous,finding.current] :
    samples.filter(s => s.at >= finding.startedAt && s.at <= finding.endedAt);
  if (selected.length < 2) return null;
  if (finding.frameCount != null && selected.length < finding.frameCount) return null;
  const witnesses=[];
  for (const sample of selected) {
    const wallAt=sample.wallAt ?? sample.at;
    if (!Number.isFinite(wallAt) || !tick(sample.positionTick) ||
        !Number.isInteger(sample.positionSnapshotId) || sample.positionSnapshotId <= 0 ||
        !Number.isInteger(sample.lifeSeq) || sample.lifeSeq <= 0) return null;
    const matches=spriteFrames.flatMap(frame => {
      // Sprite emission precedes HUD publication in the same native draw.
      // Current captures bind this order with ordinals; old captures must
      // match time AND exact source/pose, with no ambiguous candidate.
      if (Number.isInteger(frame.renderOrdinal) ? frame.renderOrdinal+1 !== sample.renderOrdinal :
          Math.abs(frame.at-wallAt) > 2) return [];
      return (frame.entries || []).filter(e => e.id === botId && e.lifeSeq === sample.lifeSeq &&
        e.drawn === true && e.suppression === 'none' &&
        e.lastSeenSnapshotId === sample.positionSnapshotId &&
        e.lastServerPositionTick === (sample.positionTick & 0xffff) &&
        Math.abs(e.renderedX-sample.displayX) <= 0.02 &&
        Math.abs(e.renderedY-sample.displayY) <= 0.02 &&
        Number.isFinite(e.screenX) && Number.isFinite(e.screenY))
        .map(e => ({at:frame.at,renderOrdinal:frame.renderOrdinal,
          screenWidth:frame.screenWidth,screenHeight:frame.screenHeight,entry:e}));
    });
    const unique=[...new Map(matches.map(m => [JSON.stringify(m),m])).values()];
    if (unique.length !== 1) return null;
    const witness=unique[0];
    // A conservative128px hull/label margin. Positive edges require actual
    // dimensions from this same native sprite frame, never a later viewport.
    const sized=Number.isFinite(witness.screenWidth) && witness.screenWidth>0 &&
      Number.isFinite(witness.screenHeight) && witness.screenHeight>0;
    if (!(witness.entry.screenX < -128 || witness.entry.screenY < -128 ||
        (sized && (witness.entry.screenX>witness.screenWidth+128 ||
          witness.entry.screenY>witness.screenHeight+128)))) return null;
    witnesses.push(witness);
  }
  return {kind:'draws-outside-proven-viewport',finding,witnesses};
}
