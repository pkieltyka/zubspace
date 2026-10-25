import assert from "node:assert/strict";

const tickDiff = (a, b) => ((a - b) << 1) >> 1;

function states(packetLog, hit, group) {
  const records = new Map();
  for (const entry of packetLog) {
    const p = Buffer.from(entry.bytes || []);
    if (entry.direction !== "received" || p[0] !== 125 || p[1] !== 2 || p[2] !== 4) continue;
    assert.ok(p.length === 122 || p.length === 124, "complete projectile-state wire record");
    assert.equal(p.length, 122 + (p[105] & 8 ? 2 : 0));
    if (p.readUInt16LE(27) !== hit.shooterId ||
        p.readUInt32LE(37) !== hit.shooterLifeSeq || p.readBigUInt64LE(41).toString() !== String(group)) continue;
    assert.equal(p.readUInt16LE(83), hit.weapon, "held and terminal facts must preserve the exact weapon");
    const record = { at: entry.at, server: p.readBigUInt64LE(3).toString(),
      arena: p.readBigUInt64LE(11).toString(), event: p.readUInt32LE(19), tick: p.readUInt32LE(23),
      session: p.readBigUInt64LE(29).toString(), flags: p[105], x: p.readFloatLE(85), y: p.readFloatLE(89),
      id: p.readUInt32LE(74),
      bytes: [...p] };
    const key = `${record.server}/${record.arena}/${record.event}`;
    const previous = records.get(key);
    if (previous) assert.deepEqual(previous.bytes, record.bytes, "duplicate wire copies must agree");
    if (!previous || record.at < previous.at) records.set(key, record);
  }
  return [...records.values()];
}

/** Actual received held state + multiple immutable render poses + a complete
 * terminal tail. No synthetic absence, zero-velocity substitution or timer can
 * stand in for an authoritative hold. The raw proof is retained for re-audit. */
export function heldProjectileEvidence({ packetLog, frames, hit, group, linkedIds = [hit.projectileId] }) {
  assert.ok(linkedIds.includes(hit.projectileId) && new Set(linkedIds).size === linkedIds.length);
  const facts = states(packetLog, hit, group);
  const terminal = facts.find(f => f.id === hit.projectileId && (f.flags & 4) && f.tick === hit.serverTick);
  assert.ok(terminal, "exact hit must own a received terminal at its server tick");
  const matches = p => p.sourcePlayerId === hit.shooterId && p.sourceLifeSeq === hit.shooterLifeSeq &&
    String(p.sourceSessionId) === terminal.session && String(p.shotGroupId) === String(group);
  const heldFacts = facts.filter(f => (f.flags & 16) && !(f.flags & 4) &&
    f.server === terminal.server && f.arena === terminal.arena && f.session === terminal.session &&
    tickDiff(terminal.tick, f.tick) >= 0 && f.at < terminal.at);
  assert.ok(heldFacts.length, "a real pre-terminal authoritative hold is required");
  let proof = null;
  for (const held of heldFacts.filter(f => f.id === hit.projectileId)) {
    const candidates = frames.filter(f => f.at >= held.at && f.at < terminal.at);
    const drawn = candidates.filter(f => linkedIds.every(id => f.projectiles?.entries.some(p => matches(p) &&
      p.authorityProjectileId === id && p.motionFrozen === true && p.drawn === true)));
    if (drawn.length < 2 || drawn.at(-1).at - drawn[0].at < 16) continue;
    const window = candidates.filter(f => f.at >= drawn[0].at && f.at <= drawn.at(-1).at);
    const children = linkedIds.map(id => heldFacts.find(f => f.id === id && f.at <= drawn[0].at));
    assert.ok(children.every(Boolean), "each linked child must own a received held fact");
    let previous = null;
    const firstTtl = new Map(), lastTtl = new Map();
    for (const frame of window) {
      assert.equal(frame.projectiles.overflow, 0, "held render proof cannot be truncated");
      if (previous) assert.ok(frame.at > previous.at && frame.at - previous.at <= 60, "held render proof must be continuous");
      for (const child of children) {
        const copies = frame.projectiles.entries.filter(p => matches(p) && p.authorityProjectileId === child.id);
        assert.equal(copies.length, 1, "each held child must have exactly one drawn copy");
        const p = copies[0];
        assert.ok(p.drawn && p.motionFrozen, "the held projectile must remain drawn and frozen");
        assert.ok(Number.isFinite(p.x) && Number.isFinite(p.y) && Number.isFinite(p.ttl));
        assert.ok(Math.abs(p.x - child.x) <= 0.02 && Math.abs(p.y - child.y) <= 0.02,
          "held draw pose must stay at the authoritative contact, including fractional frames");
        if (lastTtl.has(child.id)) assert.ok(p.ttl <= lastTtl.get(child.id) + 0.002, "holding cannot extend the weapon lifetime");
        if (!firstTtl.has(child.id)) firstTtl.set(child.id, p.ttl);
        lastTtl.set(child.id, p.ttl);
      }
      previous = frame;
    }
    for (const id of linkedIds) assert.ok(lastTtl.get(id) < firstTtl.get(id) - 0.005,
      "held flight must still age toward its weapon-owned expiry");
    proof = { held, children, frames: window, spanMs: drawn.at(-1).at - drawn[0].at };
    break;
  }
  assert.ok(proof, "at least two drawn frozen frames spanning 16 ms are required");
  const tail = frames.filter(f => f.at >= terminal.at + 50 && f.at <= terminal.at + 650);
  assert.ok(tail.length >= 12 && tail[0].at <= terminal.at + 100 && tail.at(-1).at >= terminal.at + 550,
    "complete post-terminal half-second render tail required");
  for (let i = 0; i < tail.length; i++) {
    const frame = tail[i];
    assert.equal(frame.projectiles.overflow, 0);
    if (i) assert.ok(frame.at > tail[i - 1].at && frame.at - tail[i - 1].at <= 60, "terminal tail cannot hide observation gaps");
    assert.ok(!frame.projectiles.entries.some(matches), "consumed linked group must not survive or revive");
  }
  return { held: proof, terminal, tail };
}
