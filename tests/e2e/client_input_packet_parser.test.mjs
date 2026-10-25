import assert from "node:assert/strict";
import test from "node:test";

import {
  clientPositionWarpMarker,
  inputPulseSequences,
  inputTailSequence,
} from "./combat_replay.mjs";

const INPUT_HEADER_LEN = 4;
const INPUT_ENTRY_LEN = 18;

function writeLeU32(bytes, offset, value) {
  bytes[offset] = value & 0xff;
  bytes[offset + 1] = (value >>> 8) & 0xff;
  bytes[offset + 2] = (value >>> 16) & 0xff;
  bytes[offset + 3] = (value >>> 24) & 0xff;
}

function inputPacket(baseSequence, inputBits) {
  const bytes = new Array(INPUT_HEADER_LEN + inputBits.length * INPUT_ENTRY_LEN).fill(0);
  bytes[0] = 0x62;
  bytes[1] = baseSequence & 0xff;
  bytes[2] = (baseSequence >>> 8) & 0xff;
  bytes[3] = inputBits.length;
  for (let index = 0; index < inputBits.length; index += 1) {
    const offset = INPUT_HEADER_LEN + index * INPUT_ENTRY_LEN;
    bytes[offset] = inputBits[index] & 0xff;
    bytes[offset + 1] = (inputBits[index] >>> 8) & 0xff;
  }
  return bytes;
}

test("rolling input evidence uses the full receipt-bearing entry stride", () => {
  const packet = inputPacket(0xfffe, [0, 1 << 5, 0]);

  assert.deepEqual(inputPulseSequences(packet), [0xffff]);
  assert.equal(inputTailSequence(packet), 0);

  const truncated = packet.slice(0, -1);
  assert.deepEqual(inputPulseSequences(truncated), []);
  assert.equal(inputTailSequence(truncated), null);
});

test("Warp evidence follows the complete embedded rolling input record", () => {
  const inputs = inputPacket(41, [0, 1 << 5]);
  const markerOffset = 55 + inputs.length;
  const packet = new Array(markerOffset + 5).fill(0);
  packet[0] = 0x03;
  writeLeU32(packet, 28, 0x5a5a1234);
  packet.splice(55, inputs.length, ...inputs);
  packet[markerOffset] = 0x76;
  writeLeU32(packet, markerOffset + 1, 0x10203040);

  assert.deepEqual(clientPositionWarpMarker(packet), {
    collectibleId: 0x10203040,
    positionSequence: 0x1234,
  });
  assert.equal(clientPositionWarpMarker(packet.slice(0, -1)), null);
});
