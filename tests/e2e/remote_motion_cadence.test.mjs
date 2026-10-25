import test from "node:test";
import assert from "node:assert/strict";
import { assertRemoteMotionSmooth } from "./combat_replay.mjs";
import { remoteActorMotionSmoothnessEvidence } from "./bot_combat_evidence.mjs";
import { remoteDisplayStepEvidence } from "./remote_motion_cadence.mjs";

// Independent constant-velocity trajectory. Deliberately not a recording of
// the server or a generated gameplay packet: x(t) = 160 px/s * t.
function trajectory(longFrameMs = 20) {
  let at = 1000;
  return Array.from({ length: 16 }, (_, index) => {
    if (index > 0) at += index === 8 ? longFrameMs : 20;
    const x = (at - 1000) * 0.16;
    return {
      at, lifeSeq: 3, snapCount: 0, initialCount: 1,
      positionX: x, positionY: 0, displayX: x, displayY: 0,
      velocityX: 160, velocityY: 0,
      positionTick: at / 10, positionSnapshotId: index + 1,
      combatViewTimeline: { generation: 2, presentationRenderTick: at / 10 },
    };
  });
}

function coreCheck(samples) {
  return assertRemoteMotionSmooth("analytic constant velocity", {
    metricsTrace: samples.map((sample) => ({
      at: sample.at, remoteDisplayX: sample.displayX, remoteDisplayY: sample.displayY,
      remotePositionX: sample.positionX, remotePositionY: sample.positionY,
      remoteDisplayOrientation: 0, remoteVisiblePlayers: 1,
      remoteSnapCount: sample.snapCount, remoteExtrapolatedFrames: 0,
      remotePlayers: [{ lastServerPositionTick: sample.positionTick,
        velocityX: sample.velocityX, velocityY: sample.velocityY }],
    })),
  });
}

test("core cadence gate accepts constant velocity without the old fixed-frame false positive", () => {
  assert.doesNotThrow(() => coreCheck(trajectory(20)));
  assert.doesNotThrow(() => coreCheck(trajectory(40)));
  const evidence = remoteActorMotionSmoothnessEvidence(trajectory(40));
  assert.equal(evidence.ok, true);
  assert.ok(Math.abs(evidence.maximumDisplayStep - 6.4) < 1e-9);
});

test("time-aware comparison still rejects an injected spatial discontinuity", () => {
  const samples = trajectory(40);
  samples[8].displayX += 30;
  const evidence = remoteActorMotionSmoothnessEvidence(samples);
  assert.equal(evidence.ok, false);
  assert.ok(evidence.excessiveJumps.length > 0);
});

test("the bot checker cannot replace the stricter core gate wholesale", () => {
  const samples = trajectory(20);
  samples[8].displayX += 4; // 7.2px step at unchanged cadence, not a longer frame
  assert.throws(() => coreCheck(samples), /visible remote render jump/);
  assert.equal(remoteActorMotionSmoothnessEvidence(samples).ok, true,
    "the bot checker's larger jump floor loses this existing core regression");
});

test("kinematic cadence bound preserves the smaller core discontinuity rejection", () => {
  for (const duration of [10, 16, 20, 30, 36, 40, 60]) {
    const samples = trajectory(duration);
    assert.equal(remoteDisplayStepEvidence(samples[7], samples[8]).ok, true);
  }
  const samples = trajectory(20);
  samples[8].displayX += 4;
  assert.equal(remoteDisplayStepEvidence(samples[7], samples[8]).ok, false);
  const stalled = trajectory(100);
  assert.equal(remoteDisplayStepEvidence(stalled[7], stalled[8]).reason, "render-stall");
  const malformed = { ...samples[8], velocityX: NaN };
  assert.equal(remoteDisplayStepEvidence(samples[7], malformed).reason, "invalid-render-interval");
});

test("retained Chrome diagonal step is explained by its measured render interval", () => {
  // resource-combat-GgEjd1: consecutive draw ordinals 1549 -> 1550,
  // same actor life 3, generation 2, no snap-counter change.
  const previous = { monotonicMs: 35746.1, displayX: 8154.39, displayY: 8210.2,
    velocityX: -60.7, velocityY: 147.8 };
  const current = { ...previous, monotonicMs: 35781.7, displayX: 8152, displayY: 8216 };
  const evidence = remoteDisplayStepEvidence(previous, current);
  assert.equal(evidence.ok, true);
  assert.ok(evidence.distance > 6);
  assert.ok(Math.abs(evidence.dtMs - 35.6) < 1e-8);
  assert.equal(remoteDisplayStepEvidence(previous, { ...current,
    monotonicMs: previous.monotonicMs + 20 }).ok, false,
  "the same displacement without the actual longer frame must still fail");
});

test("time-aware comparison does not excuse a render stall or lifecycle snap", () => {
  const stalled = remoteActorMotionSmoothnessEvidence(trajectory(100));
  assert.equal(stalled.ok, false);
  assert.equal(stalled.renderFrameGaps.length, 1);
  const samples = trajectory(40);
  for (let index = 8; index < samples.length; index++) samples[index].snapCount = 1;
  const snapped = remoteActorMotionSmoothnessEvidence(samples);
  assert.equal(snapped.ok, false);
  assert.equal(snapped.presentationSnaps.length, 1);
});
