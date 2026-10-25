const REALTIME_EVENT_TYPE = 0x7d;
const REALTIME_WEAPON_KIND = 1;
const REALTIME_WEAPON_V1_LEN = 66;
const REALTIME_WEAPON_V2_LEN = 74;

export function sameDecoyEntity(left, right) {
  // Recovery changes delivery IDs, never authenticated owner/life, arena,
  // shot group or birth tick. Keep u64 identities as exact decimal strings.
  const fields = ["playerId", "sessionId", "lifeSeq", "serverSessionId", "arenaStreamId", "shotGroupId", "serverTick"];
  return left != null && right != null && fields.every(field =>
    left[field] != null && left[field] === right[field]);
}

function validWeaponFact(fact) {
  const weaponType = fact.weapon & 0x1f;
  return fact.serverSessionId !== "0" &&
    fact.arenaStreamId !== "0" &&
    fact.eventId !== 0 &&
    fact.sourceSessionId !== "0" &&
    fact.sourceLifeSeq !== 0 &&
    (fact.version === 1 || fact.shotGroupId !== "0") &&
    weaponType >= 1 &&
    weaponType <= 8 &&
    fact.ship < 8 &&
    [fact.x, fact.y, fact.velocityX, fact.velocityY, fact.orientation]
      .every(Number.isFinite) &&
    fact.orientation >= 0 &&
    fact.orientation <= 1;
}

/**
 * Decode the immutable weapon subset of an authenticated 0x7d event.
 *
 * Version 1 is retained only for historical evidence fixtures. Current
 * production traffic is version 2 and includes the stable shot-group id.
 */
export function parseRealtimeWeaponEvent(packet) {
  if (
    packet?.[0] !== REALTIME_EVENT_TYPE ||
    packet?.[2] !== REALTIME_WEAPON_KIND ||
    typeof packet.readUInt16LE !== "function"
  ) {
    return null;
  }
  const version = packet[1];
  if (
    (version === 1 && packet.length !== REALTIME_WEAPON_V1_LEN) ||
    (version === 2 && packet.length !== REALTIME_WEAPON_V2_LEN) ||
    (version !== 1 && version !== 2)
  ) {
    return null;
  }

  return parseWeaponSourceFields(packet, version);
}

function parseWeaponSourceFields(packet, version) {
  const v2 = version === 2;
  const positionOffset = v2 ? 49 : 41;
  const fact = {
    version,
    serverSessionId: packet.readBigUInt64LE(3).toString(),
    arenaStreamId: packet.readBigUInt64LE(11).toString(),
    eventId: packet.readUInt32LE(19),
    serverTick: packet.readUInt32LE(23),
    source: packet.readUInt16LE(27),
    sourceSessionId: packet.readBigUInt64LE(29).toString(),
    sourceLifeSeq: packet.readUInt32LE(37),
    shotGroupId: v2 ? packet.readBigUInt64LE(41).toString() : null,
    x: packet.readFloatLE(positionOffset),
    y: packet.readFloatLE(positionOffset + 4),
    velocityX: packet.readFloatLE(positionOffset + 8),
    velocityY: packet.readFloatLE(positionOffset + 12),
    orientation: packet.readFloatLE(positionOffset + 16),
    frequency: packet.readUInt16LE(positionOffset + 20),
    ship: packet[positionOffset + 22],
    weapon: packet.readUInt16LE(positionOffset + 23),
  };
  return validWeaponFact(fact) ? fact : null;
}

// Wire-receipt evidence shares the immutable source prefix across original
// carriers and recovery captures. It does not infer receipt from live sprites.
export function parseRealtimeWeaponReceipt(packet) {
  const original = parseRealtimeWeaponEvent(packet);
  if (original) return { ...original, kind: 1, sourceServerTick: original.serverTick };
  if (!Buffer.isBuffer(packet) || packet[0] !== 0x7d || packet[1] !== 2 ||
      packet[2] !== 4 || packet.length < 122) return null;
  const flags = packet[105];
  if ((flags & ~15) !== 0 || (flags & 4) !== 0 ||
      packet.length !== 122 + ((flags & 8) !== 0 ? 2 : 0)) return null;
  const source = parseWeaponSourceFields(packet, 2);
  if (!source || ![85,89,93,97].every(offset => Number.isFinite(packet.readFloatLE(offset)))) return null;
  return { ...source, kind: 4, sourceServerTick: packet.readUInt32LE(78),
    projectileId: packet.readUInt32LE(74), childIndex: packet[82] };
}

// Exact kind-5 wire contract. Evidence must not infer a cancellation from a
// missing sprite or confuse a different source session/arena with this one.
export function parseRealtimeOwnerWeaponCancellation(packet) {
  if (!Buffer.isBuffer(packet) || packet.length !== 52 || packet[0] !== 0x7d || packet[1] !== 2 || packet[2] !== 5 || (packet[49] & ~1) !== 0 || (!(packet[49] & 1) && packet.readUInt16LE(50) !== 0)) return null;
  const fact = {
    serverSessionId: packet.readBigUInt64LE(3).toString(),
    arenaStreamId: packet.readBigUInt64LE(11).toString(),
    eventId: packet.readUInt32LE(19), serverTick: packet.readUInt32LE(23),
    source: packet.readUInt16LE(27), sourceSessionId: packet.readBigUInt64LE(29).toString(),
    throughShotGroupId: packet.readBigUInt64LE(37).toString(), ownerLifeSeq: packet.readUInt32LE(45),
    ownerInputSequence: packet[49] & 1 ? packet.readUInt16LE(50) : null,
  };
  return fact.serverSessionId !== "0" && fact.arenaStreamId !== "0" && fact.sourceSessionId !== "0" && fact.eventId !== 0 && fact.ownerLifeSeq !== 0 ? fact : null;
}

// Departures use one small ordered reliable frame, never chunks or a guessed
// payload offset. Preserve its sequence so acceptance can prove cut-before-left.
export function parseReliableOwnerWeaponCancellation(packet) {
  if (!Buffer.isBuffer(packet) || packet.length !== 58 || packet[0] !== 0 || packet[1] !== 3) return null;
  const fact = parseRealtimeOwnerWeaponCancellation(packet.subarray(6));
  return fact ? { ...fact, reliableId: packet.readUInt32LE(2) } : null;
}
