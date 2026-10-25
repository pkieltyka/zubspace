import assert from "node:assert/strict";

// Native shot logs only choose when to request a real joined trace. They
// cannot supply the exact projectile/debit/recharge verdict below.
export function botEnergySustainedCaptureHint(shots, serverTick, retainedWindowTicks) {
  if (!Number.isInteger(serverTick) || serverTick < 0 || serverTick > 0xffff_ffff ||
      !Number.isFinite(retainedWindowTicks) || retainedWindowTicks < 300) return null;
  const lives = new Map();
  for (const shot of shots) {
    if (!Number.isInteger(shot.server_tick) || shot.server_tick < 0 || shot.server_tick > serverTick ||
        shot.server_tick <= serverTick - retainedWindowTicks ||
        !Number.isInteger(shot.owner_id) || shot.owner_id < 0 || shot.owner_id > 65535 ||
        !Number.isInteger(shot.owner_life_seq) || shot.owner_life_seq <= 0 || shot.owner_life_seq > 0xffff_ffff ||
        typeof shot.owner_session_id !== "string" || typeof shot.shot_group_id !== "string" ||
        !/^[1-9]\d*$/.test(shot.owner_session_id) || !/^[1-9]\d*$/.test(shot.shot_group_id)) continue;
    const identity = `${shot.owner_id}/${shot.owner_session_id}/${shot.owner_life_seq}`;
    if (!lives.has(identity)) lives.set(identity, new Map());
    lives.get(identity).set(shot.shot_group_id, shot);
  }
  for (const [identity, groups] of lives) {
    const events = [...groups.values()].sort((a, b) => a.server_tick - b.server_tick);
    const first = events[0], last = events.at(-1);
    if (events.length >= 8 && serverTick - last.server_tick <= 100 &&
        last.server_tick - first.server_tick >= 300)
      return { identity, firstTick: first.server_tick, lastTick: last.server_tick,
        shots: events.length, retainedWindowTicks };
  }
  return null;
}

export function botEnergyCaptureQuotaAt(capturedAt, now, reservedSlots = 0) {
  assert.ok(Number.isInteger(reservedSlots) && reservedSlots >= 0 && reservedSlots < 4);
  const available = 4 - reservedSlots;
  return Math.max(now, capturedAt.length < available ? 0 : capturedAt.at(-available) + 61_000);
}

// Sustained evidence can span several overlapping rings. Reserve one slot
// for a native bomb hint; the other three must remain available to build that
// continuous history rather than waiting for eight shots to fit a single ring.
export function botEnergyCapturePolicy({ capturedAt, now, nextTimerAt, observedBomb,
  freshBombHint = false, sustainedHint = false }) {
  const reserve = Number(!observedBomb);
  return {
    reserve,
    timerDue: now >= nextTimerAt && botEnergyCaptureQuotaAt(capturedAt, now, reserve) <= now,
    hintDue: Boolean(freshBombHint || sustainedHint) &&
      botEnergyCaptureQuotaAt(capturedAt, now, Number(!observedBomb && !freshBombHint)) <= now,
  };
}

export function nextBotEnergyCaptureAt(capturedAt, now, retainedWindowTicks = 0, reservedSlots = 0) {
  // The quota permits four uploads per minute. Spending them a second apart
  // repeatedly observes almost the same few seconds, then leaves a minute
  // unseen. Use the actual ring duration while reserving 35% for overlap and
  // variable capture cost. Coverage is still verified from each real window.
  assert.ok(Number.isFinite(retainedWindowTicks) && retainedWindowTicks >= 0);
  const spacing = Math.max(1000, Math.min(3500, retainedWindowTicks * 10 * 0.65));
  return botEnergyCaptureQuotaAt(capturedAt, now + spacing, reservedSlots);
}

// Cold-checkpoint QA evidence, not a gameplay driver. Incomplete observation
// returns null; contradictory accounting must fail rather than wait it away.
export function botWeaponEnergyEvidence(events, profiles, { minimumShots = 8, minimumTicks = 300, coverageWindows = null, requireSustained = true } = {}) {
  // A shot interval may cross a capture boundary while remaining fully
  // observed by the union of overlapping rings. Never bridge a real gap.
  const coverage = [];
  for (const [first, last] of [...(coverageWindows || [])].sort((a, b) => a[0] - b[0])) {
    assert.ok(Number.isInteger(first) && Number.isInteger(last) && first <= last);
    const previous = coverage.at(-1);
    if (previous && first <= previous[1]) previous[1] = Math.max(previous[1], last);
    else coverage.push([first, last]);
  }
  const groups = new Map(), lives = new Map(), perShip = new Map();
  for (const event of events) {
    if (event.type !== "server_projectile_spawned" || !profiles.has(event.ownerId)) continue;
    const key = `${event.ownerId}/${event.ownerSessionId}/${event.ownerLifeSeq}/${event.shotGroupId}`;
    const group = groups.get(key) || [];
    group.push(event); groups.set(key, group);
  }
  const commits = events.filter(e => e.type === "server_weapon_event_committed" && profiles.has(e.ownerId));
  const seen = new Set();
  let gunActivations = 0, bombActivations = 0;
  for (const event of commits) {
    const p = profiles.get(event.ownerId);
    assert.equal(event.ship, p.ship, "energy debit must use the assigned ship");
    assert.equal(event.frequency, event.ownerId, "bot must retain its unique FFA frequency");
    assert.match(event.ownerSessionId, /^[1-9]\d*$/, "exact source session is required");
    assert.match(event.shotGroupId, /^[1-9]\d*$/, "exact shot group is required");
    const key = `${event.ownerId}/${event.ownerSessionId}/${event.ownerLifeSeq}/${event.shotGroupId}`;
    assert.ok(!seen.has(key), "one debit per activation"); seen.add(key);
    const type = event.weapon & 31;
    assert.ok([1, 2, 3, 4].includes(type), "autonomous ordinary weapons are guns or bombs");
    const bomb = [3, 4].includes(type);
    if (bomb) bombActivations++; else gunActivations++;
    assert.equal(event.weapon & 0x8000, 0, "bots must keep multifire and dropped mines off");
    const level = ((event.weapon >> 5) & 3) + 1;
    assert.equal(level, bomb ? p.maxBombs : p.maxGuns, "ordinary bot weapons remain maxed");
    assert.equal(typeof event.superActive, "boolean");
    const required = bomb ? p.bombFireEnergy + p.bombFireEnergyUpgrade * (level - 1) : p.bulletFireEnergy * level;
    assert.ok(Number.isFinite(required) && required > 0, "known authored weapon cost is required");
    const cost = event.superActive ? 0 : required;
    assert.equal(event.energyCost, cost, "debit must equal the authored cost, or zero during Super");
    for (const field of ["energyBefore", "energyAfter", "serverTick", "ownerLifeSeq"]) {
      assert.ok(Number.isInteger(event[field]) && event[field] >= 0, `invalid ${field}`);
    }
    assert.ok(Number.isInteger(event.maxEnergy) && event.maxEnergy > 0 && event.maxEnergy <= 65535,
      "exact energy capacity is required");
    for (const field of ["energyRemainderBefore", "energyRemainderAfter"]) {
      const value = event[field];
      assert.ok(Number.isFinite(value) && value >= 0 && value < 1 && Math.fround(value) === value,
        `invalid exact f32 ${field}`);
    }
    // EnergyPool combines the integer and its recharge fraction using f32.
    // A near-one fraction can round that sum up even though the separately
    // retained integer has not advanced. Prove the actual arithmetic exactly;
    // a +/-1 integer tolerance would conceal real free energy or extra debits.
    const remainder = event.energyBefore >= event.maxEnergy ? 0 : event.energyRemainderBefore;
    const available = Math.fround(event.energyBefore + remainder);
    assert.ok(available > required, "accepted shot requires strict authored funding, including Super");
    if (event.superActive) {
      assert.equal(event.energyAfter, event.energyBefore, "Super cannot debit whole energy");
      assert.equal(event.energyRemainderAfter, event.energyRemainderBefore, "Super cannot debit fractional energy");
    } else {
      const after = Math.max(0, Math.min(65535, Math.fround(available - cost)));
      const whole = Math.floor(after);
      const fraction = whole >= event.maxEnergy ? 0 : Math.fround(after - whole);
      assert.equal(event.energyAfter, whole, "accepted shot must debit exactly once using shared f32 arithmetic");
      assert.equal(event.energyRemainderAfter, fraction, "accepted shot must preserve the exact post-debit fraction");
    }
    const projectiles = groups.get(key) || [];
    const doubleBarrel = !bomb && p.doubleBarrel;
    assert.equal(projectiles.length, doubleBarrel ? 2 : 1, "one activation must publish the authored barrel count");
    assert.equal(new Set(projectiles.map(e => e.projectileId)).size, projectiles.length);
    assert.deepEqual(projectiles.map(e => e.shotIndex).sort(), doubleBarrel ? [0, 1] : [0]);
    for (const projectile of projectiles) {
      assert.equal(projectile.weapon, event.weapon);
      assert.equal(projectile.serverTick, event.serverTick);
    }
    const lifeKey = `${event.ownerId}/${event.ownerSessionId}/${event.ownerLifeSeq}`;
    const life = lives.get(lifeKey) || [];
    life.push(event); lives.set(lifeKey, life);
    perShip.set(p.ship, (perShip.get(p.ship) || 0) + 1);
  }
  const sustained = [], bulletBombSequences = [];
  for (const [identity, shots] of lives) {
    shots.sort((a, b) => a.serverTick - b.serverTick);
    const p = profiles.get(shots[0].ownerId);
    const gaps = [];
    let runStart = 0, longestRun = 1, longestRunTicks = 0, unobservedIntervals = 0;
    for (let i = 1; i < shots.length; i++) {
      const ticks = shots[i].serverTick - shots[i - 1].serverTick;
      const precedingBomb = [3, 4].includes(shots[i - 1].weapon & 31);
      assert.ok(ticks >= (precedingBomb ? p.bombFireDelay : p.bulletFireDelay) * 2,
        "actual fire cadence cannot bypass the preceding weapon cooldown");
      if (i >= 2 && [3, 4].includes(shots[i].weapon & 31) &&
          shots.slice(i - 2, i).every(shot => [1, 2].includes(shot.weapon & 31))) {
        bulletBombSequences.push({ identity, ship: p.ship,
          shotGroups: shots.slice(i - 2, i + 1).map(shot => shot.shotGroupId) });
      }
      const covered = coverageWindows == null || coverage.some(([first, last]) =>
        first < shots[i - 1].serverTick && last >= shots[i].serverTick);
      if (!covered) { unobservedIntervals++; runStart = i; }
      // Incoming damage/status drains can only lower energy. Account for real
      // prize refills so natural greens cannot become a false free-shot claim.
      const prizeGain = events.filter(e => e.type === "server_bot_prize_committed"
        && e.ownerId === shots[i].ownerId && e.ownerSessionId === shots[i].ownerSessionId
        && e.ownerLifeSeq === shots[i].ownerLifeSeq && e.serverTick > shots[i - 1].serverTick
        && e.serverTick <= shots[i].serverTick)
        .reduce((sum, e) => sum + Math.max(0, e.energyAfter - e.energyBefore), 0);
      assert.ok(!covered || shots[i].energyBefore <= shots[i - 1].energyAfter + p.rechargePerSecond * ticks / 100 + prizeGain + 1,
        "between-shot gain exceeds authored recharge within the same life");
      if (i - runStart + 1 >= longestRun) {
        longestRun = i - runStart + 1;
        longestRunTicks = shots[i].serverTick - shots[runStart].serverTick;
      }
      gaps.push(ticks);
    }
    const elapsedTicks = shots.at(-1).serverTick - shots[0].serverTick;
    if (longestRun >= minimumShots && longestRunTicks >= minimumTicks) sustained.push({ identity,
      ship: p.ship, shots: shots.length, elapsedTicks, minimumGap: Math.min(...gaps),
      longestCoveredRun: longestRun, longestCoveredRunTicks: longestRunTicks, unobservedIntervals,
      energyBefore: shots[0].energyBefore, energyAfter: shots.at(-1).energyAfter,
      energySpent: shots.reduce((sum, e) => sum + e.energyCost, 0),
      maximumRecharge: p.rechargePerSecond * elapsedTicks / 100 });
  }
  const requiredShips = new Set([...profiles.values()].map(p => p.ship));
  if (![...requiredShips].every(ship => perShip.has(ship)) || (requireSustained && sustained.length === 0)) return null;
  return { acceptedActivations: commits.length, gunActivations, bombActivations, bulletBombSequences,
    activationsByShip: [...perShip], sustained, sustainedRequired: requireSustained,
    scope: "observed autonomous guns/bombs with exact debit, barrels and preceding-weapon cooldown; rejection and Super boundaries have separate native coverage" };
}
