import assert from 'node:assert/strict';

// Read-only, compact frame capture. Avoid cloning packet/actor histories while
// the measured keyboard layer is active. A truncated trace cannot pass below.
export async function captureMenuFlight(client, sinceRenderOrdinal = null) {
  return client.evaluate(`(() => {
    const metrics = window.__zubspaceUiMetrics || {};
    const local = metrics.localGameplayState || {};
    const pick = r => r && Object.fromEntries([
      'renderOrdinal','clientInputSeq','selfPlayerId','playerLifeSeq','localInputBits',
      'playerX','playerY','playerVelocityX','playerVelocityY','playerOrientation'
    ].map(k => [k,r[k]]));
    const trace = window.__zubspaceMetricsTrace || [];
    return { scope: { playerId: metrics.selfPlayerId, lifeSeq: metrics.playerLifeSeq,
      session: local.authorityScopeServerSessionId, arena: local.authorityScopeArenaStreamId,
      authorityPlayerId: local.authorityScopePlayerId, authorityLifeSeq: local.authorityScopeLifeSeq,
      alive: local.authorityScopeAlive, phase: local.authorityPhase },
      layer: { list: metrics.scoreboardVisible, menu: local.menuVisible },
      frame: pick(trace.at(-1)),
      frames: ${sinceRenderOrdinal === null ? '[]' : `trace.filter(r => r.renderOrdinal > ${JSON.stringify(sinceRenderOrdinal)}).map(pick)`} };
  })()`);
}

export function assertMenuFlightIsolation(before, after, layer) {
  const a = before?.frame, b = after?.frame;
  assert.ok(a && b, 'menu flight proof needs actual render frames');
  assert.ok(['list', 'menu'].includes(layer), 'unknown keyboard layer');
  assert.equal(before.layer?.[layer], true, 'keyboard layer was not active at the start');
  assert.equal(after.layer?.[layer], true, 'keyboard layer did not remain active');
  assert.ok(before.scope?.session != null && before.scope?.arena != null, 'menu flight proof needs its live scope');
  assert.equal(before.scope.phase, 'Live');
  assert.equal(before.scope.alive, true);
  assert.equal(before.scope.authorityPlayerId, before.scope.playerId);
  assert.equal(before.scope.authorityLifeSeq, before.scope.lifeSeq);
  assert.deepEqual(after.scope, before.scope, 'menu flight proof crossed its authority scope');
  assert.equal(a.selfPlayerId, before.scope.playerId);
  assert.equal(a.playerLifeSeq, before.scope.lifeSeq);
  assert.equal(a.localInputBits, 0, 'menu proof began with an admitted flight control');
  assert.ok(Number.isInteger(a.clientInputSeq) && a.clientInputSeq >= 0 && a.clientInputSeq <= 0xffff);
  assert.ok(Number.isInteger(a.renderOrdinal) && Number.isInteger(b.renderOrdinal) && b.renderOrdinal > a.renderOrdinal);
  const frames = after.frames || [];
  assert.equal(frames.length, b.renderOrdinal - a.renderOrdinal, 'incomplete menu render history');
  let maximumCoastErrorPx = 0;
  for (const [index, frame] of frames.entries()) {
    assert.equal(frame.renderOrdinal, a.renderOrdinal + index + 1, 'missing or duplicated menu render frame');
    assert.equal(frame.selfPlayerId, before.scope.playerId, 'menu frame belongs to another player');
    assert.equal(frame.playerLifeSeq, before.scope.lifeSeq, 'menu frame belongs to another life');
    assert.equal(frame.localInputBits, 0, 'menu leaked a sampled flight control');
    assert.equal(frame.playerOrientation, a.playerOrientation, 'menu leaked rotation');
    assert.equal(frame.playerVelocityX, a.playerVelocityX, 'menu changed horizontal inertia');
    assert.equal(frame.playerVelocityY, a.playerVelocityY, 'menu changed vertical inertia');
    const steps = (frame.clientInputSeq - a.clientInputSeq) & 0xffff;
    assert.ok(Number.isInteger(frame.clientInputSeq) && frame.clientInputSeq >= 0 &&
      frame.clientInputSeq <= 0xffff && steps < 0x8000, 'invalid menu input chronology');
    // Positions are canonical60Hz state, not wall-clock extrapolation through
    // a test-driver pause. Keep the original1px positional allowance around
    // the inherited inertial trajectory. Rounded HUD velocities and f32
    // integration can accumulate a small subpixel difference.
    const error = Math.hypot(frame.playerX - a.playerX - a.playerVelocityX * steps / 60,
      frame.playerY - a.playerY - a.playerVelocityY * steps / 60);
    assert.ok(Number.isFinite(error) && error <= 1, `menu flight departed its inertial trajectory: ${error}px`);
    maximumCoastErrorPx = Math.max(maximumCoastErrorPx, error);
  }
  assert.deepEqual(frames.at(-1), b, 'menu proof omitted its final frame');
  return { layer, renderFrames: frames.length,
    inputSteps: (b.clientInputSeq - a.clientInputSeq) & 0xffff,
    inheritedVelocity: { x: a.playerVelocityX, y: a.playerVelocityY }, maximumCoastErrorPx };
}
