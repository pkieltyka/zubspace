import assert from "node:assert/strict";

// Server movement witnesses expose the detector used by acquisition and the
// actual navigation target separately. Legitimate safety/prize navigation can
// temporarily override pursuit; it must never grant a hidden player target.
export function botDetectorTargetEvidence(traces, { sinceTick, statusByHuman,
  expectedBots = 2, requireSelectedActive = true, minSamples = 5, minSpan = 100 }) {
  const groups = new Map();
  for (const row of traces.filter(t => t.server_tick >= sinceTick)) {
    if (!statusByHuman.has(row.assigned_human_player_id)) continue;
    if (!groups.has(row.bot_id)) groups.set(row.bot_id, []);
    groups.get(row.bot_id).push(row);
  }
  assert.ok(groups.size <= expectedBots, "unexpected bot authority witnesses");
  if (groups.size < expectedBots) return null;
  const bots = [];
  for (const [id, samples] of groups) {
    samples.sort((a, b) => a.server_tick - b.server_tick);
    const first = samples[0], last = samples.at(-1);
    const pursuit = [];
    for (const row of samples) {
      assert.equal(row.life_seq, first.life_seq, "bot life changed during a protected observation");
      assert.equal(typeof row.bot_xradar_selected, "boolean", "missing assigned detector evidence");
      assert.equal(typeof row.bot_xradar_active, "boolean", "missing active detector evidence");
      assert.equal(row.bot_xradar_capable, true, "detector capability must remain owned");
      assert.equal(row.bot_xradar_selected, first.bot_xradar_selected, "detector assignment changed");
      assert.equal(row.assigned_human_player_id, first.assigned_human_player_id, "bot changed its human slot");
      if (requireSelectedActive || !row.bot_xradar_selected) {
        assert.equal(row.bot_xradar_active, row.bot_xradar_selected, "active detector differs from its assignment");
      }
      const status = statusByHuman.get(row.assigned_human_player_id) & 3;
      assert.equal(row.assigned_human_status & 3, status, "bot observed a different submitted status");
      const detects = status !== 3 || row.bot_xradar_active;
      assert.equal(row.detected_human_player_id, detects ? row.assigned_human_player_id : 65535,
        "acquisition must use this bot's own active detector");
      if (statusByHuman.has(row.target_player_id)) {
        assert.equal(row.target_player_id, row.assigned_human_player_id,
          "concealment redirected a bot onto another human's slot");
      }
      if (!detects) assert.notEqual(row.target_player_id, row.assigned_human_player_id,
        "an undetected player became the actual navigation/aiming target");
      else if (row.target_player_id === row.assigned_human_player_id) pursuit.push(row);
    }
    if (samples.length < minSamples || last.server_tick - first.server_tick < minSpan) return null;
    if (samples.some(r => (r.assigned_human_status & 3) !== 3 || r.bot_xradar_active) &&
        (pursuit.length < minSamples || pursuit.at(-1).server_tick - pursuit[0].server_tick < minSpan)) return null;
    bots.push({ id, life: first.life_seq, selected: first.bot_xradar_selected,
      activeAtEnd: last.bot_xradar_active, assignedHuman: first.assigned_human_player_id,
      samples: samples.length, pursuitSamples: pursuit.length, firstTick: first.server_tick, lastTick: last.server_tick });
  }
  assert.equal(bots.filter(b => b.selected).length, Math.ceil(expectedBots / 2), "detector split must be balanced");
  assert.equal(new Set(bots.map(b => b.assignedHuman)).size, expectedBots, "bots must retain separate human assignments");
  return { bots };
}
