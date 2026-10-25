import { observedActorSourceTick, remoteActorMotionSmoothnessEvidence } from "./bot_combat_evidence.mjs";
import { captureBrowserJsonReader } from "./browser_json_snapshot.mjs";
import { createJsonChunkReader } from "./cdp_json_capture.mjs";
import { serverLogField } from "../../tools/server_log_fields.mjs";

// A four-minute passive observation is longer than the legitimate two-minute
// fresh-spawn grace. Participate once, through real browser keys, and prove
// that ordinary direction change was admitted for this exact observer life.
export async function prepareRemoteMotionObserver(actor, server, turnRight, timeoutMs = 4_000) {
  if (typeof turnRight !== "function" || !Array.isArray(server?.logs) ||
      !Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 4_000)
    throw new Error("motion observation requires a bounded genuine-key observer preparation");
  const before = (await actor.metricsState()).uiMetrics;
  const scope = m => JSON.stringify([m?.localGameplayState?.authorityScopeServerSessionId,
    m?.localGameplayState?.authorityScopeArenaStreamId, m?.selfPlayerId, m?.playerLifeSeq]);
  const live = m => m?.playerAlive === true && m?.serverPlayerAlive === true &&
    Number.isInteger(m.playerLifeSeq) && m.localGameplayState?.authorityPhase === "Live";
  if (!live(before)) throw new Error("motion observer must be live before participation");
  const offset = server.logs.join("").length;
  const deadline = Date.now() + timeoutMs;
  await turnRight();
  const field = (line, name) => serverLogField(line, name) ??
    line.match(new RegExp(`(?:^|\\s)${name}=([^\\s]+)`))?.[1];
  while (Date.now() < deadline) {
    const after = (await actor.metricsState()).uiMetrics;
    if (!live(after) || scope(after) !== scope(before)) throw new Error("motion observer participation crossed a lifecycle boundary");
    const witness = server.logs.join("").slice(offset).split("\n").find(line => {
      if (!line.includes("human authority input trace")) return false;
      const sequenceDelta = (Number(field(line, "input_sequence")) - before.clientInputSeq) & 0xffff;
      return Number(field(line, "player_id")) === before.selfPlayerId &&
        Number(field(line, "life_seq")) === before.playerLifeSeq &&
        String(field(line, "reliable_lifecycle_baseline")) === "false" &&
        Number.isInteger(Number(field(line, "input_direction"))) &&
        Number(field(line, "input_direction")) !== before.playerProtocolDirection &&
        sequenceDelta > 0 && sequenceDelta < 0x8000;
    });
    if (witness && after.playerProtocolDirection !== before.playerProtocolDirection)
      return { keysOnly: true, scope: scope(before), lifeSeq: before.playerLifeSeq,
        beforeDirection: before.playerProtocolDirection, afterDirection: after.playerProtocolDirection,
        acceptedInput: witness };
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  throw new Error("motion observer turn lacks exact ordinary admitted-input evidence");
}

// Runs inside the browser. Observe the existing post-render diagnostics without
// changing input, transport, simulation, or the product's bounded trace ring.
// No WebDriver read occurs until this capture has stopped.
export function installRemoteMotionCapture(botIds, durationMs, capacity = 40_000) {
  const trace = window.__zubspaceMetricsTrace;
  if (!Array.isArray(trace)) throw new Error("motion capture requires trace=1");
  if (window.__zubspaceMotionSoak) throw new Error("motion capture already installed");
  const originalPush = trace.push;
  const originalPushDescriptor = Object.getOwnPropertyDescriptor(trace, "push");
  const capture = {
    startedAt: performance.now(), durationMs, capacity, overflow: 0,
    totalFrames: 0, observerCostMs: 0, maximumObserverCostMs: 0, frames: [],
    endedAt: null,
  };
  window.__zubspaceMotionSoak = capture;
  const seal = (value) => {
    if (value && typeof value === "object" && !Object.isFrozen(value)) {
      Object.freeze(value);
      for (const child of Object.values(value)) seal(child);
    }
    return value;
  };
  // Keep the observer out of the array's serializable data properties, including
  // when a failure artifact freezes the trace before the observation ends.
  Object.defineProperty(trace, "push", { configurable: true, writable: true,
    enumerable: false, value: function (...entries) {
    const result = originalPush.apply(this, entries);
    for (const frame of entries) {
      const start = performance.now();
      capture.totalFrames += 1;
      if (capture.frames.length >= capacity) {
        capture.overflow += 1;
        continue;
      }
      const sprites = window.__zubspaceRemoteSpriteTrace?.at(-1);
      // Detach the small current record while collecting, not the entire
      // multi-minute history at export. The measured observer cost includes it.
      capture.frames.push(seal(structuredClone({
        at: start,
        wallAt: frame.at,
        observerLifeSeq: frame.playerLifeSeq,
        observerAlive: frame.playerAlive,
        combatViewTimeline: frame.combatViewTimeline,
        bots: botIds.map((id) => {
          const player = frame.remotePlayers?.find((entry) => entry.id === id);
          const presentation = frame.remoteActorPresentation?.find((entry) => entry.id === id);
          const sprite = sprites?.entries?.find((entry) => entry.id === id);
          const authoritative = frame.authoritativeSnapshotActors?.find((entry) => entry.id === id);
          const lifeAdvance = (presentation?.lifeSeq - authoritative?.lifeSeq) >>> 0;
          const deathOvertookSnapshot = lifeAdvance > 0 && lifeAdvance < 0x8000_0000;
          const legitimatelySuppressed = ["cloak", "attached", "spectator"]
            .includes(sprite?.suppression);
          return {
            id, lifeSeq: player?.lifeSeq,
            authoritativeLifeSeq: authoritative?.lifeSeq,
            expectedDrawn: authoritative != null && !deathOvertookSnapshot && !legitimatelySuppressed,
            snapCount: presentation?.snapCount, initialCount: presentation?.initialCount,
            lastSnapReason: presentation?.lastSnapReason,
            positionX: player?.positionX, positionY: player?.positionY,
            positionTick: player?.lastServerPositionTick,
            positionSnapshotId: presentation?.lastSeenSnapshotId,
            displayX: player?.displayX, displayY: player?.displayY,
            velocityX: player?.velocityX, velocityY: player?.velocityY,
            drawn: sprite?.drawn === true && sprite?.lifeSeq === player?.lifeSeq,
          };
        }),
      })));
      const cost = performance.now() - start;
      capture.observerCostMs += cost;
      capture.maximumObserverCostMs = Math.max(capture.maximumObserverCostMs, cost);
    }
    return result;
  }});
  setTimeout(() => {
    if (originalPushDescriptor) Object.defineProperty(trace, "push", originalPushDescriptor);
    else delete trace.push;
    capture.endedAt = performance.now();
    Object.freeze(capture.frames);
    Object.freeze(capture);
  }, durationMs);
  return { startedAt: capture.startedAt, durationMs, capacity };
}

export function completedMotionReader(createReader, chunkCharacters) {
  const capture = window.__zubspaceMotionSoak;
  if (!capture || !Number.isFinite(capture.endedAt) ||
      !Object.isFrozen(capture) || !Object.isFrozen(capture.frames)) {
    throw new Error("motion snapshot is not completed and sealed");
  }
  return createReader(capture, chunkCharacters);
}

export function remoteMotionSoakEvidence(capture, botIds, expectedObserverLifeSeq = null) {
  const frames = capture?.frames || [];
  const spanMs = frames.length >= 2 ? frames.at(-1).at - frames[0].at : 0;
  const frameGaps = [];
  let maximumFrameGapMs = 0;
  for (let index = 1; index < frames.length; index += 1) {
    const gap = frames[index].at - frames[index - 1].at;
    maximumFrameGapMs = Math.max(maximumFrameGapMs, gap);
    if (gap >= 80) frameGaps.push({
      durationMs: gap, previous: frames[index - 1], current: frames[index],
    });
  }
  const windowValid = Number.isFinite(capture?.startedAt) && Number.isFinite(capture?.endedAt) &&
    frames.length > 0 && frames[0].at >= capture.startedAt && frames.at(-1).at <= capture.endedAt;
  if (windowValid) {
    for (const [boundary, gap, adjacent] of [
      ["start", frames[0].at - capture.startedAt, frames[0]],
      ["end", capture.endedAt - frames.at(-1).at, frames.at(-1)],
    ]) {
      maximumFrameGapMs = Math.max(maximumFrameGapMs, gap);
      if (gap >= 80) frameGaps.push({ boundary, durationMs: gap, adjacent });
    }
  }
  const bots = botIds.map((id) => {
    const segments = [];
    let samples = [];
    let drawnFrames = 0;
    const missingDraws = [];
    const finish = () => {
      if (samples.length >= 2) {
        segments.push(remoteActorMotionSmoothnessEvidence(samples));
      }
      samples = [];
    };
    for (const frame of frames) {
      const bot = frame.bots?.find((entry) => entry.id === id);
      if (bot?.expectedDrawn && (!bot.drawn || bot.lifeSeq !== bot.authoritativeLifeSeq)) {
        missingDraws.push({ at: frame.at, bot });
      }
      if (!bot?.drawn) {
        finish();
        continue;
      }
      drawnFrames += 1;
      if (samples.length && samples.at(-1).lifeSeq !== bot.lifeSeq) finish();
      samples.push({ ...bot, at: frame.at,
        positionTick: observedActorSourceTick(bot.positionTick, frame.combatViewTimeline),
        combatViewTimeline: frame.combatViewTimeline });
    }
    finish();
    return {
      id, drawnFrames, segments: segments.length,
      maximumDisplayStep: Math.max(0, ...segments.map((entry) => entry.maximumDisplayStep)),
      stationaryTimelineFrames: segments.reduce((sum, entry) => sum + entry.stationaryTimelineFrames, 0),
      presentationMotionFrames: segments.reduce((sum, entry) => sum + entry.presentationMotionFrames, 0),
      presentationMotionWitnesses: segments.flatMap((entry) => entry.presentationMotionWitnesses).slice(0, 16),
      failures: segments.filter((entry) => !entry.ok),
      missingDrawCount: missingDraws.length, missingDraws: missingDraws.slice(0, 16),
    };
  });
  const observerLifeSeq = expectedObserverLifeSeq ?? frames[0]?.observerLifeSeq;
  const invalidObserverFrames = frames.filter(frame => frame.observerAlive !== true ||
    !Number.isInteger(observerLifeSeq) || frame.observerLifeSeq !== observerLifeSeq);
  const observer = { ok: frames.length > 0 && invalidObserverFrames.length === 0,
    lifeSeq: observerLifeSeq, invalidFrameCount: invalidObserverFrames.length,
    invalidFrames: invalidObserverFrames.slice(0, 16).map(frame => ({ at: frame.at,
      lifeSeq: frame.observerLifeSeq, alive: frame.observerAlive })) };
  return {
    ok: windowValid && observer.ok && capture.overflow === 0 &&
      capture.totalFrames === frames.length && spanMs >= capture.durationMs - 100 &&
      frameGaps.length === 0 && bots.length > 0 && bots.every((bot) =>
        bot.drawnFrames >= 100 && bot.segments > 0 && bot.failures.length === 0 &&
        bot.missingDrawCount === 0
      ),
    durationMs: capture?.durationMs, spanMs, frameCount: frames.length,
    overflow: capture?.overflow, maximumFrameGapMs,
    observerCostMs: capture?.observerCostMs,
    maximumObserverCostMs: capture?.maximumObserverCostMs,
    frameGapCount: frameGaps.length, frameGaps: frameGaps.slice(0, 16), observer, bots,
  };
}

export async function proveRemoteMotionSoak(actor, botIds, durationMs, report, { server, turnRight } = {}) {
  if (botIds.length !== 2 || new Set(botIds).size !== 2) {
    throw new Error("sustained bot motion acceptance requires two distinct bots");
  }
  if (!Number.isInteger(durationMs) || durationMs < 240_000 || durationMs > 300_000) {
    throw new Error("motion soak must retain 240000–300000 ms of uninterrupted play");
  }
  const observerPreparation = await prepareRemoteMotionObserver(actor, server, turnRight);
  report("motion_soak_observer_prepared", observerPreparation);
  await actor.evaluate(`(${installRemoteMotionCapture.toString()})(${JSON.stringify(botIds)}, ${durationMs})`);
  const deadline = Date.now() + durationMs + 100;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, Math.min(30_000, deadline - Date.now())));
    report("motion_soak_progress", { remainingMs: Math.max(0, deadline - Date.now()) });
  }
  const retained = await captureBrowserJsonReader(actor, size =>
    `(${completedMotionReader.toString()})(${createJsonChunkReader.toString()},${size})`);
  const capture = retained.value;
  report("motion_soak_export", { timing: retained.timing });
  const evidence = remoteMotionSoakEvidence(capture, botIds, observerPreparation.lifeSeq);
  evidence.observerPreparation = observerPreparation;
  // Keep the compact result even if a later combat/lifecycle assertion fails.
  report("motion_soak_complete", { evidence });
  if (!evidence.ok) {
    throw new Error(`sustained bot presentation failed: ${JSON.stringify(evidence)}`);
  }
  return evidence;
}
