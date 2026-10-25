// Server-admitted movement only. This cannot establish browser rendering,
// DataChannel delivery, resource cost, or capacity by itself.
export function createAdmittedRecordReader({ maximumRecords = 200000 } = {}) {
  if (!Number.isSafeInteger(maximumRecords) || maximumRecords < 1) throw new Error("invalid record bound");
  let cursor = 0, pending = "";
  const records = [];
  const stats = { chunks: 0, characters: 0, scans: 0 };
  return {
    stats,
    read(chunks) {
      if (chunks.length < cursor) throw new Error("server log was truncated during measurement");
      stats.scans++;
      while (cursor < chunks.length) {
        const chunk = chunks[cursor++];
        stats.chunks++; stats.characters += chunk.length;
        const lines = (pending + chunk).split("\n");
        pending = lines.pop();
        if (pending.length > 1024 * 1024) throw new Error("unterminated server log line exceeds capture bound");
        for (const line of lines) {
          if (!line.includes("human authority input trace")) continue;
          if (records.length >= maximumRecords) throw new Error("admitted record capture capacity exceeded");
          records.push(Object.fromEntries([...line.matchAll(/(\w+)=(-?[\d.]+)/g)]
            .map((match) => [match[1], Number(match[2])])));
        }
      }
      return records;
    },
  };
}

export function movingPeerWindowEvidence({
  records, playerIds, startTick, endTick, maximumGapTicks, minimumDistance,
}) {
  const modulus = 0x80000000;
  const delta = (tick) => (tick - startTick + modulus) % modulus;
  if (!Array.isArray(records) || !Array.isArray(playerIds) || !playerIds.length ||
      new Set(playerIds).size !== playerIds.length ||
      !playerIds.every((id) => Number.isInteger(id) && id >= 0 && id <= 65535) ||
      ![startTick, endTick].every((tick) => Number.isInteger(tick) && tick >= 0 && tick < modulus) ||
      !Number.isInteger(maximumGapTicks) || maximumGapTicks <= 0 ||
      !Number.isFinite(minimumDistance) || minimumDistance <= 0) {
    throw new Error("invalid moving-peer measurement contract");
  }
  const duration = delta(endTick);
  if (duration <= maximumGapTicks || duration >= modulus / 2) {
    throw new Error("moving-peer window must be bounded and longer than its gap limit");
  }
  const peers = playerIds.map((playerId) => {
    const samples = records.filter((r) => r.player_id === playerId &&
      Number.isInteger(r.server_tick) && r.server_tick >= 0 && r.server_tick < modulus &&
      delta(r.server_tick) <= duration);
    const failures = [];
    let previousTick = 0;
    let previous = null;
    let distance = 0;
    let maximumGap = 0;
    const lives = new Set();
    for (const sample of samples) {
      const tick = delta(sample.server_tick);
      lives.add(sample.life_seq);
      if (!Number.isInteger(sample.life_seq) || sample.life_seq <= 0 ||
          !Number.isInteger(sample.input_bits) || (sample.input_bits & 1) === 0 ||
          !Number.isFinite(sample.x) || !Number.isFinite(sample.y)) {
        failures.push("invalid-or-not-thrusting");
      }
      if (tick < previousTick) failures.push("nonmonotonic-admission");
      maximumGap = Math.max(maximumGap, tick - previousTick);
      if (previous) distance += Math.hypot(sample.x - previous.x, sample.y - previous.y);
      previousTick = tick;
      previous = sample;
    }
    maximumGap = Math.max(maximumGap, duration - previousTick);
    if (!samples.length) failures.push("missing-peer");
    if (lives.size !== 1) failures.push("not-one-exact-life");
    if (maximumGap > maximumGapTicks) failures.push("admission-gap");
    if (!Number.isFinite(distance) || distance < minimumDistance) failures.push("insufficient-motion");
    return { playerId, samples: samples.length, lives: [...lives], distance, maximumGap,
      failures: [...new Set(failures)] };
  });
  return { ok: peers.every((peer) => !peer.failures.length), startTick, endTick,
    duration, maximumGapTicks, minimumDistance, peers };
}
