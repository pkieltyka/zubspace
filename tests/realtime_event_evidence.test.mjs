import assert from "node:assert/strict";
import test from "node:test";

import { sameDecoyEntity, parseRealtimeWeaponReceipt, parseRealtimeWeaponEvent, parseRealtimeOwnerWeaponCancellation, parseReliableOwnerWeaponCancellation } from "../tools/realtime_event_evidence.mjs";

test("Decoy identity survives delivery refresh but distinguishes a new deployment", () => {
  const original = { playerId: 0, sessionId: "18446744073709551614", lifeSeq: 7,
    serverSessionId: "18446744073709551613", arenaStreamId: "2", shotGroupId: "18446744073709551612",
    serverTick: 123, eventId: 500, x: 100 };
  const recovered = { ...original, eventId: 600, x: 200 };
  assert.ok(sameDecoyEntity(original, recovered));
  assert.ok(!sameDecoyEntity(original, { ...recovered, shotGroupId: "18446744073709551611" }));
});

test("Decoy identity fails closed for incomplete or different source scope", () => {
  const source = { playerId: 0, sessionId: "11", lifeSeq: 7,
    serverSessionId: "22", arenaStreamId: "33", shotGroupId: "44", serverTick: 123 };
  assert.ok(!sameDecoyEntity(null, source));
  for (const field of Object.keys(source)) {
    assert.ok(!sameDecoyEntity(source, { ...source, [field]: null }));
    assert.ok(!sameDecoyEntity(source, { ...source, [field]: typeof source[field] === "string" ? "99" : 99 }));
  }
});

function weaponPacket(version) {
  const v2 = version === 2;
  const packet = Buffer.alloc(v2 ? 74 : 66);
  packet[0] = 0x7d;
  packet[1] = version;
  packet[2] = 1;
  packet.writeBigUInt64LE(11n, 3);
  packet.writeBigUInt64LE(12n, 11);
  packet.writeUInt32LE(13, 19);
  packet.writeUInt32LE(14, 23);
  packet.writeUInt16LE(15, 27);
  packet.writeBigUInt64LE(16n, 29);
  packet.writeUInt32LE(17, 37);
  if (v2) packet.writeBigUInt64LE(18n, 41);
  const positionOffset = v2 ? 49 : 41;
  packet.writeFloatLE(100.25, positionOffset);
  packet.writeFloatLE(200.5, positionOffset + 4);
  packet.writeFloatLE(-30.75, positionOffset + 8);
  packet.writeFloatLE(40.125, positionOffset + 12);
  packet.writeFloatLE(0.625, positionOffset + 16);
  packet.writeUInt16LE(19, positionOffset + 20);
  packet[positionOffset + 22] = 3;
  packet.writeUInt16LE(2 | (2 << 5), positionOffset + 23);
  return packet;
}

test("current realtime weapon evidence decodes v2 shot-group identity", () => {
  assert.deepEqual(parseRealtimeWeaponEvent(weaponPacket(2)), {
    version: 2,
    serverSessionId: "11",
    arenaStreamId: "12",
    eventId: 13,
    serverTick: 14,
    source: 15,
    sourceSessionId: "16",
    sourceLifeSeq: 17,
    shotGroupId: "18",
    x: 100.25,
    y: 200.5,
    velocityX: -30.75,
    velocityY: 40.125,
    orientation: 0.625,
    frequency: 19,
    ship: 3,
    weapon: 66,
  });
});

test("historical v1 evidence stays readable without weakening v2 framing", () => {
  assert.equal(parseRealtimeWeaponEvent(weaponPacket(1))?.shotGroupId, null);
  assert.equal(parseRealtimeWeaponEvent(weaponPacket(2).subarray(0, 66)), null);

  const zeroGroup = weaponPacket(2);
  zeroGroup.writeBigUInt64LE(0n, 41);
  assert.equal(parseRealtimeWeaponEvent(zeroGroup), null);

  const nonFinite = weaponPacket(2);
  nonFinite.writeUInt32LE(0x7fc00000, 49);
  assert.equal(parseRealtimeWeaponEvent(nonFinite), null);
});


function cancellationPacket() {
  const b = Buffer.alloc(52); b[0] = 0x7d; b[1] = 2; b[2] = 5;
  b.writeBigUInt64LE(77n, 3); b.writeBigUInt64LE(88n, 11); b.writeUInt32LE(9, 19);
  b.writeUInt16LE(12, 27); b.writeBigUInt64LE(18446744073709551615n, 29);
  b.writeBigUInt64LE(18446744073709551615n, 37); b.writeUInt32LE(1, 45); return b;
}
test("silent owner cutoff preserves u64 identities and distinguishes absent from zero input", () => {
  const b = cancellationPacket(), absent = parseRealtimeOwnerWeaponCancellation(b);
  assert.equal(absent.sourceSessionId, "18446744073709551615");
  assert.equal(absent.throughShotGroupId, "18446744073709551615");
  assert.equal(absent.ownerInputSequence, null);
  b[49] = 1; assert.equal(parseRealtimeOwnerWeaponCancellation(b).ownerInputSequence, 0);
  b.writeUInt16LE(65535, 50); assert.equal(parseRealtimeOwnerWeaponCancellation(b).ownerInputSequence, 65535);
});
test("owner cutoff parser rejects malformed framing, absent sequences and zero scope", () => {
  for (const changed of [b => b.subarray(0, 51), b => Buffer.concat([b, Buffer.from([0])]), b => { b[1] = 1; return b; }, b => { b[49] = 2; return b; }, b => { b[50] = 1; return b; }, b => { b.fill(0, 3, 11); return b; }, b => { b.fill(0, 11, 19); return b; }, b => { b.fill(0, 29, 37); return b; }, b => { b.fill(0, 45, 49); return b; }]) {
    assert.equal(parseRealtimeOwnerWeaponCancellation(changed(cancellationPacket())), null);
  }
});

test("reliable departure cutoff preserves exact frame sequence and rejects guessed offsets", () => {
  const header=Buffer.from([0,3,255,255,255,255]);
  const frame=Buffer.concat([header,cancellationPacket()]);
  assert.deepEqual(parseReliableOwnerWeaponCancellation(frame),{
    ...parseRealtimeOwnerWeaponCancellation(cancellationPacket()),reliableId:4294967295,
  });
  for (const invalid of [frame.subarray(1),frame.subarray(0,57),Buffer.concat([frame,Buffer.from([0])]),
    Buffer.concat([Buffer.from([0,14,0,0,0,0]),cancellationPacket()]),
    Buffer.concat([Buffer.from([1,3,0,0,0,0]),cancellationPacket()])]) {
    assert.equal(parseReliableOwnerWeaponCancellation(invalid),null);
  }
  const malformed=Buffer.from(frame);malformed[55]=2;
  assert.equal(parseReliableOwnerWeaponCancellation(malformed),null);
});


test("recovery receipt preserves immutable source independently of capture and delivery IDs", () => {
  const source=weaponPacket(2), recovery=Buffer.alloc(122);
  source.copy(recovery); recovery[2]=4;
  recovery.writeUInt32LE(99,19); recovery.writeUInt32LE(200,23);
  recovery.writeUInt32LE(7,74); recovery.writeUInt32LE(14,78); recovery[105]=1;
  const original=parseRealtimeWeaponReceipt(source), state=parseRealtimeWeaponReceipt(recovery);
  assert.equal(original.kind,1); assert.equal(state.kind,4);
  assert.equal(state.eventId,99); assert.equal(state.serverTick,200);
  assert.equal(state.sourceServerTick,14);
  for(const key of ["source","sourceSessionId","sourceLifeSeq","shotGroupId","serverSessionId","arenaStreamId"])
    assert.equal(state[key],original[key]);
  const withInput=Buffer.concat([recovery,Buffer.from([0,0])]); withInput[105]=9;
  assert.equal(parseRealtimeWeaponReceipt(withInput).projectileId,7);
});

test("recovery receipt rejects terminal, malformed framing, flags, identity and nonfinite data", () => {
  const packet=Buffer.alloc(122); weaponPacket(2).copy(packet); packet[2]=4; packet[105]=1;
  for(const mutate of [b=>b.subarray(0,121), b=>Buffer.concat([b,Buffer.from([0])]),
    b=>{b[105]=5;return b;},b=>{b[105]=17;return b;},b=>{b[105]=9;return b;},
    b=>{b.fill(0,41,49);return b;}, b=>{b.writeUInt32LE(0x7fc00000,85);return b;}])
    assert.equal(parseRealtimeWeaponReceipt(mutate(Buffer.from(packet))),null);
});
