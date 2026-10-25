import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import { botEnergyCapturePolicy, botEnergyCaptureQuotaAt, botEnergySustainedCaptureHint, botWeaponEnergyEvidence, nextBotEnergyCaptureAt } from "./bot_weapon_energy_evidence.mjs";

const nativeBurst = () => Array.from({length: 8}, (_, index) => ({
  owner_id: 768, owner_session_id: "18446744073709551360", owner_life_seq: 2,
  shot_group_id: String(index + 1), server_tick: 100 + index * 48,
}));
test("native activity can request a trace while the complete sustained window still fits", () => {
  // Retained native run missed in the old coverage gap3770..6609:
  // the quota was already spent despite this real5.43s window fitting the ring.
  const captured = [[3828,128],[3876,131],[3993,136],[4041,138],
    [4186,144],[4235,147],[4323,150],[4371,153]];
  const shots = nativeBurst().map((shot, index) => ({...shot,
    server_tick: captured[index][0], shot_group_id: String(captured[index][1])}));
  const hint = botEnergySustainedCaptureHint(shots, 4371, 580);
  assert.equal(hint.identity, "768/18446744073709551360/2");
  assert.equal(hint.firstTick, 3828); assert.equal(hint.lastTick, 4371);
  assert.equal(hint.shots, 8);
});
test("a trace hint cannot combine duplicate, split-life, stale, future or short activity", () => {
  const shots = nativeBurst();
  const duplicates = [...shots.slice(0, 7), shots[0], shots[0]];
  const split = shots.map((shot, index) => ({...shot, owner_life_seq: index < 4 ? 1 : 2}));
  const short = shots.map((shot, index) => ({...shot, server_tick: 100 + index * 30}));
  assert.equal(botEnergySustainedCaptureHint(duplicates, 440, 580), null);
  assert.equal(botEnergySustainedCaptureHint(split, 440, 580), null);
  assert.equal(botEnergySustainedCaptureHint(shots, 600, 580), null);
  assert.equal(botEnergySustainedCaptureHint(shots, 435, 580), null);
  assert.equal(botEnergySustainedCaptureHint(short, 315, 580), null);
  assert.equal(botEnergySustainedCaptureHint(shots, 440, 300), null);
  assert.equal(botEnergySustainedCaptureHint(shots, 440, Infinity), null);
});
test("a sustained history wider than one ring gets three overlapping uploads while retaining a bomb slot", () => {
  // Actual failed capture completion times, relative to the first. Its
  // two-window union783..1673 missed the real eight-shot810..1943 interval.
  // A third overlapping capture must be eligible before the minute expires.
  const capturedAt = [0, 3606], now = 7106;
  const policy = (extra = {}) => botEnergyCapturePolicy({capturedAt,now,
    nextTimerAt:7106,observedBomb:false,...extra});
  assert.equal(policy().timerDue,true);
  assert.equal(policy().reserve,1);
  capturedAt.push(now);
  assert.equal(policy({now:10606}).timerDue,false);
  assert.equal(policy({now:28250,freshBombHint:true}).hintDue,true);
  assert.equal(policy({now:28250,sustainedHint:true}).hintDue,false,
    "a sustained hint cannot steal the reserved bomb slot");
  capturedAt.push(28250);
  assert.equal(policy({now:30000,freshBombHint:true}).hintDue,false);
  assert.equal(policy({now:61000,observedBomb:true}).timerDue,true);
});

const profiles = new Map([[768, { ship: 4, bulletFireEnergy: 20, maxGuns: 3,
  doubleBarrel: true, bulletFireDelay: 25, rechargePerSecond: 115,
  bombFireEnergy: 300, bombFireEnergyUpgrade: 50, maxBombs: 2, bombFireDelay: 150 }]]);
test("captured same-life energy run remains continuous across overlapping ring windows", () => {
  const captured = JSON.parse(fs.readFileSync(new URL("../fixtures/bot_energy_overlap_42b85eb.json", import.meta.url)));
  const result = botWeaponEnergyEvidence(captured.events, new Map(captured.profiles), {
    coverageWindows: captured.coverageWindows,
  });
  assert.ok(result, "the original overlapping captures cover every between-shot interval");
  assert.equal(result.sustained[0].longestCoveredRun, 9);
  assert.equal(result.sustained[0].longestCoveredRunTicks, 1049);
  assert.equal(result.bombActivations, 0, "missing bomb evidence remains missing");
});
test("overlapping windows can be composed but a real unobserved gap cannot", () => {
  assert.ok(botWeaponEnergyEvidence(fixture(), profiles, { coverageWindows: [[250, 500], [99, 275]] }));
  assert.equal(botWeaponEnergyEvidence(fixture(), profiles, { coverageWindows: [[99, 275], [276, 500]] }), null);
});
test("overlapping capture scheduling respects the sliding quota despite variable upload costs", () => {
  const times = [0];
  for (let i = 0; i < 20; i++) {
    const next = nextBotEnergyCaptureAt(times, times.at(-1) + (i < 4 ? 3500 : 100));
    assert.ok(times.filter(t => next - t < 60_000).length < 4);
    times.push(next);
  }
});
test("capture spacing uses the measured ring duration without spending the quota in one short burst", () => {
  assert.equal(nextBotEnergyCaptureAt([0], 1000, 500), 4250);
  assert.equal(nextBotEnergyCaptureAt([0], 1000, 900), 4500);
  assert.equal(nextBotEnergyCaptureAt([0], 1000, 100), 2000);
  assert.equal(nextBotEnergyCaptureAt([0, 1500, 4000, 7500], 8000, 500), 61000);
  const times = [0];
  for (let i = 0; i < 20; i++) {
    const next = nextBotEnergyCaptureAt(times, times.at(-1) + 100, 500);
    assert.ok(times.filter(t => next - t < 60_000).length < 4);
    times.push(next);
  }
  assert.ok(times[3] > 9000, "the initial four captures cover a longer contiguous interval");
});
test("timer samples reserve one quota slot for a real bomb without bypassing the fourth-upload limit", () => {
  const completed = [10_000, 13_500, 17_000];
  assert.equal(nextBotEnergyCaptureAt(completed, 20_000, 600, 1), 71_000);
  assert.equal(botEnergyCaptureQuotaAt(completed, 56_000), 56_000, "a bomb can use the reserved slot");
  completed.push(56_000);
  assert.equal(botEnergyCaptureQuotaAt(completed, 56_100), 71_000);
  assert.equal(nextBotEnergyCaptureAt(completed, 56_100, 600), 71_000);
  assert.equal(botEnergyCaptureQuotaAt(completed, 71_000), 71_000);
});
function fixture() {
  return Array.from({ length: 9 }, (_, i) => {
    const shot = { ownerId: 768, ownerSessionId: "42", ownerLifeSeq: 1,
      shotGroupId: String(i + 1), serverTick: 100 + i * 50, weapon: 65 };
    return [{ ...shot, type: "server_weapon_event_committed", ship: 4, frequency: 768,
      maxEnergy: 1700, energyRemainderBefore: 0, energyRemainderAfter: 0,
      energyBefore: 1700 - i * 3, energyAfter: 1640 - i * 3, energyCost: 60, superActive: false },
    ...[0, 1].map(shotIndex => ({ ...shot, type: "server_projectile_spawned", shotIndex, projectileId: i * 2 + shotIndex }))];
  }).flat();
}
function mixedFixture() {
  const events = fixture().slice(0, 9);
  const bomb = events[6];
  Object.assign(bomb, { weapon: 36, energyBefore: 1600, energyAfter: 1250, energyCost: 350 });
  Object.assign(events[7], { weapon: 36 });
  events.splice(8, 1); // Double-barrel applies to guns, never to bombs.
  const next = fixture().slice(9, 12);
  next.forEach(event => { event.serverTick = 500; });
  Object.assign(next[0], { energyBefore: 1500, energyAfter: 1440 });
  events.push(...next);
  return events;
}
test("mixed bot guns and bombs retain exact debit, one bomb projectile and the preceding weapon cooldown", () => {
  const result = botWeaponEnergyEvidence(mixedFixture(), profiles, { requireSustained: false });
  assert.equal(result.bombActivations, 1);
  assert.equal(result.gunActivations, 3);
  assert.equal(result.bulletBombSequences.length, 1);
  for (const mutate of [
    e => { e[6].energyAfter = e[6].energyBefore; },
    e => { e[6].energyCost = 300; },
    e => { e.push({ ...e[7], projectileId: 999, shotIndex: 1 }); },
    e => { e.slice(8).forEach(event => { event.serverTick = 250; }); },
  ]) {
    const events = mixedFixture(); mutate(events);
    assert.throws(() => botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  }
});
test("sustained double-barrel shots each cost one activation with bounded recharge", () => {
  const result = botWeaponEnergyEvidence(fixture(), profiles);
  assert.equal(result.acceptedActivations, 9);
  assert.equal(result.sustained[0].energySpent, 540);
  assert.equal(result.sustained[0].maximumRecharge, 460);
});
test("free shots, double charges, wrong source, missing barrel, multifire and cadence fail", () => {
  for (const mutate of [
    e => { e[0].energyCost = 0; }, e => { e[0].energyAfter -= 60; },
    e => { e[1].ownerSessionId = "43"; }, e => { e.splice(2, 1); },
    e => { e[0].weapon |= 0x8000; }, e => { e[0].ship = 0; },
    e => { e[0].frequency = 769; }, e => { e[3].serverTick = 101; e[4].serverTick = 101; e[5].serverTick = 101; },
    e => { e[3].energyBefore = 1700; e[3].energyAfter = 1640; },
    e => { e.push({ ...e[0] }); },
  ]) { const events = fixture(); mutate(events); assert.throws(() => botWeaponEnergyEvidence(events, profiles)); }
});
test("short, split-life or missing-ship observations cannot count as sustained evidence", () => {
  assert.equal(botWeaponEnergyEvidence(fixture().slice(0, 9), profiles), null);
  const events = fixture().map(e => ({ ...e, ownerLifeSeq: e.shotGroupId <= 4 ? 1 : 2 }));
  assert.equal(botWeaponEnergyEvidence(events, profiles), null);
  const extra = new Map([...profiles, [769, { ...profiles.get(768), ship: 0 }]]);
  assert.equal(botWeaponEnergyEvidence(fixture(), extra), null);
  assert.equal(botWeaponEnergyEvidence(fixture(), profiles, { coverageWindows: [[99, 200], [400, 500]] }), null);
  const bounded = botWeaponEnergyEvidence(fixture().slice(0, 3), profiles, { requireSustained: false });
  assert.deepEqual(bounded.sustained, []);
  assert.equal(bounded.sustainedRequired, false);
});
test("Super is a declared zero-cost activation, not a normal free shot", () => {
  const events = fixture().slice(0, 3);
  Object.assign(events[0], { superActive: true, energyBefore: 61, energyAfter: 61, energyCost: 0 });
  assert.ok(botWeaponEnergyEvidence(events, profiles, { minimumShots: 1, minimumTicks: 0 }));
  events[0].superActive = false;
  assert.throws(() => botWeaponEnergyEvidence(events, profiles));
});

test("near-whole recharge uses the exact shared f32 debit rather than integer subtraction", () => {
  const events = fixture().slice(0, 3);
  Object.assign(events[0], { energyBefore: 1042, energyRemainderBefore: Math.fround(1 - 2 ** -24),
    energyAfter: 983, energyRemainderAfter: 0 });
  assert.ok(botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
});

test("fractional free energy or extra charges fail even when integer energies agree", () => {
  const events = fixture().slice(0, 3);
  Object.assign(events[0], { energyBefore: 1000, energyAfter: 940,
    energyRemainderBefore: 0.375, energyRemainderAfter: 0.375 });
  assert.ok(botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  for (const fraction of [0.25, 0.5]) {
    events[0].energyRemainderAfter = fraction;
    assert.throws(() => botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  }
});

test("funding is strict on available fractional energy and malformed precision fails closed", () => {
  const events = fixture().slice(0, 3);
  Object.assign(events[0], { energyBefore: 60, energyAfter: 0 });
  assert.throws(() => botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  Object.assign(events[0], { energyRemainderBefore: 0.25, energyRemainderAfter: 0.25 });
  assert.ok(botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  for (const bad of [undefined, NaN, -0.1, 1, 0.1]) {
    events[0].energyRemainderBefore = bad;
    assert.throws(() => botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  }
});

test("Super preserves both whole energy and its exact fraction", () => {
  const events = fixture().slice(0, 3);
  Object.assign(events[0], { superActive: true, energyBefore: 61, energyAfter: 61,
    energyCost: 0, energyRemainderBefore: 0.75, energyRemainderAfter: 0.75 });
  assert.ok(botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
  events[0].energyRemainderAfter = 0.5;
  assert.throws(() => botWeaponEnergyEvidence(events, profiles, { requireSustained: false }));
});
test("a correlated prize refill is allowed, another source's refill is not borrowed", () => {
  const events = fixture();
  events[3].energyBefore = 1700; events[3].energyAfter = 1640;
  const prize = { type: "server_bot_prize_committed", ownerId: 768, ownerSessionId: "42",
    ownerLifeSeq: 1, serverTick: 125, energyBefore: 1640, energyAfter: 1700 };
  events.push(prize);
  assert.ok(botWeaponEnergyEvidence(events, profiles));
  prize.ownerSessionId = "43";
  assert.throws(() => botWeaponEnergyEvidence(events, profiles));
});
