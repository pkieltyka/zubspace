import assert from "node:assert/strict";
import test from "node:test";
import { recoveryMembershipReady } from "./recovery_membership.mjs";

const expected = [{ id: 0, lifeSeq: 1 }, { id: 1, lifeSeq: 3 }];
const states = () => expected.map((self, index) => ({ uiMetrics: {
  selfPlayerId: self.id, playerLifeSeq: self.lifeSeq,
  playerAlive: true, serverPlayerAlive: true,
  authoritativeSnapshotActors: [{ id: expected[1 - index].id, lifeSeq: expected[1 - index].lifeSeq }],
} }));

test("both exact peer lives must be delivered before measured recovery", () => {
  assert.equal(recoveryMembershipReady(states(), expected), true);
  for (const index of [0, 1]) {
    const sample = states(); sample[index].uiMetrics.authoritativeSnapshotActors = [];
    assert.equal(recoveryMembershipReady(sample, expected), false);
  }
});

test("a peer ID from an old or later life cannot satisfy recovery membership", () => {
  for (const index of [0, 1]) for (const delta of [-1, 1]) {
    const sample = states(); sample[index].uiMetrics.authoritativeSnapshotActors[0].lifeSeq += delta;
    assert.equal(recoveryMembershipReady(sample, expected), false);
  }
});

test("local death, replacement or life transition invalidates the staged pair", () => {
  for (const index of [0, 1]) for (const [key, value] of [
    ["selfPlayerId", 8], ["playerLifeSeq", 7], ["playerAlive", false], ["serverPlayerAlive", false],
  ]) {
    const sample = states(); sample[index].uiMetrics[key] = value;
    assert.equal(recoveryMembershipReady(sample, expected), false);
  }
});

test("distant live spawns do not imply mutual AOI membership", () => {
  const sample = states();
  for (const [index, position] of [[0, [7032, 9352]], [1, [9320, 7240]]]) {
    const m = sample[index].uiMetrics; [m.playerX, m.playerY] = position;
    m.authoritativeSnapshotActors = [{ id: m.selfPlayerId, lifeSeq: m.playerLifeSeq }];
  }
  assert.equal(recoveryMembershipReady(sample, expected), false);
  // Only a delivered exact peer, not a convenient distance threshold, opens the gate.
  for (let index = 0; index < 2; index++) {
    sample[index].uiMetrics.authoritativeSnapshotActors.push(expected[1 - index]);
  }
  assert.equal(recoveryMembershipReady(sample, expected), true);
});

test("missing scopes and duplicate identities fail closed", () => {
  for (const bad of [null, [], [expected[0]], [expected[0], expected[0]], [{ id: null, lifeSeq: 1 }, expected[1]]])
    assert.equal(recoveryMembershipReady(states(), bad), false);
  assert.equal(recoveryMembershipReady([], expected), false);
});
