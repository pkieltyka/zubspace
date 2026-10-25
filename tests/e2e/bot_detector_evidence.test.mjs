import assert from "node:assert/strict";
import test from "node:test";
import { botDetectorTargetEvidence } from "./bot_detector_evidence.mjs";

const options = { sinceTick: 100, statusByHuman: new Map([[0, 3], [1, 3]]) };
function rows(status = 3) {
  return [0, 1].flatMap(i => [100, 125, 150, 175, 200].map(tick => ({
    server_tick: tick, bot_id: 768 + i, life_seq: 1, bot_xradar_selected: i === 0,
    bot_xradar_active: i === 0, bot_xradar_capable: true, assigned_human_player_id: i,
    assigned_human_status: status, detected_human_player_id: i === 0 || status !== 3 ? i : 65535,
    target_player_id: i === 0 || status !== 3 ? i : 65535,
  })));
}
test("combined concealment permits only the active detector to acquire and pursue", () => {
  const evidence = botDetectorTargetEvidence(rows(), options);
  assert.deepEqual(evidence.bots.map(b => [b.selected, b.pursuitSamples]), [[true, 5], [false, 0]]);
  for (const status of [0, 1, 2, 0x80 | 2]) {
    assert.ok(botDetectorTargetEvidence(rows(status), { ...options, statusByHuman: new Map([[0, status], [1, status]]) }));
  }
});
test("missing acquisition, hidden pursuit, borrowed detectors and status mismatch fail", () => {
  for (const change of [
    r => { r[5].detected_human_player_id = 1; },
    r => { r[5].target_player_id = 1; },
    r => { r[5].target_player_id = 0; },
    r => { r[5].bot_xradar_active = true; },
    r => { r[0].detected_human_player_id = 65535; },
    r => { r[0].assigned_human_status = 0; },
    r => { delete r[0].bot_xradar_active; },
  ]) { const samples = rows(); change(samples); assert.throws(() => botDetectorTargetEvidence(samples, options)); }
});
test("a short trace or acquisition without sustained actual pursuit is incomplete", () => {
  assert.equal(botDetectorTargetEvidence(rows().filter(r => r.server_tick < 200), options), null);
  const samples = rows(); samples[0].target_player_id = 65535;
  assert.equal(botDetectorTargetEvidence(samples, options), null);
  assert.equal(botDetectorTargetEvidence(rows().filter(r => r.bot_id === 768), options), null);
});
test("an exhausted selected detector loses hidden acquisition without losing its assignment", () => {
  const samples = rows().map(r => ({ ...r, bot_xradar_active: false,
    detected_human_player_id: 65535, target_player_id: 65535 }));
  const evidence = botDetectorTargetEvidence(samples, { ...options, requireSelectedActive: false });
  assert.equal(evidence.bots[0].selected, true); assert.equal(evidence.bots[0].activeAtEnd, false);
  assert.equal(evidence.bots[0].pursuitSamples, 0);
});
