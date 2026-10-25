// ConnectionQualityProbe framing: src/protocol/connection_quality.rs.
// Only a byte-exact response to a preceding received challenge is control
// traffic. Every other sent game-lane packet remains forbidden in this gate.
export function auditBaselineWaitGamePackets(entries, startSequence) {
  if (!Number.isSafeInteger(startSequence) || startSequence < 0) {
    throw new Error("baseline packet audit requires an exact starting sequence");
  }
  const ordered = [...entries].sort((a, b) => a.seq - b.seq);
  const isGame = entry => entry.detail?.channel === "zubspace-game";
  const valid = (bytes, type) => Array.isArray(bytes) && bytes.length === 30 &&
    bytes.every(byte => Number.isInteger(byte) && byte >= 0 && byte <= 255) &&
    bytes[0] === type && bytes[1] === 1 &&
    bytes.slice(2, 10).some(Boolean) && bytes.slice(10, 18).some(Boolean) &&
    bytes.slice(22, 30).some(Boolean);
  const challenges = new Map();
  const rejectedPackets = [];
  let totalGameChannelPacketsDuringWait = 0;
  let qualityEchoPacketsDuringWait = 0;
  let previousSequence = -1;
  for (const entry of ordered) {
    if (!Number.isSafeInteger(entry.seq) || entry.seq < 0 || entry.seq === previousSequence) {
      throw new Error("baseline packet audit encountered a missing/invalid sequence");
    }
    previousSequence = entry.seq;
    if (!isGame(entry)) continue;
    if (entry.direction === "received" && valid(entry.bytes, 0x81)) {
      const key = entry.bytes.slice(1).join(",");
      challenges.set(key, (challenges.get(key) || 0) + 1);
    }
    if (entry.direction !== "sent") continue;
    const key = valid(entry.bytes, 0x82) ? entry.bytes.slice(1).join(",") : null;
    const matched = key != null && (challenges.get(key) || 0) > 0;
    // Consume replies before the measured window too: a challenge answered
    // before startSequence cannot excuse a replay during the wait.
    if (matched) challenges.set(key, challenges.get(key) - 1);
    if (entry.seq <= startSequence) continue;
    totalGameChannelPacketsDuringWait += 1;
    if (matched) {
      qualityEchoPacketsDuringWait += 1;
    } else {
      rejectedPackets.push({ sequence: entry.seq, firstByte: entry.bytes?.[0] ?? null,
        bytes: entry.bytes?.length ?? null });
    }
  }
  return { startSequence, endSequence: ordered.at(-1)?.seq ?? startSequence,
    windowComplete: startSequence === 0 || ordered.some(entry => entry.seq === startSequence),
    gameplayPacketsDuringWait: rejectedPackets.length, qualityEchoPacketsDuringWait,
    totalGameChannelPacketsDuringWait, rejectedPackets };
}
