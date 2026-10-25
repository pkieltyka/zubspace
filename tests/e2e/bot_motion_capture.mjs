// Test-only observation of the already-published metrics. Retain both sides of
// each presentation/membership boundary before the ordinary 1200-frame ring
// evicts it. No polling, extra rendering, or gameplay changes are introduced.
export function installBrowserBotMotionCapture(botIds, maximumFrames = 4096) {
  if (window.__zubspaceBotMotionCapture) throw new Error('bot motion capture already installed');
  const trace = window.__zubspaceMetricsTrace;
  if (!Array.isArray(trace) || !Number.isInteger(maximumFrames) || maximumFrames < 2 ||
      !Array.isArray(botIds) || !botIds.length || !botIds.every(Number.isInteger)) {
    throw new Error('bot motion capture requires a live metrics ring and bot ids');
  }
  const ids = new Set(botIds);
  const capture = { trace, frames: [], sprites: [], overflow: 0, observed: 0 };
  window.__zubspaceBotMotionCapture = capture;
  let previous = trace.at(-1), previousKey;
  const key = frame => JSON.stringify((frame?.remoteActorPresentation || [])
    .filter(actor => ids.has(actor.id)).map(actor => [actor.id, actor.lifeSeq,
      actor.snapCount, actor.initialCount, actor.lastSnapReason,
      actor.positionPresent, actor.renderPositionPresent]).sort((a,b) => a[0]-b[0]));
  if (previous) previousKey = key(previous);
  const retained = new Set(), retainedSprites = new Set();
  const push = trace.push;
  Object.defineProperty(trace, 'push', { configurable: true, writable: true, value: function (...frames) {
    for (const frame of frames) {
      capture.observed++;
      const nextKey = key(frame);
      if (previous && previousKey !== nextKey) {
        for (const boundary of [previous, frame]) {
          if (retained.has(boundary.renderOrdinal)) continue;
          if (capture.frames.length >= maximumFrames) { capture.overflow++; continue; }
          retained.add(boundary.renderOrdinal); capture.frames.push(boundary);
        }
        for (const sprite of (window.__zubspaceRemoteSpriteTrace || []).slice(-2)) {
          if (retainedSprites.has(sprite.renderOrdinal)) continue;
          if (capture.sprites.length >= maximumFrames) { capture.overflow++; continue; }
          retainedSprites.add(sprite.renderOrdinal); capture.sprites.push(sprite);
        }
      }
      previous = frame; previousKey = nextKey;
    }
    return push.apply(this, frames);
  }});
}

export async function installBotMotionCapture(actor, botIds) {
  await actor.evaluate(`(${installBrowserBotMotionCapture.toString()})(${JSON.stringify(botIds)})`);
}
