import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { cutoverWireEvidence, readCutoverWireEvidence } from "./cutover_wire_observation.mjs";
import { parseRealtimeOwnerWeaponCancellation, parseReliableOwnerWeaponCancellation } from "../../tools/realtime_event_evidence.mjs";

function packets() {
  const cut = Buffer.alloc(52);
  cut.set([0x7d, 2, 5]);
  cut.writeBigUInt64LE(1n, 3); cut.writeBigUInt64LE(2n, 11);
  cut.writeUInt32LE(5, 19); cut.writeUInt32LE(2400, 23);
  cut.writeUInt16LE(0, 27); cut.writeBigUInt64LE(9n, 29);
  cut.writeBigUInt64LE(18n, 37); cut.writeUInt32LE(2, 45);
  const reliable = Buffer.alloc(58); reliable.set([0, 3]);
  reliable.writeUInt32LE(7, 2); cut.copy(reliable, 6);
  const left = Buffer.alloc(9); left.set([0, 3]); left.writeUInt32LE(8, 2);
  left[6] = 4; left.writeUInt16LE(0, 7);
  return {cut, reliable, left};
}
const entry = bytes => ({direction: "received", bytes: Array.from(bytes), at: 100});

test("small live evidence preserves exact cutoffs on both carriers and the ordered leave", () => {
  const {cut, reliable, left} = packets();
  const rows = [entry(cut), entry(reliable), entry(left)];
  const source = {__zubspacePacketLogFirst: [rows[0]], __zubspacePacketLog: rows.slice(1)};
  for (const key of ["uiMetrics", "__zubspaceMetricsTrace", "__zubspaceRemoteSpriteTrace"])
    Object.defineProperty(source, key, {get() { assert.fail(`live wire reader touched ${key}`); }});
  const selected = cutoverWireEvidence(source);
  assert.deepEqual(selected.packetLogFirst, [rows[0]]);
  assert.deepEqual(selected.packetLog, rows.slice(1));
  const event = parseRealtimeOwnerWeaponCancellation(Buffer.from(selected.packetLogFirst[0].bytes));
  const receipt = parseReliableOwnerWeaponCancellation(Buffer.from(selected.packetLog[0].bytes));
  assert.equal(event.sourceSessionId, "9"); assert.equal(event.throughShotGroupId, "18");
  assert.equal(receipt.reliableId, 7); assert.equal(receipt.ownerLifeSeq, 2);
  assert.equal(Buffer.from(selected.packetLog[1].bytes).readUInt32LE(2), 8);
});
test("snapshots, outgoing and unrelated or differently framed packets stay out of the live transfer", () => {
  const {cut, reliable, left} = packets();
  const wrongVersion = Buffer.from(cut); wrongVersion[1] = 1;
  const wrongInner = Buffer.from(reliable); wrongInner[8] = 1;
  const wrongRoster = Buffer.from(left); wrongRoster[6] = 3;
  const selected = cutoverWireEvidence({__zubspacePacketLog: [
    entry(Buffer.alloc(65536, 0x70)), {...entry(cut), direction: "sent"},
    entry(wrongVersion), entry(wrongInner), entry(wrongRoster), entry(cut.subarray(1)), {},
  ]});
  assert.deepEqual(selected, {packetLogFirst: [], packetLog: []});
});
test("the actual browser expression reads only the bounded wire projection", async () => {
  const window = {__zubspacePacketLogFirst: [entry(packets().cut)], __zubspacePacketLog: []};
  const result = await readCutoverWireEvidence({evaluate: expression => vm.runInNewContext(expression, {window})});
  assert.equal(result.packetLogFirst.length, 1); assert.equal(result.packetLog.length, 0);
});
