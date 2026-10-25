import assert from "node:assert/strict";
import test from "node:test";
import { ownerBombHandoffEvidence, ownerBombConvergenceEvidence } from "./owner_bomb_handoff_evidence.mjs";

function fixture() {
  const handoff = { phase: "owner-bomb-handoff", activation: "18446744073709551615",
    serverSessionId: "18446744073709551614", arenaStreamId: "2", sourceSessionId: "3",
    shotGroupId: "4", sourcePlayerId: 9, sourceLifeSeq: 7, sourceProjectileIndex: 0,
    authorityProjectileId: 33, inputSequence: 65535, weaponData: 3, monotonicMs: 105, terminal: false, motionFrozen: false };
  const local = { activation: handoff.activation, sourceLifeSeq: 7, inputSequence: null,
    sourceProjectileIndex: 0, drawn: true, x: 100, y: 100, velocityX: 100, velocityY: 0,
    animationSeconds: 0.1, bouncesRemaining: 1 };
  const confirmed = { sourcePlayerId: 9, sourceSessionId: "3", sourceLifeSeq: 7,
    shotGroupId: "4", sourceProjectileIndex: 0, authorityProjectileId: 33, drawn: true,
    x: 95, y: 100, velocityX: 100, velocityY: 0, animationSeconds: 0.05,
    bouncesRemaining: 1, motionFrozen: false };
  const base = { serverSessionId: handoff.serverSessionId, arenaStreamId: "2", viewerPlayerId: 9 };
  return { admissions: [handoff], frames: [
    { ...base, monotonicMs: 100, spriteFrameOrdinal: 1,
      predictedBombs: { total: 1, overflow: 0, entries: [local] }, projectiles: { total: 0, overflow: 0, entries: [] } },
    { ...base, monotonicMs: 110, spriteFrameOrdinal: 2,
      predictedBombs: { total: 0, overflow: 0, entries: [] }, projectiles: { total: 1, overflow: 0, entries: [confirmed] } },
  ] };
}

test("exact draw discontinuity keeps large identities and unbound first-frame activation", () => {
  const report = ownerBombHandoffEvidence(fixture());
  assert.equal(report.incomplete.length, 0);
  assert.equal(report.cases.length, 1);
  assert.equal(report.cases[0].backward, true);
  assert.equal(report.cases[0].alongVelocityPx, -5);
  assert.equal(report.cases[0].linearResidualPx, 6);
  assert.equal(report.cases[0].activation, "18446744073709551615");
});

test("wrong authenticated source, life, group, projectile or child cannot replace an exact draw", () => {
  for (const key of ["sourcePlayerId", "sourceSessionId", "sourceLifeSeq", "shotGroupId",
    "sourceProjectileIndex", "authorityProjectileId"]) {
    const f = fixture(), row = f.frames[1].projectiles.entries[0];
    row[key] = typeof row[key] === "string" ? "99" : 99;
    const report = ownerBombHandoffEvidence(f);
    assert.equal(report.cases.length, 0, key);
    assert.equal(report.incomplete[0].reason, "missing-last-predicted-or-first-canonical-frame");
  }
});

test("overflow, ordinal gaps and lifecycle scope cuts are incomplete evidence", () => {
  for (const mutate of [f => f.frames[0].predictedBombs.overflow = 1,
    f => f.frames[1].projectiles.overflow = 1,
    f => f.frames[1].projectiles.total = 2,
    f => f.frames[1].spriteFrameOrdinal = 3,
    f => f.frames[1].arenaStreamId = "9"]) {
    const f = fixture(); mutate(f);
    assert.equal(ownerBombHandoffEvidence(f).cases.length, 0);
  }
});

test("real bounce, Repel velocity change, frozen state and terminal do not become free-flight regressions", () => {
  for (const mutate of [f => f.frames[1].projectiles.entries[0].bouncesRemaining = 0,
    f => f.frames[1].projectiles.entries[0].velocityX = -100,
    f => f.frames[1].projectiles.entries[0].motionFrozen = true,
    f => f.admissions[0].motionFrozen = true]) {
    const f = fixture(); mutate(f);
    const report = ownerBombHandoffEvidence(f);
    assert.equal(report.cases[0].freeFlightComparable, false);
    assert.equal(report.cases[0].backward, false);
  }
  const terminal = fixture(); terminal.admissions[0].terminal = true;
  assert.equal(ownerBombHandoffEvidence(terminal).cases.length, 0);
});

test("duplicate identities and hidden draws cannot claim a visible handoff", () => {
  for (const mutate of [f => f.frames[1].projectiles.entries.push(f.frames[1].projectiles.entries[0]),
    f => f.frames[0].predictedBombs.entries[0].drawn = false,
    f => f.frames[1].projectiles.entries[0].drawn = false,
    f => f.frames[1].projectiles.entries[0].x = NaN,
    f => f.admissions[0].activation = "18446744073709551616"]) {
    const f = fixture(); mutate(f);
    assert.equal(ownerBombHandoffEvidence(f).cases.length, 0);
  }
});
function convergenceFixture() {
  const f = fixture(), row = f.frames[1].projectiles.entries[0];
  for (let i = 0; i <= 30; i++) {
    const lead = 0.05 * Math.max(0, 1 - i / 25);
    f.frames[i + 1] = { ...f.frames[1], monotonicMs: 110 + i * 10, spriteFrameOrdinal: i + 2,
      projectiles: { total: 1, overflow: 0, entries: [{ ...row, x: 100 + i * 0.8,
        age: 0.05 + i / 100, posePartialSeconds: 0,
        animationSeconds: 0.05 + i / 100 + lead, ownerPredictionLeadSeconds: lead }] } };
  }
  return { observation: f, handoff: ownerBombHandoffEvidence(f).cases[0] };
}

test("full exact canonical draw sequence must converge without moving a reversal to a later frame", () => {
  const { observation, handoff } = convergenceFixture();
  const result = ownerBombConvergenceEvidence(observation, handoff);
  assert.equal(result.ok, true); assert.equal(result.frames, 31);
  assert.equal(result.finalLeadSeconds, 0); assert.equal(result.durationMs, 300);
});

test("a missing draw/frame/scope or absent lead clock cannot prove convergence", () => {
  for (const mutate of [f => f.frames.splice(4, 1), f => f.frames[5].projectiles.entries = [],
    f => f.frames[5].arenaStreamId = "99", f => f.frames[5].projectiles.overflow = 1,
    f => f.frames[5].projectiles.entries[0].sourceSessionId = "99",
    f => delete f.frames[5].projectiles.entries[0].ownerPredictionLeadSeconds,
    f => f.frames.splice(-5)]) {
    const { observation, handoff } = convergenceFixture(); mutate(observation);
    assert.equal(ownerBombConvergenceEvidence(observation, handoff).ok, false);
  }
});

test("late backward steps, animation resets, restarted lead and unfinished fade are rejected", () => {
  for (const mutate of [f => f.frames[7].projectiles.entries[0].x -= 10,
    f => f.frames[7].projectiles.entries[0].animationSeconds = 0,
    f => f.frames[7].projectiles.entries[0].ownerPredictionLeadSeconds = 0.1,
    f => { const row = f.frames.at(-1).projectiles.entries[0]; row.ownerPredictionLeadSeconds = 0.01; row.animationSeconds += 0.01; },
    f => f.frames[7].projectiles.entries[0].bouncesRemaining = 0,
    f => f.frames[7].projectiles.entries[0].motionFrozen = true]) {
    const { observation, handoff } = convergenceFixture(); mutate(observation);
    assert.equal(ownerBombConvergenceEvidence(observation, handoff).ok, false);
  }
});
