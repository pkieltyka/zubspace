import test from "node:test";
import assert from "node:assert/strict";
import { assertBatchDraw, batchChanges, heldAuthorityEvidence, heldBits, heldCodes, heldRespawnSuppressionEvidence } from "./held_key_batches.mjs";

function fixture(mask = 31) {
  const state = { uiMetrics: { playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 7,
    localGameplayState: { inputBits: heldBits(mask) } }, gameKeyBridge: { trackedCodes: heldCodes(mask) } };
  const records = Array.from({ length: 16 }, (_, i) => ({ player_id: 2, life_seq: 7,
    input_bits: heldBits(mask), input_sequence: (65530 + i) & 0xffff }));
  return { state, records };
}

test("all held masks preserve exact physical/local/authority membership", () => {
  for (let mask = 0; mask < 32; mask += 1) {
    const { state, records } = fixture(mask);
    assert.ok(heldAuthorityEvidence(state, records, 2, 7, mask));
    records[0].input_bits |= 64;
    assert.ok(heldAuthorityEvidence(state, records, 2, 7, mask));
    state.gameKeyBridge.trackedCodes.push("Space");
    assert.equal(heldAuthorityEvidence(state, records, 2, 7, mask), null);
  }
});

test("stale life, lost held input, duplicate sequences and missing metrics cannot pass", () => {
  for (const mutate of [
    ({ state }) => { state.uiMetrics.playerLifeSeq = 8; },
    ({ state }) => { delete state.uiMetrics.localGameplayState.inputBits; },
    ({ records }) => { records[3].input_bits = 0; },
    ({ records }) => { records[3].input_sequence = records[2].input_sequence; },
    ({ records }) => { records[3].life_seq = 8; },
  ]) {
    const f = fixture(); mutate(f);
    assert.equal(heldAuthorityEvidence(f.state, f.records, 2, 7, 31), null);
  }
});

function drawFixture() {
  return { inputs: [
    { serial: 11, kind: "keydown", code: "ControlLeft", renderOrdinal: 20, playerLifeSeq: 7 },
    { serial: 12, kind: "keydown", code: "ArrowLeft", renderOrdinal: 20, playerLifeSeq: 7 },
  ], frames: [{ renderOrdinal: 21, playerLifeSeq: 7, playerAlive: true, serverPlayerAlive: true,
    localRenderPrediction: { inputBits: 4 } }] };
}

test("batch draw requires real ordered events and the exact next draw", () => {
  assert.deepEqual(batchChanges(3, 5), [{ key: "ArrowLeft", type: "keyUp" }, { key: "ArrowRight", type: "keyDown" }]);
  assert.equal(assertBatchDraw(drawFixture(), 10, 0, 3, 7).sameFrame, true);
  for (const mutate of [
    f => { f.inputs.pop(); },
    f => { f.inputs[0].serial = 9; },
    f => { f.frames[0].renderOrdinal = 22; },
    f => { f.frames[0].playerLifeSeq = 8; },
    f => { f.frames[0].localRenderPrediction.inputBits = 0; },
  ]) {
    const f = drawFixture(); mutate(f);
    assert.throws(() => assertBatchDraw(f, 10, 0, 3, 7));
  }
});

test("cross-frame delivery is recorded honestly, not called simultaneous", () => {
  const f = drawFixture();
  f.inputs[0].renderOrdinal = 19;
  assert.equal(assertBatchDraw(f, 10, 0, 3, 7).sameFrame, false);
});

test("respawn suppression needs physical holds but neutral local and authoritative input", () => {
  const make = () => {
    const f = fixture(0);
    f.state.gameKeyBridge.trackedCodes = heldCodes(7);
    f.state.uiMetrics.pendingWeapon = 0;
    for (const record of f.records) { record.reported_weapon = 0; record.accepted_weapon = 0; }
    return f;
  };
  const f = make();
  assert.ok(heldRespawnSuppressionEvidence(f.state, f.records, 2, 7, 7));
  for (const mutate of [
    f => { f.state.gameKeyBridge.trackedCodes = []; },
    f => { f.state.uiMetrics.pendingWeapon = 1; },
    f => { f.state.uiMetrics.localGameplayState.inputBits = 4; },
    f => { f.records[0].reported_weapon = 66; },
    f => { f.records[0].accepted_weapon = 66; },
    f => { f.records[0].life_seq = 6; },
    f => { f.records[1].input_sequence = f.records[0].input_sequence; },
  ]) {
    const f = make(); mutate(f);
    assert.equal(heldRespawnSuppressionEvidence(f.state, f.records, 2, 7, 7), null);
  }
});
