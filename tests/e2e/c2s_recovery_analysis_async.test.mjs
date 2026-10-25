import assert from "node:assert/strict";
import test from "node:test";
import { c2sRecoveryLogSummary } from "./combat_replay.mjs";
import { c2sRecoveryLogSummaryAsync } from "./c2s_recovery_analysis_async.mjs";

const sample = (id, alignment, deviation, steps) =>
  `sim observation sample player_id=${id} deviation=${deviation} input_bits=1 input_alignment="${alignment}" position_sequence_steps=${steps}`;
const logText = [sample(7, "stable", .5, 2), sample(7, "transition", 1.5, 3),
  sample(8, "stable", 0, 1), sample(7, "missing", .5, 1),
  'server security reject category="position" reason="missing_input_record" player_id=7',
  'server security reject category="position" reason="blocked_position_hull" player_id=8',
  'server security reject category="movement" reason="snapback" player_id=7',
  'position input recovery timed out player_id=7',
].join("\n");

test("worker preserves every original scored summary and rejection count", async () => {
  const expected = c2sRecoveryLogSummary({ logs: [logText] }, 0, 7, [7, 8]);
  const actual = await c2sRecoveryLogSummaryAsync({ logText, playerId: 7, participantIds: [7, 8] });
  assert.deepEqual(actual, expected);
  assert.equal(actual.playerDeviation.stableActiveInput.samples, 1);
  assert.equal(actual.playerDeviation.missingInputSamples, 1);
  assert.equal(actual.participantDeviations[8].all.samples, 1);
  assert.equal(actual.inputWitnessRejects.missing_input_record, 1);
  assert.equal(actual.positionAdmissionRejects.blocked_position_hull, 1);
  assert.equal(actual.inputRecoveryTimeouts, 1);
  assert.equal(actual.snapbackRejects, 1);
  assert.equal(actual.replaySteps.maxSteps, 3);
});

test("separate concurrent reductions retain their own immutable windows and player identities", async () => {
  const inputs = [7, 8].map(id => ({ logText: sample(id, "stable", id / 10, 1), playerId: id, participantIds: [id] }));
  const results = await Promise.all(inputs.map(c2sRecoveryLogSummaryAsync));
  for (let i = 0; i < inputs.length; i++) {
    const input = inputs[i];
    assert.deepEqual(results[i], c2sRecoveryLogSummary({ logs: [input.logText] }, 0, input.playerId, input.participantIds));
    assert.equal(results[i].playerDeviation.all.samples, 1);
    assert.deepEqual(Object.keys(results[i].participantDeviations), [String(input.playerId)]);
  }
});

test("a broken worker reduction rejects rather than returning empty passing evidence", async () => {
  await assert.rejects(c2sRecoveryLogSummaryAsync({ logText, playerId: 7, participantIds: null }), /map/);
});

test("the frozen UTF-16 scoring window excludes its original prefix and later appended logs", async () => {
  const prefix = "older λ diagnostic\n" + sample(7, "missing", 5, 1) + "\n";
  const server = { logs: [prefix, logText] };
  const offset = prefix.length;
  const expected = c2sRecoveryLogSummary(server, offset, 7, [7, 8]);
  const frozen = server.logs.join("").slice(offset);
  const pending = c2sRecoveryLogSummaryAsync({ logText: frozen, playerId: 7, participantIds: [7, 8] });
  server.logs.push("\n" + sample(7, "invalid", 99, 1));
  assert.deepEqual(await pending, expected);
});
