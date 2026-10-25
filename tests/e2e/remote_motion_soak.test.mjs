import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { installRemoteMotionCapture, remoteMotionSoakEvidence, completedMotionReader, prepareRemoteMotionObserver } from "./remote_motion_soak.mjs";
import { createJsonChunkReader } from "./cdp_json_capture.mjs";

function captureFixture() {
  const frames = Array.from({ length: 121 }, (_, index) => ({
    at: index * 10, observerLifeSeq: 2, observerAlive: true,
    bots: [{
      id: 768, lifeSeq: 1, snapCount: 0, initialCount: 1, drawn: true,
      authoritativeLifeSeq: 1, expectedDrawn: true,
      positionX: index, positionY: 0, displayX: index, displayY: 0,
      velocityX: 100, velocityY: 0,
    }],
  }));
  return { startedAt: 0, durationMs: 1_200, totalFrames: frames.length, overflow: 0, endedAt: 1_200, frames };
}

test("sustained motion rejects late stalls, snaps, incomplete capture, and no actual draw", () => {
  const clean = captureFixture();
  assert.equal(remoteMotionSoakEvidence(clean, [768]).ok, true);
  const stall = structuredClone(clean);
  for (const frame of stall.frames.slice(100)) frame.at += 120;
  assert.equal(remoteMotionSoakEvidence(stall, [768]).frameGapCount, 1);
  assert.equal(remoteMotionSoakEvidence(stall, [768]).ok, false);
  const snap = structuredClone(clean);
  snap.frames.at(-1).bots[0].snapCount += 1;
  assert.equal(remoteMotionSoakEvidence(snap, [768]).ok, false);
  assert.equal(remoteMotionSoakEvidence({ ...clean, overflow: 1 }, [768]).ok, false);
  assert.equal(remoteMotionSoakEvidence({ ...clean, endedAt: null }, [768]).ok, false);
  const invisible = structuredClone(clean);
  invisible.frames.forEach((frame) => { frame.bots[0].drawn = false; });
  assert.equal(remoteMotionSoakEvidence(invisible, [768]).ok, false);
  const blink = structuredClone(clean);
  blink.frames[110].bots[0].drawn = false;
  assert.equal(remoteMotionSoakEvidence(blink, [768]).ok, false,
    "even one missing live drawable actor must not hide in a long capture");
});

test("sustained motion compares short source ticks in their observed full-width epoch", () => {
  const capture = captureFixture();
  for (const [index, frame] of capture.frames.entries()) {
    const sourceTick = 70000 + index;
    const renderTick = sourceTick - 7;
    const path = tick => Math.max(0, tick - 70080) * 2;
    frame.combatViewTimeline = { generation: 1, gameAppliedServerTick: sourceTick,
      scheduledRenderTick: renderTick, presentationRenderTick: renderTick, holdOrStall: false };
    Object.assign(frame.bots[0], { positionTick: sourceTick & 0xffff,
      positionSnapshotId: index + 1, positionX: path(sourceTick), displayX: path(renderTick),
      velocityX: 200 });
  }
  const normal = remoteMotionSoakEvidence(capture, [768]);
  assert.equal(normal.ok, true, "source restart must play at the older render cursor after u16 wrap");
  assert.ok(normal.bots[0].stationaryTimelineFrames > 0);
  const frozen = structuredClone(capture);
  frozen.frames.forEach(frame => { frame.bots[0].displayX = 0; });
  assert.equal(remoteMotionSoakEvidence(frozen, [768]).ok, false,
    "a continuing hold after the presented source moves still fails");
  const missing = structuredClone(capture);
  missing.frames.forEach(frame => { delete frame.combatViewTimeline.gameAppliedServerTick; });
  assert.equal(remoteMotionSoakEvidence(missing, [768]).ok, false,
    "a short source tick without an observed full-width anchor grants no allowance");
});

test("capture is bounded, observes post-render facts, and restores the original trace writer", () => {
  let timer;
  let now = 0;
  const trace = [];
  const originalPush = trace.push;
  const context = vm.createContext({
    window: {
      __zubspaceMetricsTrace: trace,
      __zubspaceRemoteSpriteTrace: [{ entries: [{ id: 768, lifeSeq: 2, drawn: true }] }],
    },
    performance: { now: () => now },
    structuredClone,
    setTimeout: (callback) => { timer = callback; },
  });
  vm.runInContext(`(${installRemoteMotionCapture.toString()})([768], 1000, 2)`, context);
  const frame = {
    combatViewTimeline: { nested: { renderTick: 42 } },
    at: 12, remotePlayers: [{ id: 768, lifeSeq: 2, displayX: 32, displayY: 64, lastServerPositionTick: 42 }],
    remoteActorPresentation: [{ id: 768, snapCount: 3, lastSeenSnapshotId: 19 }],
    authoritativeSnapshotActors: [{ id: 768, lifeSeq: 2 }],
  };
  for (let index = 0; index < 3; index += 1) { now += 10; trace.push(frame); }
  assert.equal(trace.length, 3, "the product trace still receives every original frame");
  const capture = context.window.__zubspaceMotionSoak;
  assert.equal(capture.totalFrames, 3);
  assert.equal(capture.frames.length, 2);
  assert.equal(capture.overflow, 1);
  assert.equal(capture.frames[0].bots[0].drawn, true);
  assert.equal(capture.frames[0].bots[0].snapCount, 3);
  assert.equal(capture.frames[0].bots[0].positionTick, 42);
  assert.equal(capture.frames[0].bots[0].positionSnapshotId, 19);
  assert.equal(capture.frames[0].bots[0].expectedDrawn, true);
  assert.deepEqual(structuredClone(trace), [frame, frame, frame],
    "failure artifacts must remain cloneable while observation is active");
  assert.throws(() => vm.runInContext(`(${completedMotionReader.toString()})(null, 16)`, context), /completed and sealed/);
  frame.combatViewTimeline.nested.renderTick = 99;
  assert.equal(capture.frames[0].combatViewTimeline.nested.renderTick, 42,
    "later live metrics cannot rewrite owned motion history");
  assert.ok(Object.isFrozen(capture.frames[0].combatViewTimeline.nested));
  timer();
  assert.ok(Object.isFrozen(capture));
  assert.ok(Object.isFrozen(capture.frames));
  assert.throws(() => { capture.frames[0].bots[0].drawn = false; }, TypeError);
  const reader = vm.runInContext(`(${completedMotionReader.toString()})(${createJsonChunkReader.toString()}, 16)`, context);
  let encoded = '';
  for (;;) { const chunk=reader.read(); encoded+=chunk.data; if(chunk.done) break; }
  assert.deepEqual(JSON.parse(encoded), JSON.parse(JSON.stringify(capture)), "sealed export keeps every frame");
  assert.equal(trace.push, originalPush);
  assert.equal(Object.hasOwn(trace, "push"), false,
    "cleanup restores inherited push without leaving an enumerable function");
  assert.deepEqual(structuredClone(trace), [frame, frame, frame]);
  trace.push(frame);
  assert.equal(capture.totalFrames, 3, "capture is frozen before DevTools reads it");
});

test("sustained motion includes the opening and closing partial frame intervals", () => {
  for (const boundary of ["start", "end"]) {
    const capture = captureFixture();
    capture.frames = boundary === "start" ? capture.frames.slice(8) : capture.frames.slice(0, -8);
    capture.totalFrames = capture.frames.length;
    const evidence = remoteMotionSoakEvidence(capture, [768]);
    assert.equal(evidence.ok, false, `${boundary} stall cannot hide in the span tolerance`);
    assert.equal(evidence.maximumFrameGapMs, 80);
    assert.equal(evidence.frameGapCount, 1);
    assert.equal(evidence.frameGaps[0].boundary, boundary);
  }
});

test("a motion window rejects a dead, spectator, replaced or unbound observer", () => {
  for (const mutation of [
    frame => { frame.observerAlive = false; },
    frame => { frame.observerLifeSeq += 1; },
    frame => { delete frame.observerLifeSeq; },
    frame => { delete frame.observerAlive; },
  ]) {
    const capture = captureFixture(); mutation(capture.frames[80]);
    const result = remoteMotionSoakEvidence(capture, [768]);
    assert.equal(result.ok, false);
    assert.equal(result.observer.invalidFrameCount, 1);
  }
  assert.equal(remoteMotionSoakEvidence(captureFixture(), [768], 1).ok, false,
    "even a stable successor cannot replace the life whose turn was admitted before capture");
});

async function observerPreparationFixture({ admitted = true, changedLife = false, baseline = false, structured = false } = {}) {
  const metrics = { selfPlayerId: 0, playerLifeSeq: 2, playerAlive: true, serverPlayerAlive: true,
    clientInputSeq: 65534, playerProtocolDirection: 0,
    localGameplayState: { authorityPhase: "Live", authorityScopeServerSessionId: "1", authorityScopeArenaStreamId: "2" } };
  const actor = { metricsState: async () => ({ uiMetrics: structuredClone(metrics) }) }, server = { logs: [] };
  let presses = 0;
  const turn = async () => {
    presses++; metrics.playerProtocolDirection = 3; metrics.clientInputSeq = 2;
    if (changedLife) metrics.playerLifeSeq++;
    if (admitted) server.logs.push((structured ? JSON.stringify({ fields: { message: "human authority input trace",
      player_id: 0, life_seq: 2, input_sequence: 1, input_direction: 2, reliable_lifecycle_baseline: baseline } }) :
      `INFO human authority input trace player_id=0 life_seq=2 input_sequence=1 input_direction=2 reliable_lifecycle_baseline=${baseline}`) + "\n");
  };
  try { return { result: await prepareRemoteMotionObserver(actor, server, turn, 25), presses }; }
  catch (error) { return { error, presses }; }
}

test("observer participation requires ordinary server-admitted direction change in the same life", async () => {
  for (const structured of [false, true]) {
    const { result, error, presses } = await observerPreparationFixture({ structured });
    assert.ifError(error); assert.equal(presses, 1);
    assert.equal(result.keysOnly, true); assert.equal(result.lifeSeq, 2);
    assert.match(result.acceptedInput, /human authority input trace/);
  }
  for (const options of [{ admitted: false }, { baseline: true }, { changedLife: true }]) {
    const { result, error } = await observerPreparationFixture(options);
    assert.equal(result, undefined);
    assert.match(error.message, /admitted-input evidence|lifecycle boundary/);
  }
});
