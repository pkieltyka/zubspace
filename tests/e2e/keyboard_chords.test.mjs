import assert from "node:assert/strict";
import test from "node:test";
import { FIRE_TURN_CHORD_CASES, AFTERBURNER_CASES, singleHeldKeyEvidence, safeZoneThrustEvidence } from "./keyboard_chords.mjs";

function safeWitness() {
  return {
    playerId: 7, lifeSeq: 3,
    isAuthoredSafe: (x, y) => x >= 8192 && x < 8208 && y >= 8192 && y < 8208,
    state: {
      gameKeyBridge: { trackedCodes: ["ControlLeft", "ArrowUp"] },
      uiMetrics: { playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 3,
        inSafeZone: true, localGameplayState: { inputBits: 1 } },
    },
    records: Array.from({ length: 64 }, (_, i) => ({ player_id: 7, life_seq: 3,
      input_sequence: (65520 + i) & 0xffff, input_bits: i % 16 === 0 ? 65 : 1,
      reported_weapon: 0, accepted_weapon: 0, x: 8200, y: 8200 })),
  };
}

test("safe-zone proof requires repeated exact-life stops and forward input across sequence wrap", () => {
  const proof = safeZoneThrustEvidence(safeWitness());
  assert.equal(proof.stopCount, 4);
  assert.equal(proof.acceptedCount, 64);
});

test("safe-zone proof rejects missing stops, unsafe geometry, firing, stale state and duplicated input", () => {
  for (const mutate of [
    x => { x.records.pop(); },
    x => { x.records[0].input_bits = 1; },
    x => { x.records[1].input_bits = 0; },
    x => { x.records[1].accepted_weapon = 1; },
    x => { x.records[1].reported_weapon = 1; },
    x => { x.records[1].x = 8300; },
    x => { x.records[1].life_seq = 2; },
    x => { x.records[1].input_sequence = x.records[0].input_sequence; },
    x => { x.records[1].input_sequence = (x.records[0].input_sequence - 1) & 0xffff; },
    x => { x.state.uiMetrics.inSafeZone = false; },
    x => { x.state.uiMetrics.playerLifeSeq = 4; },
    x => { x.state.uiMetrics.playerAlive = false; },
    x => { x.state.uiMetrics.localGameplayState.inputBits = 0; },
    x => { x.state.gameKeyBridge.trackedCodes = ["ArrowUp"]; },
  ]) {
    const input = safeWitness();
    mutate(input);
    assert.equal(safeZoneThrustEvidence(input), null, String(mutate));
  }
});

function assertAllPairOrders(cases, pairs) {
  const identity = ({ first, second, release }) => JSON.stringify([first, second, release]);
  const actual = cases.map(identity);
  const expected = [];
  for (const [a, b] of pairs) {
    for (const [first, second] of [[a, b], [b, a]]) {
      for (const release of [[a, b], [b, a]]) {
        expected.push(identity({ first, second, release }));
      }
    }
  }
  assert.equal(new Set(actual).size, actual.length, "no duplicate ordering proofs");
  assert.deepEqual(actual.sort(), expected.sort());
}

test("fire/turn cases include both release orders for each direction and press order", () => {
  assertAllPairOrders(FIRE_TURN_CHORD_CASES, [
    ["Control", "ArrowLeft"], ["Control", "ArrowRight"],
  ]);
});

test("Afterburner cases include both release orders for each press order", () => {
  assertAllPairOrders(AFTERBURNER_CASES, [["Shift", "ArrowUp"]]);
});

function witness(remainingKey = "ArrowLeft", bits = 4, code = "ArrowLeft", weapon = 0) {
  return {
    remainingKey, playerId: 7, lifeSeq: 3,
    state: {
      gameKeyBridge: { trackedCodes: [code] },
      uiMetrics: {
        playerAlive: true, serverPlayerAlive: true, playerLifeSeq: 3,
        localGameplayState: { inputBits: bits, afterburnerActive: false },
      },
    },
    records: Array.from({ length: 16 }, () => ({
      player_id: 7, life_seq: 3, input_bits: bits, reported_weapon: weapon,
    })),
  };
}

test("intermediate release requires exact physical, local, and authoritative single-key state", () => {
  for (const args of [
    ["Control", 0, "ControlLeft", 1], ["ArrowLeft", 4, "ArrowLeft", 0],
    ["ArrowRight", 8, "ArrowRight", 0], ["ArrowUp", 1, "ArrowUp", 0],
    ["Shift", 16, "ShiftLeft", 0],
  ]) {
    const input = witness(...args);
    const result = singleHeldKeyEvidence(input);
    assert.equal(result.remainingKey, args[0]);
    assert.equal(result.authoritativeTail.length, 16);
  }
});

test("intermediate release rejects cancellation, stuck keys, stale life, and incomplete evidence", () => {
  for (const mutate of [
    x => { x.state.gameKeyBridge.trackedCodes = []; },
    x => { x.state.gameKeyBridge.trackedCodes.push("ControlLeft"); },
    x => { x.state.uiMetrics.localGameplayState.inputBits = 0; },
    x => { x.state.uiMetrics.localGameplayState.afterburnerActive = true; },
    x => { x.state.uiMetrics.playerAlive = false; },
    x => { x.state.uiMetrics.serverPlayerAlive = false; },
    x => { x.state.uiMetrics.playerLifeSeq = 4; },
    x => { x.records.pop(); },
    x => { x.records.at(-1).input_bits = 0; },
    x => { x.records.at(-1).reported_weapon = 1; },
    x => { delete x.records.at(-1).reported_weapon; },
    x => { x.records.at(-1).life_seq = 2; },
    x => { x.records.at(-1).player_id = 8; },
  ]) {
    const input = witness();
    mutate(input);
    assert.equal(singleHeldKeyEvidence(input), null, String(mutate));
  }
  assert.equal(singleHeldKeyEvidence(witness("Control", 0, "ControlLeft", 0)), null,
    "a physical fire latch without an authoritative shot is insufficient");
});
