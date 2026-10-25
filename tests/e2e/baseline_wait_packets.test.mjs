import test from 'node:test';
import assert from 'node:assert/strict';
import { auditBaselineWaitGamePackets as audit } from './baseline_wait_packets.mjs';
const probe = (type = 0x81) => { const b = Array(30).fill(0); b[0] = type; b[1] = 1; b[2] = 7; b[10] = 9; b[18] = 3; b[22] = 11; return b; };
const packet = (seq, direction, bytes, channel = 'zubspace-game') => ({ seq, direction, bytes, detail: { channel } });
const start = () => packet(1, 'received', [0x70]);
test('one exact scoped challenge/echo remains live control traffic', () => {
  const result = audit([start(), packet(2, 'received', probe()), packet(3, 'sent', probe(0x82))], 1);
  assert.equal(result.windowComplete, true); assert.equal(result.gameplayPacketsDuringWait, 0);
  assert.equal(result.qualityEchoPacketsDuringWait, 1); assert.equal(result.totalGameChannelPacketsDuringWait, 1);
});
test('ordinary positions, input and unknown game packets remain forbidden', () => {
  const result = audit([start(), ...[0x03, 0x62, 0xff].map((type, i) => packet(i + 2, 'sent', [type]))], 1);
  assert.equal(result.gameplayPacketsDuringWait, 3); assert.equal(result.qualityEchoPacketsDuringWait, 0);
});
test('invalid length, version, empty scope and mismatched nonce do not bypass the gate', () => {
  const candidates = [probe(0x82).slice(0, 29), [...probe(0x82), 0], probe(0x82), probe(0x82), probe(0x82)];
  candidates[2][1] = 2; candidates[3].fill(0, 2, 10); candidates[4][22] = 12;
  const result = audit([start(), packet(2, 'received', probe()), ...candidates.map((b, i) => packet(i + 3, 'sent', b))], 1);
  assert.equal(result.gameplayPacketsDuringWait, 5); assert.equal(result.qualityEchoPacketsDuringWait, 0);
});
test('an echo requires a preceding challenge on the actual game channel', () => {
  for (const entries of [[], [packet(3, 'received', probe())], [packet(0, 'received', probe(), 'zubspace-reliable')]]) {
    assert.equal(audit([start(), packet(2, 'sent', probe(0x82)), ...entries], 1).gameplayPacketsDuringWait, 1);
  }
});
test('one received challenge cannot legitimize a replayed extra echo', () => {
  const result = audit([start(), packet(2, 'received', probe()), packet(3, 'sent', probe(0x82)), packet(4, 'sent', probe(0x82))], 1);
  assert.equal(result.qualityEchoPacketsDuringWait, 1); assert.equal(result.gameplayPacketsDuringWait, 1);
});
test('an overwritten starting sequence reports incomplete evidence', () => {
  const result = audit([packet(2, 'received', probe()), packet(3, 'sent', probe(0x82))], 1);
  assert.equal(result.windowComplete, false);
});
test('invalid packet sequence metadata fails the audit', () => {
  assert.throws(() => audit([], -1)); assert.throws(() => audit([packet(null, 'sent', [3])], 0));
});

test('a reply before the wait consumes its challenge and cannot excuse a later replay', () => {
  const result = audit([packet(1, 'received', probe()), packet(2, 'sent', probe(0x82)),
    packet(3, 'sent', probe(0x82))], 2);
  assert.equal(result.windowComplete, true);
  assert.equal(result.qualityEchoPacketsDuringWait, 0);
  assert.equal(result.gameplayPacketsDuringWait, 1);
});
test('duplicate sequence metadata cannot establish a preceding challenge', () => {
  assert.throws(() => audit([start(), packet(2, 'received', probe()),
    packet(2, 'sent', probe(0x82))], 1));
});
