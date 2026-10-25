const KEYS = { ArrowLeft: { bit: 4, sign: -1 }, ArrowRight: { bit: 8, sign: 1 }, ArrowUp: { bit: 1 } };

// Read once after the measured presses. Do not serialize the full QA trace in
// a hot polling loop or inject gameplay state to manufacture a response.
export const INPUT_LATENCY_CAPTURE = `({
  inputs: (window.__zubspaceInputTrace || []).filter(event => Number.isInteger(event.serial)).slice(-32),
  frames: (window.__zubspaceMetricsTrace || []).slice(-256).map(frame => ({
    renderOrdinal: frame.renderOrdinal, monotonicMs: frame.monotonicMs,
    playerLifeSeq: frame.playerLifeSeq, playerAlive: frame.playerAlive,
    serverPlayerAlive: frame.serverPlayerAlive, localInputBits: frame.localInputBits,
    playerOrientation: frame.playerOrientation,
    playerVelocityX: frame.playerVelocityX, playerVelocityY: frame.playerVelocityY,
    clientInputSeq: frame.clientInputSeq,
    localRenderPrediction: frame.localRenderPrediction,
    gameplayGate: frame.gameplayGate,
  }))
})`;

// A ship already pinned at its own terminal speed gains nothing from one more
// thrust integration, so a zero predicted-velocity delta is legitimate only when
// the draw is provably saturated: equal to the fastest prediction the run ever
// reached. Anything slower than that must still accelerate.
function thrustSaturated(draw, frames) {
  const speed = Math.hypot(draw.velocityX, draw.velocityY);
  if (speed === 0) return false;
  const topSpeed = frames.reduce((fastest, frame) => {
    const prediction = frame.localRenderPrediction;
    if (!prediction || !Number.isFinite(prediction.velocityX) || !Number.isFinite(prediction.velocityY)) {
      return fastest;
    }
    return Math.max(fastest, Math.hypot(prediction.velocityX, prediction.velocityY));
  }, 0);
  return speed === topSpeed;
}

export function assertNextDrawPrediction({ inputs, frames }, afterSerial, expectedKeys) {
  const presses = inputs.filter(event => event.serial > afterSerial && event.kind === "keydown");
  if (JSON.stringify(presses.map(event => event.code)) !== JSON.stringify(expectedKeys)) {
    throw new Error("input latency proof has missing or unexpected physical presses");
  }
  return presses.map(event => {
    const key = KEYS[event.code];
    const frame = frames.find(frame => frame.renderOrdinal === event.renderOrdinal + 1);
    const fail = reason => { throw new Error(`next-draw prediction ${reason}: ${JSON.stringify({ event, frame })}`); };
    if (!key || !Number.isInteger(event.renderOrdinal) || !Number.isFinite(event.monotonicMs) || !frame) fail("missing frame witness");
    if (frame.playerLifeSeq !== event.playerLifeSeq || frame.playerAlive !== true || frame.serverPlayerAlive !== true) fail("crossed readiness/life");
    if (!Number.isFinite(frame.monotonicMs) || frame.monotonicMs < event.monotonicMs) fail("invalid clock order");
    const draw = frame.localRenderPrediction;
    const priorDraw = event.localRenderPrediction;
    if (!draw || !priorDraw) fail("missing actual local draw state");
    if ((draw.inputBits & key.bit) === 0 || !Number.isInteger(draw.inputBits)) fail("did not consume the held key");
    const sequenceAdvance = (frame.clientInputSeq - event.clientInputSeq) & 0xffff;
    if (!Number.isInteger(frame.clientInputSeq) || !Number.isInteger(event.clientInputSeq) || sequenceAdvance >= 0x8000) fail("invalid canonical sequence");
    if (!Number.isFinite(draw.partialSeconds) || draw.partialSeconds < 0 || draw.partialSeconds > 1 / 60 + 1e-8) fail("unbounded render prediction");
    if (sequenceAdvance === 0 && draw.partialSeconds === 0) fail("did not advance local prediction");
    let saturatedByClamp = false;
    if (key.sign) {
      const delta = ((draw.orientation - priorDraw.orientation + 1.5) % 1) - 0.5;
      if (!Number.isFinite(delta) || delta * key.sign <= 0) fail("did not predict the requested rotation");
    } else {
      const delta = Math.hypot(draw.velocityX - priorDraw.velocityX, draw.velocityY - priorDraw.velocityY);
      if (!Number.isFinite(delta)) fail("did not predict thrust");
      if (delta === 0) {
        if (!thrustSaturated(draw, frames)) fail("did not predict thrust");
        saturatedByClamp = true;
      }
    }
    return { key: event.code, lifeSeq: event.playerLifeSeq, fromRenderOrdinal: event.renderOrdinal,
      predictedRenderOrdinal: frame.renderOrdinal, latencyMs: frame.monotonicMs - event.monotonicMs,
      sequenceAdvance, saturatedByClamp, event, frame };
  });
}

export function assertDrawCadence(frames, fromOrdinal, minimumHz) {
  const window = frames.filter(frame => frame.renderOrdinal >= fromOrdinal);
  if (window.length < 16 || window[0].renderOrdinal !== fromOrdinal) {
    throw new Error("high-refresh evidence lost its starting draw or has too few samples");
  }
  const intervals = [];
  for (let i = 1; i < window.length; i += 1) {
    const previous = window[i - 1], current = window[i];
    const dt = current.monotonicMs - previous.monotonicMs;
    if (current.renderOrdinal !== previous.renderOrdinal + 1 || !Number.isFinite(dt) || dt <= 0) {
      throw new Error(`high-refresh evidence has missing draws or invalid clock order: ${JSON.stringify({
        previous: { renderOrdinal: previous.renderOrdinal, monotonicMs: previous.monotonicMs },
        current: { renderOrdinal: current.renderOrdinal, monotonicMs: current.monotonicMs }, dt,
      })}`);
    }
    intervals.push(dt);
  }
  const durationMs = window.at(-1).monotonicMs - window[0].monotonicMs;
  const measuredHz = intervals.length * 1000 / durationMs;
  if (!Number.isFinite(minimumHz) || minimumHz <= 60 || measuredHz < minimumHz) {
    throw new Error(`high-refresh cadence not realized: ${JSON.stringify({ measuredHz, minimumHz, durationMs, draws: window.length, fromOrdinal, throughOrdinal: window.at(-1).renderOrdinal, intervals })}`);
  }
  const sorted = [...intervals].sort((a, b) => a - b);
  const percentile = rank => sorted[Math.ceil(sorted.length * rank) - 1];
  return { fromOrdinal, throughOrdinal: window.at(-1).renderOrdinal,
    draws: window.length, durationMs, measuredHz, minimumHz,
    p50Ms: percentile(0.5), p95Ms: percentile(0.95), maxMs: sorted.at(-1), intervals };
}

export function assertHeldDrawCadence(capture, press, captureFromOrdinal) {
  const release = capture.inputs.find(event => event.serial > press.serial &&
    event.kind === "keyup" && event.code === press.code);
  if (!release || release.monotonicMs <= press.monotonicMs || release.playerLifeSeq !== press.playerLifeSeq) {
    throw new Error("high-refresh proof lacks an exact same-life physical release");
  }
  const cadence = assertDrawCadence(
    capture.frames.filter(frame => frame.renderOrdinal <= release.renderOrdinal),
    press.renderOrdinal, 90);
  const heldDurationMs = release.monotonicMs - press.monotonicMs;
  const measuredHz = (release.renderOrdinal - press.renderOrdinal) * 1000 / heldDurationMs;
  if (!Number.isFinite(measuredHz) || measuredHz < 90) {
    throw new Error(`high-refresh physical hold cadence not realized: ${JSON.stringify({ press, release, cadence, measuredHz })}`);
  }
  return { ...cadence, drawMeasuredHz: cadence.measuredHz, measuredHz, heldDurationMs,
    pressSerial: press.serial, releaseSerial: release.serial,
    pressMs: press.monotonicMs, releaseMs: release.monotonicMs,
    // Controller RPC / setup gaps remain visible, but are not represented as
    // time during which the player was physically holding the measured key.
    captureWindow: capture.frames.filter(frame => frame.renderOrdinal >= captureFromOrdinal)
      .map(({ renderOrdinal, monotonicMs }) => ({ renderOrdinal, monotonicMs })) };
}

export async function proveNextDrawPrediction(actor, capture, { highRefresh = false } = {}) {
  const keys = ["ArrowLeft", "ArrowRight", "ArrowUp"];
  const evidence = [];
  // Repeated presses sample different render/simulation phases. Capture only
  // between rounds, preserving the exact witnesses before bounded rings evict
  // them. Every round must pass; no retry discards an inconvenient frame.
  for (let round = 0; round < 4; round += 1) {
    // Faster draws fill the fixed ring sooner. Read between presses in this
    // mode; never enlarge the production ring or poll during a held press.
    for (const batch of highRefresh ? keys.map(key => [key]) : [keys]) {
      const before = await capture();
      const serial = before.inputs.at(-1)?.serial ?? 0;
      for (const key of batch) await actor.press(key, 150);
      const after = await capture();
      const predictions = assertNextDrawPrediction(after, serial, batch);
      const cadence = highRefresh ? assertHeldDrawCadence(after, predictions[0].event, before.frames.at(-1)?.renderOrdinal) : null;
      evidence.push(...predictions
        .map(proof => ({ round, ...proof, ...(cadence ? { cadence } : {}) })));
    }
  }
  if (highRefresh && !evidence.some(proof => proof.sequenceAdvance === 0)) {
    throw new Error("high-refresh presses did not exercise a zero-canonical-step draw");
  }
  return evidence;
}
