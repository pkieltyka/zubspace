import assert from "node:assert/strict";
import test from "node:test";
import { heldProjectileEvidence as audit } from "./held_projectile_evidence.mjs";

function fixture() {
  const hit = { projectileId: 8, shooterId: 0, shooterLifeSeq: 2, serverTick: 120, weapon: 66 };
  function packet(flags, event, tick, at) {
    const p = Buffer.alloc(122); p.set([125, 2, 4]);
    p.writeBigUInt64LE(11n, 3); p.writeBigUInt64LE(22n, 11); p.writeUInt32LE(event, 19);
    p.writeUInt32LE(tick, 23); p.writeUInt16LE(0, 27); p.writeBigUInt64LE(33n, 29);
    p.writeUInt32LE(2, 37); p.writeBigUInt64LE(7n, 41); p.writeUInt32LE(8, 74);
    p.writeUInt16LE(66, 83);
    p.writeFloatLE(100, 85); p.writeFloatLE(200, 89); p[105] = flags;
    return { at, direction: "received", bytes: [...p] };
  }
  const projectile = { sourcePlayerId: 0, sourceLifeSeq: 2, sourceSessionId: "33", shotGroupId: "7",
    authorityProjectileId: 8, motionFrozen: true, drawn: true, x: 100, y: 200 };
  const frames = [];
  for (let at = 1000; at < 1100; at += 16) frames.push({ at,
    projectiles: { overflow: 0, entries: [{ ...projectile, ttl: 5 - (at - 1000) / 1000 }] } });
  for (let at = 1150; at <= 1750; at += 16) frames.push({ at, projectiles: { overflow: 0, entries: [] } });
  return { hit, group: "7", packetLog: [packet(16, 9, 110, 999), packet(4, 10, 120, 1100)], frames };
}
test("real held state keeps multiple drawn poses fixed while TTL ages, then retires", () => {
  const r = audit(fixture()); assert.equal(r.held.spanMs, 96); assert.ok(r.tail.length >= 12);
});
test("the original 38px held-flight drift and fractional draw drift are rejected", () => {
  for (const drift of [38, 0.25]) { const f = fixture(); f.frames[2].projectiles.entries[0].x += drift;
    assert.throws(() => audit(f), /authoritative contact/); }
});
test("a stationary normal projectile without a received held fact is not hold proof", () => {
  const f = fixture(); f.packetLog.shift(); assert.throws(() => audit(f), /real pre-terminal/);
});
test("another arena's hold cannot satisfy the terminal's scope", () => {
  const f = fixture(); f.packetLog[0].bytes[11]++;
  assert.throws(() => audit(f), /real pre-terminal/);
});
test("wrong session, life or group cannot supply the frozen draw", () => {
  for (const key of ["sourceSessionId", "sourceLifeSeq", "shotGroupId"]) {
    const f = fixture(); for (const frame of f.frames) for (const p of frame.projectiles.entries) p[key] = 99;
    assert.throws(() => audit(f), /two drawn frozen frames/);
  }
});
test("lost frozen flag, unchanged lifetime and a sparse trace cannot hide failure", () => {
  for (const mode of ["flag", "ttl", "gap"]) {
    const f = fixture();
    if (mode === "flag") f.frames[2].projectiles.entries[0].motionFrozen = false;
    if (mode === "ttl") for (const frame of f.frames.slice(0, 7)) frame.projectiles.entries[0].ttl = 5;
    if (mode === "gap") f.frames.splice(1, 4);
    assert.throws(() => audit(f));
  }
});
test("terminal identity and complete absence of every linked sibling remain mandatory", () => {
  const wrong = fixture(); wrong.hit.serverTick++; assert.throws(() => audit(wrong), /exact hit/);
  const revived = fixture(); revived.frames[9].projectiles.entries = [{ ...revived.frames[0].projectiles.entries[0], authorityProjectileId: 9 }];
  assert.throws(() => audit(revived), /linked group/);
  const short = fixture(); short.frames.length = 12; assert.throws(() => audit(short), /half-second/);
});

function linkedFixture() {
  const f = fixture(), p = Buffer.from(f.packetLog[0].bytes);
  p.writeUInt32LE(8, 19); p.writeUInt32LE(9, 74); p.writeFloatLE(110, 85);
  f.packetLog.push({ at: 999, direction: "received", bytes: [...p] });
  f.linkedIds = [8, 9];
  for (const frame of f.frames) if (frame.projectiles.entries.length) {
    frame.projectiles.entries.push({ ...frame.projectiles.entries[0], authorityProjectileId: 9, x: 110 });
  }
  return f;
}
test("both linked barrels require their own held wire facts and stationary aging draws", () => {
  assert.equal(audit(linkedFixture()).held.children.length, 2);
  const drift = linkedFixture(); drift.frames[2].projectiles.entries[1].x += 0.25;
  assert.throws(() => audit(drift), /authoritative contact/);
  const missing = linkedFixture(); missing.packetLog.pop();
  assert.throws(() => audit(missing), /each linked child/);
  const ttl = linkedFixture();
  for (const frame of ttl.frames.slice(0, 7)) frame.projectiles.entries[1].ttl = 5;
  assert.throws(() => audit(ttl), /weapon-owned expiry/);
});
