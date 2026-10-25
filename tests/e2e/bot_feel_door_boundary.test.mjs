import assert from "node:assert/strict";
import test from "node:test";
import { botDoorRelocationEvidence, remoteActorMotionSmoothnessEvidence,
  remoteActorMotionWithDoorEvidence } from "./bot_combat_evidence.mjs";

// Exact two-frame excerpt from the retained passive bot observation. The
// latest authority already advanced; the delayed draw crosses server tick1680.
const frames = [
  {
    "at": 1000868957601,
    "lifeSeq": 1,
    "snapCount": 1,
    "initialCount": 0,
    "lastSnapReason": 3,
    "positionX": 8936,
    "positionY": 8168,
    "positionTick": 1686,
    "positionSnapshotId": 542,
    "displayX": 8251,
    "displayY": 7651,
    "velocityX": -2.1,
    "velocityY": 11.1,
    "combatViewTimeline": {
      "generation": 1,
      "serverTick": 1686,
      "delayTicks": 7,
      "targetDelayTicks": 7,
      "scheduledRenderTick": 1679,
      "presentationRenderTick": 1679,
      "gameAppliedSnapshotId": 542,
      "gameAppliedServerTick": 1686,
      "holdOrStall": false,
      "weaponReady": true,
      "weaponView": {
        "waitingFrames": 21,
        "historicalFrames": 20,
        "currentFrames": 174,
        "missingHistoryFrames": 0,
        "effectDrops": 0
      }
    }
  },
  {
    "at": 1000868957618,
    "lifeSeq": 1,
    "snapCount": 1,
    "initialCount": 0,
    "lastSnapReason": 3,
    "positionX": 8936,
    "positionY": 8169,
    "positionTick": 1688,
    "positionSnapshotId": 543,
    "displayX": 8936,
    "displayY": 8168,
    "velocityX": -3.2,
    "velocityY": 14.8,
    "combatViewTimeline": {
      "generation": 1,
      "serverTick": 1688,
      "delayTicks": 7,
      "targetDelayTicks": 7,
      "scheduledRenderTick": 1681,
      "presentationRenderTick": 1681,
      "gameAppliedSnapshotId": 543,
      "gameAppliedServerTick": 1688,
      "holdOrStall": false,
      "weaponReady": true,
      "weaponView": {
        "waitingFrames": 21,
        "historicalFrames": 20,
        "currentFrames": 174,
        "missingHistoryFrames": 0,
        "effectDrops": 0
      }
    }
  }
];
const facts = botDoorRelocationEvidence([
  '2000-01-01T19:09:17.532595Z  INFO event="door_overlap_bot_relocated" player_id=768 life_seq=1 server_tick=1680 destination_x=8936.0 destination_y=8168.0',
]);

test("passive feel recognizes the exact delayed display cut without hiding its raw jump", () => {
  const raw = remoteActorMotionSmoothnessEvidence(frames);
  assert.equal(raw.ok, false); assert.equal(raw.excessiveJumps.length, 1);
  const proof = remoteActorMotionWithDoorEvidence(frames, 768, facts);
  assert.equal(proof.ok, true); assert.equal(proof.excessiveJumps.length, 0);
  assert.equal(proof.observedExcessiveJumps.length, 1); assert.ok(proof.maximumDisplayStep > 800);
  assert.equal(proof.authorizedDoorRelocations.length, 1);
  assert.equal(proof.authorizedDoorRelocations[0].kind, "continuous-door-display");
});

test("missing, duplicate, wrong-life, wrong-player, wrong-tick or wrong-destination facts cannot excuse a jump", () => {
  for (const events of [[], [...facts, ...facts], [{...facts[0], lifeSeq: 2}],
    [{...facts[0], playerId: 769}], [{...facts[0], serverTick: 1679}],
    [{...facts[0], destinationX: 8800}]])
    assert.equal(remoteActorMotionWithDoorEvidence(frames, 768, events).ok, false);
  const drift = structuredClone(frames); drift[1].displayX += 100;
  assert.equal(remoteActorMotionWithDoorEvidence(drift, 768, facts).ok, false);
});

test("authorized relocation facts never waive an uninterrupted-window cadence failure", () => {
  const gap = structuredClone(frames); gap[1].at += 100;
  const proof = remoteActorMotionWithDoorEvidence(gap, 768, facts);
  assert.equal(proof.ok, false); assert.equal(proof.renderFrameGaps.length, 1);
});

test("unexplained continuous sprite holds still fail with relocation facts present", () => {
  const held = Array.from({ length: 7 }, (_, i) => ({ at: i * 20, lifeSeq: 1,
    snapCount: 0, initialCount: 0, positionX: i * 3, positionY: 0,
    displayX: 0, displayY: 0, velocityX: 150, velocityY: 0 }));
  const proof = remoteActorMotionWithDoorEvidence(held, 768, facts);
  assert.equal(proof.ok, false); assert.equal(proof.materialHolds.length, 1);
});
