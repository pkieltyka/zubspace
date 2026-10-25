import assert from "node:assert/strict";
import test from "node:test";
import { movingPeerWindowEvidence, createAdmittedRecordReader } from "./moving_peer_evidence.mjs";

test("incremental admission reader preserves split lines without rescanning", () => {
  const reader = createAdmittedRecordReader();
  const chunks = ["noise\nhuman authority input trace player_id=1 server_t"];
  assert.equal(reader.read(chunks).length, 0);
  chunks.push("ick=100 x=-2.5\nhuman authority input trace player_id=2\n");
  const records = reader.read(chunks);
  assert.deepEqual(records, [{player_id: 1, server_tick: 100, x: -2.5}, {player_id: 2}]);
  assert.equal(reader.read(chunks), records);
  assert.equal(reader.stats.chunks, 2);
  assert.equal(reader.stats.characters, chunks.join("").length);
  assert.throws(() => reader.read([]), /truncated/);
});
test("record overflow fails rather than silently dropping evidence", () => {
  const reader = createAdmittedRecordReader({maximumRecords: 1});
  assert.throws(() => reader.read(["human authority input trace player_id=1\nhuman authority input trace player_id=2\n"]), /capacity/);
});

function fixture() {
  return { playerIds: [1, 2], startTick: 100, endTick: 200,
    maximumGapTicks: 10, minimumDistance: 10,
    records: [1, 2].flatMap((id) => Array.from({ length: 11 }, (_, i) => ({
      player_id: id, life_seq: 1, server_tick: 100 + i * 10,
      input_bits: 1, x: i * 2, y: id,
    }))) };
}

test("every peer covers the same sustained window", () => {
  assert.equal(movingPeerWindowEvidence(fixture()).ok, true);
});
test("sequential activity cannot masquerade as concurrent movement", () => {
  const f = fixture();
  f.records = f.records.filter((r) => r.player_id === 1 ? r.server_tick <= 150 : r.server_tick >= 150);
  assert.equal(movingPeerWindowEvidence(f).ok, false);
});
test("focus-cleared keys, missing peers, stalls and new lives fail", () => {
  for (const mutate of [
    (f) => { f.records[4].input_bits = 0; },
    (f) => { f.records = f.records.filter((r) => r.player_id !== 2); },
    (f) => { f.records.splice(3, 3); },
    (f) => { f.records[4].life_seq = 2; },
    (f) => { for (const r of f.records) r.x = 0; },
    (f) => { f.records[4].x = NaN; },
    (f) => { [f.records[3], f.records[4]] = [f.records[4], f.records[3]]; },
  ]) {
    const f = fixture(); mutate(f);
    assert.equal(movingPeerWindowEvidence(f).ok, false);
  }
});
test("bounded windows cross the Continuum tick wrap", () => {
  const f = fixture(), shift = 0x80000000 - 150;
  f.startTick = (f.startTick + shift) % 0x80000000;
  f.endTick = (f.endTick + shift) % 0x80000000;
  for (const r of f.records) r.server_tick = (r.server_tick + shift) % 0x80000000;
  assert.equal(movingPeerWindowEvidence(f).ok, true);
});
test("empty, duplicate or unbounded measurement contracts are rejected", () => {
  for (const patch of [{playerIds: []}, {playerIds: [1, 1]}, {endTick: 100},
    {maximumGapTicks: 0}, {minimumDistance: 0}, {endTick: 0x80000000}]) {
    assert.throws(() => movingPeerWindowEvidence({...fixture(), ...patch}));
  }
});
