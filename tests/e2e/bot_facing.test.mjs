import test from "node:test";
import assert from "node:assert/strict";
import { botFacingEvidence, collectBotFacingEvidence, facingBearingDiversityStep } from "./bot_facing.mjs";

// One sprite bin of the 40-frame ship sheet, in turns of a full circle.
const BIN = 1 / 40;
const binOf = (bin) => bin * BIN;
const frameOf = (bin) => {
  const turns = Math.fround(binOf(bin));
  const remainder = turns % 1;
  return Math.trunc(Math.fround((remainder < 0 ? Math.fround(remainder + 1) : remainder) * 40)) % 40;
};

// A population of three launches whose ticks coincide with an authoritative draw.
// `displays` are the continuous headings the renderer was given; `prior` adds an earlier
// draw, which is the arc the next drawn heading has to step along.
function states({ shots = [10, 20, 30], authoritative = null, displays = null,
  frames = null, prior = null, updateTicks = [100, 101, 102], headings = true } = {}) {
  const told = authoritative || shots;
  const shown = displays || told;
  const entry = (bin, toldBin, i, updateTick = 0) => ({
    id: 768, sourceSessionId: "900", lifeSeq: 4, authoritativeMember: true,
    drawn: true, suppression: "none",
    shipFrame: frames ? frames[i] : frameOf(bin),
    displayOrientation: headings ? binOf(bin) : null,
    authoritativeOrientation: headings ? binOf(toldBin) : null,
    lastServerPositionTick: updateTick % 65_536,
    directedPresentation: false,
    orientationInterpolationStart: null,
    orientationInterpolationElapsed: 0,
    orientationInterpolationDuration: 0 });
  const earlier = (prior ? (Array.isArray(prior) ? prior : [prior]) : [])
    .map((frame, index) => ({
      presentationRenderTick: frame.tick ?? 99 + index,
      entries: [entry(frame.display, frame.authoritative, index,
        frame.updateTick ?? (frame.tick ?? 99 + index))],
    }));
  return [{
    metricsTrace: shots.map((bin, i) => ({ remoteProjectileSources: [{
      playerId: 768, sessionId: "900", lifeSeq: 4, shotGroupId: String(i + 1),
      serverTick: 100 + i, fireOrientation: binOf(bin), kind: "bullet",
    }] })),
    remoteSpriteTrace: shots.map((bin, i) => ({ presentationRenderTick: 100 + i,
      entries: [entry(shown[i], told[i], i, updateTicks[i])] })).concat(earlier),
  }];
}

test("bot facing binds three actual draws to exact launch ticks", () => {
  const evidence = botFacingEvidence(states(), [768]);
  assert.equal(evidence.matched, 3);
  assert.equal(evidence.toldLaunches, 3);
  assert.equal(evidence.maximumAuthorityErrorBins, 0);
  assert.equal(evidence.maximumPresentationBins, 0);
});

test("explosion fragments cannot overwrite a parent launch or count as fresh muzzle headings", () => {
  const population = states();
  const parent = population[0].metricsTrace[0].remoteProjectileSources[0];
  // Retained aged shape: group27 fired76330, exploded76370; fragments retain
  // the parent's orientation while the ship has legitimately turned by then.
  population[0].metricsTrace.push({ remoteProjectileSources: [
    { ...parent, kind: "shrapnel", serverTick: 120 },
  ] });
  population[0].remoteSpriteTrace.push({ presentationRenderTick: 120, entries: [{
    ...population[0].remoteSpriteTrace[0].entries[0], shipFrame: frameOf(39),
    displayOrientation: binOf(39), authoritativeOrientation: binOf(39), lastServerPositionTick: 120,
  }] });
  const result = botFacingEvidence(population, [768]);
  assert.equal(result.shots, 3);
  assert.equal(result.matched, 3);
  assert.equal(result.maximumAuthorityErrorBins, 0);
  assert.equal(result.samples[0].serverTick, 100);
  for (const kind of ["shrapnel", "unknown", undefined]) {
    const fragmentsOnly = states();
    for (const frame of fragmentsOnly[0].metricsTrace) frame.remoteProjectileSources[0].kind = kind;
    assert.throws(() => botFacingEvidence(fragmentsOnly, [768]), /matched 0\/0/);
  }
});

test('new collection snapshots cannot use older ring-buffer launches to satisfy freshness', () => {
  const population = states();
  assert.equal(botFacingEvidence(population, [768]).matched, 3);
  assert.throws(() => botFacingEvidence(population, [768], {minimumShotServerTick: 102}),
    /requires three exact-tick launches; matched 1\/1/);
  assert.throws(() => botFacingEvidence(population, [768], {minimumShotServerTick: 103}),
    /requires three exact-tick launches; matched 0\/0/);
  assert.equal(botFacingEvidence(population, [768], {minimumShotServerTick: 100}).matched, 3);
  assert.throws(() => botFacingEvidence(population, [768], {minimumShotServerTick: NaN}),
    /invalid minimum firing tick/);
});

test('minimum firing tick admits genuine wrap-newer launches and rejects the old epoch', () => {
  const population = states();
  assert.equal(botFacingEvidence(population, [768], {minimumShotServerTick: 0xfffffff0}).matched, 3);
  assert.throws(() => botFacingEvidence(population, [768], {minimumShotServerTick: 0x100000000}),
    /invalid minimum firing tick/);
});

test("an authoritative heading from a later update does not condemn the launch", () => {
  // The presentation frame for tick 100 already carries tick 101's heading, four bins on. The
  // launch at tick 100 is still answered by the sampled frame whose update is tick 100, and each
  // later launch is answered by the frame holding its own tick.
  const population = states({
    shots: [20, 24, 24], displays: [20, 21, 23],
    authoritative: [24, 24, 24], updateTicks: [101, 102, 103],
    prior: { display: 20, authoritative: 20, tick: 99, updateTick: 100 },
  });
  const evidence = botFacingEvidence(population, [768]);
  assert.equal(evidence.toldLaunches, 3);
  assert.equal(evidence.maximumAuthorityErrorBins, 0);
  assert.equal(evidence.maximumPresentationBins, 4);
});

test("bot facing refuses a population where the launch tick was never told", () => {
  assert.throws(() => botFacingEvidence(
    states({ updateTicks: [111, 112, 113] }),
    [768],
  ), /requires three launches whose tick the client was told/);
});

test("bot facing compares against the truncating weapon heading in all 40 bins", () => {
  for (let direction = 0; direction < 40; direction += 1) {
    // The sprite and gun must select the lower bin throughout a turn.
    const population = states({ shots: [direction + 0.75, 20, 30] });
    const evidence = botFacingEvidence(population, [768]);
    assert.equal(evidence.samples[0].expectedFrame, direction);
    assert.equal(evidence.samples[0].authorityErrorBins, 0);
  }
});

test("retained aged Chrome heading now keeps both gun and sprite in bin 39", () => {
  const population = states({ shots: [0.98809 / BIN, 20, 30] });
  const evidence = botFacingEvidence(population, [768]);
  assert.equal(evidence.samples[0].expectedFrame, 39);
  assert.equal(evidence.samples[0].authorityErrorBins, 0);
  assert.equal(evidence.samples[0].drawnFrame, 39);
  const roundedSprite = states({ shots: [0.98809 / BIN, 20, 30], frames: [0, 20, 30] });
  assert.throws(() => botFacingEvidence(roundedSprite, [768]),
    /sprite bin does not match the heading it was given/);
  // A sprite drawn two bins away from the heading it was given is a renderer defect,
  // even though it happens to land on the weapon bin.
  const wrongSprite = states({ shots: [39.52, 20, 30], frames: [38, 20, 30] });
  assert.throws(() => botFacingEvidence(wrongSprite, [768]),
    /sprite bin does not match the heading it was given/);
});

test("a drawn heading behind its authoritative heading is not an authority failure", () => {
  // Extreme authors a 360 deg/s turn rate, i.e. 40 sprite bins per second: a heading two bins
  // along its smoothing step is the shipped presentation, not a stale attacker.
  const lagged = states({ shots: [20.2, 20, 30], displays: [18.3, 20, 30],
    prior: { display: 17.4, authoritative: 20.2 } });
  const evidence = botFacingEvidence(lagged, [768]);
  assert.equal(evidence.samples[0].authorityErrorBins, 0);
  assert.equal(evidence.samples[0].presentationBins, 2);
  assert.equal(evidence.samples[0].errorBins, 2);
});

test("bot facing rejects fire whose told heading is perpendicular", () => {
  // The launch says bin 20; the heading this client was told for that tick is bin 30.
  assert.throws(() => botFacingEvidence(
    states({ shots: [20, 20, 30], authoritative: [30, 20, 30], displays: [30, 20, 30] }),
    [768],
  ), /does not match its authoritative heading/);
});

test("bot facing rejects a heading outside the span it was told", () => {
  assert.throws(() => botFacingEvidence(
    states({ shots: [12, 20, 30], displays: [25, 20, 30],
      prior: { display: 10, authoritative: 12 } }),
    [768],
  ), /outside the span of headings the renderer was told/);
});

// Captured clean413fe23 Extreme G76: the position packet is quantized to .7,
// but the authenticated weapon carries .71598 and its birth draw uses .71597606.
function firingBirthStates() {
  const population = states({ shots: [28, 28, 28],
    authoritative: [28, 28, 28], displays: [.71597606 / BIN, 28, 28],
    prior: { display: 28, authoritative: 28 } });
  const shot = population[0].metricsTrace[0].remoteProjectileSources[0];
  Object.assign(shot, { authenticated: true, fireOrientation: .71598,
    kind: "bullet", authorityProjectileId: 124, sourceProjectileIndex: 0 });
  const frame = population[0].remoteSpriteTrace[0];
  frame.entries[0].directedPresentation = true;
  frame.projectiles = { entries: [{ sourcePlayerId: 768, sourceSessionId: "900",
    sourceLifeSeq: 4, shotGroupId: "1", sourceServerTick: 100, poseTick: 100,
    authorityProjectileId: 124, sourceProjectileIndex: 0,
    drawn: true, motionFrozen: false }] };
  return population;
}

function eventOnlyFiringBirthStates() {
  const population = firingBirthStates();
  Object.assign(population[0].metricsTrace[0].remoteProjectileSources[0], {
    authorityProjectileId: null, eventId: 5,
  });
  Object.assign(population[0].remoteSpriteTrace[0].projectiles.entries[0], {
    authorityProjectileId: null, eventId: 5,
  });
  return population;
}

test("an authenticated event-only birth supplies its heading before the game carrier assigns an ID", () => {
  const evidence = botFacingEvidence(eventOnlyFiringBirthStates(), [768]);
  assert.equal(evidence.samples[0].firingAnchor, true);
  assert(evidence.samples[0].firingHeadingError < 1e-5);
});

for (const [label, mutate] of [
  ["wrong event", (s, p) => { p.eventId = 6; }],
  ["missing event", (s, p) => { delete p.eventId; }],
  ["unauthenticated event", (s, p) => { s.authenticated = false; }],
  ["known mismatched projectile", (s, p) => { p.authorityProjectileId = 125; }],
  ["unobserved native source", (s, p) => { s.authorityProjectileId = 124; }],
  ["wrong session", (s, p) => { p.sourceSessionId = "901"; }],
  ["wrong life", (s, p) => { p.sourceLifeSeq = 5; }],
  ["wrong group", (s, p) => { p.shotGroupId = "99"; }],
  ["wrong child", (s, p) => { p.sourceProjectileIndex = 1; }],
  ["late event projection", (s, p) => { p.poseTick = 101; }],
]) {
  test(`an event-only ${label} cannot authorize an untold heading`, () => {
    const population = eventOnlyFiringBirthStates();
    mutate(population[0].metricsTrace[0].remoteProjectileSources[0],
      population[0].remoteSpriteTrace[0].projectiles.entries[0]);
    assert.throws(() => botFacingEvidence(population, [768]), /outside the span/);
  });
}

test("a fresh authenticated weapon birth tells its exact firing heading", () => {
  const evidence = botFacingEvidence(firingBirthStates(), [768]);
  assert.equal(evidence.samples[0].firingAnchor, true);
  assert.ok(evidence.samples[0].firingHeadingError < 1e-5);
  assert.equal(evidence.samples[0].expectedFrame, 28);
  assert.equal(evidence.samples[0].drawnFrame, 28);
});

for (const [label, mutate] of [
  ["unauthenticated", (s, f) => { s.authenticated = false; }],
  ["wrong session", (s, f) => { f.projectiles.entries[0].sourceSessionId = "901"; }],
  ["wrong life", (s, f) => { f.projectiles.entries[0].sourceLifeSeq = 5; }],
  ["wrong player", (s, f) => { f.projectiles.entries[0].sourcePlayerId = 769; }],
  ["wrong group", (s, f) => { f.projectiles.entries[0].shotGroupId = "99"; }],
  ["wrong projectile", (s, f) => { f.projectiles.entries[0].authorityProjectileId = 125; }],
  ["wrong child", (s, f) => { f.projectiles.entries[0].sourceProjectileIndex = 1; }],
  ["late projection", (s, f) => { f.projectiles.entries[0].poseTick = 101; }],
  ["old launch", (s, f) => { f.projectiles.entries[0].sourceServerTick = 99; }],
  ["hidden projectile", (s, f) => { f.projectiles.entries[0].drawn = false; }],
  ["shrapnel", (s, f) => { s.kind = "shrapnel"; }],
  ["unknown kind", (s, f) => { s.kind = "unknown"; }],
  ["non-directed actor", (s, f) => { f.entries[0].directedPresentation = false; }],
]) {
  test(`a ${label} cannot authorize a fresh firing heading`, () => {
    const population = firingBirthStates();
    mutate(population[0].metricsTrace[0].remoteProjectileSources[0],
      population[0].remoteSpriteTrace[0]);
    assert.throws(() => botFacingEvidence(population, [768]),
      ["shrapnel", "unknown kind"].includes(label)
        ? /requires three exact-tick launches; matched 2\/2/
        : /outside the span/);
  });
}

test("a later draw at the same cursor tick cannot authorize an earlier draw", () => {
  const population = firingBirthStates();
  const first = population[0].remoteSpriteTrace[0];
  const duplicate = structuredClone(first);
  delete first.projectiles;
  population[0].remoteSpriteTrace.push(duplicate);
  assert.throws(() => botFacingEvidence(population, [768]), /outside the span/);
});

test("a future firing birth cannot excuse an earlier untold heading", () => {
  const population = firingBirthStates();
  population[0].metricsTrace[0].remoteProjectileSources[0].serverTick = 101;
  const birth = population[0].remoteSpriteTrace[0].projectiles;
  birth.entries[0].sourceServerTick = 101;
  birth.entries[0].poseTick = 101;
  delete population[0].remoteSpriteTrace[0].projectiles;
  population[0].remoteSpriteTrace[1].projectiles = birth;
  population[0].remoteSpriteTrace[1].entries[0].directedPresentation = true;
  // Preserve a distinct launch at100 to test the earlier display, alongside
  // the real future birth. It has no authenticated firing-heading witness.
  population[0].metricsTrace.push({ remoteProjectileSources: [{
    playerId: 768, sessionId: "900", lifeSeq: 4, shotGroupId: "old",
    serverTick: 100, fireOrientation: .7, kind: "bullet",
  }] });
  assert.throws(() => botFacingEvidence(population, [768]), /outside the span/);
});

test("a fresh birth still requires the exact firing heading, even within one sprite bin", () => {
  const population = firingBirthStates();
  // Both .71 and .71598 map to bin28 and lie within the told hull.
  population[0].remoteSpriteTrace[0].entries[0].displayOrientation = .71;
  assert.throws(() => botFacingEvidence(population, [768]), /exact firing heading/);
});

test("a heading past a reversed target is still inside what the renderer was told", () => {
  // The sustained Chrome/Extreme cell's real data: the client was chasing bin 24 when the server
  // reversed its target to bin 22, so the heading it drew one step later (bin 22.8) lies past the
  // current target and inside the span it was actually given. A single-endpoint arc condemned this.
  const population = states({
    shots: [22, 20, 30],
    authoritative: [22, 20, 30],
    displays: [22.8, 20, 30],
    prior: { display: 20, authoritative: 24, tick: 99 },
  });
  const evidence = botFacingEvidence(population, [768]);
  // The draw sits past the *current* target (bin 22) but inside the span up to the heading the
  // renderer was already chasing (bin 24), which is the geometry the old arc condemned.
  assert.ok(evidence.samples[0].headingDelta > binOf(22) - binOf(20),
    "the fixture must really place the draw past the current target");
  assert.ok(evidence.samples[0].headingDelta <= evidence.samples[0].headingHi,
    "and within the span the renderer was actually told");
  // Bin 22.8 now selects bin 22, matching the gun's truncating bin.
  assert.equal(evidence.samples[0].errorBins, 0);
});

test("a heading that turns back with no target behind it is still rejected", () => {
  // Same geometry, opposite direction: the renderer was told only forward headings, so a drawn
  // heading behind its previous draw is not a heading it received.
  assert.throws(() => botFacingEvidence(
    states({
      shots: [22, 20, 30],
      authoritative: [22, 20, 30],
      displays: [18.2, 20, 30],
      prior: { display: 20, authoritative: 24, tick: 99 },
    }),
    [768],
  ), /outside the span of headings the renderer was told/);
});

test("a low heading told before the previous draw still authorises the draw", () => {
  // The frozen matrix's red cell, from the retained Chrome/Chaos capture: the life was told bin 39.52
  // (0.988 turns) two samples earlier, then bin 0, and the launch's draw sits at 0.988 again - half a
  // sprite bin behind its previous sampled draw, which is where the renderer's interpolation arc was
  // still seated. `begin_orientation_interpolation` re-seats that arc at the display of its own moment,
  // so the hull of headings the life was told - not the span since the previous sample - is the bound.
  const population = states({
    shots: [40, 20, 30],
    authoritative: [40, 20, 30],
    displays: [-0.48, 20, 30],
    prior: [{ tick: 98, display: 39.52, authoritative: 39.52 },
      { tick: 99, display: 40, authoritative: 40 }],
  });
  const evidence = botFacingEvidence(population, [768]);
  const sample = evidence.samples[0];
  assert.ok(sample.headingDelta < -1e-3,
    "the fixture must really place the draw behind its previous sampled heading");
  assert.ok(sample.headingLo < -1e-3,
    "the told hull must really extend behind the previous sampled heading");
  assert.ok(sample.headingDelta >= sample.headingLo - 1e-3,
    "and the draw must sit inside that hull");
});

test("a low heading the life was never told is still rejected", () => {
  // Same draw, same arc start, without the earlier bin-39.52 update: nothing behind the previous
  // sampled heading was ever told, so the heading is not evidence-backed.
  assert.throws(() => botFacingEvidence(
    states({
      shots: [40, 20, 30],
      authoritative: [40, 20, 30],
      displays: [-0.48, 20, 30],
      prior: [{ tick: 99, display: 40, authoritative: 40 }],
    }),
    [768],
  ), /outside the span of headings the renderer was told/);
});

test("a heading told in the frame carrying the launch's update counts as told", () => {
  // The retained Firefox/Extreme cell's real geometry. The launch's own position update is
  // evidenced by a frame two cursor ticks later than the launch (the cursor renders by
  // interpolating into the sample ahead of it), and that frame reports the heading the draw was
  // rotating into. Cutting the told window at the launch tick hid it and condemned a legal draw.
  const population = states({
    shots: [22.5, 22, 23],
    authoritative: [22, 22, 23],
    displays: [22.5, 22.5, 23],
    updateTicks: [101, 102, 100],
    prior: { display: 20, authoritative: 22, tick: 99 },
  });
  const evidence = botFacingEvidence(population, [768]);
  assert.equal(evidence.toldLaunches, 3);
  assert.equal(evidence.samples[0].authorityErrorBins, 1);
  // The draw is past every heading sampled at or before its own launch tick (bin 22)...
  assert.ok(evidence.samples[0].headingDelta > binOf(22) - binOf(20),
    "the fixture must place the draw past the headings at the launch tick itself");
  // ...and inside the span once the frame carrying its update is included (bin 23).
  assert.ok(Math.abs(evidence.samples[0].headingHi - (binOf(23) - binOf(20))) < 1e-9,
    `the span must end at the bin the later frame reported: ${evidence.samples[0].headingHi}`);
  assert.ok(evidence.samples[0].headingDelta <= evidence.samples[0].headingHi);
});

test("a heading told only after the launch's update was carried is still condemned", () => {
  // The same population with the launch drawn two bins further along than any heading known by
  // the time its own update was sampled: widening the window to that frame must not become a
  // free pass for headings the client learned later.
  assert.throws(() => botFacingEvidence(
    states({
      shots: [22.5, 22, 23],
      authoritative: [22, 22, 23],
      displays: [24.5, 22.5, 23],
      updateTicks: [101, 102, 100],
      prior: { display: 20, authoritative: 22, tick: 99 },
    }),
    [768],
  ), /outside the span of headings the renderer was told/);
});

test("bot facing rejects a heading further behind than any authored smoothing", () => {
  assert.throws(() => botFacingEvidence(
    states({ shots: [20, 20, 30], displays: [11, 20, 30],
      prior: { display: 10, authoritative: 20 } }),
    [768],
  ), /further behind its authoritative heading/);
});

test("bot facing requires the headings of the launch tick", () => {
  assert.throws(() => botFacingEvidence(states({ headings: false }), [768]),
    /needs the drawn and authoritative headings/);
});

test("bot facing rejects radians-compressed sprite headings", () => {
  assert.throws(() => botFacingEvidence(
    states({ displays: [3, 3, 3] }),
    [768],
  ), /further behind its authoritative heading|sprite bin does not match/);
});

for (const field of ["lifeSeq", "sourceSessionId"]) {
  test(`bot facing rejects wrong ${field}`, () => {
    const population = states();
    population[0].remoteSpriteTrace.forEach((frame) => {
      if (field === "lifeSeq") frame.entries[0].lifeSeq = 9;
      else frame.entries[0].sourceSessionId = "901";
    });
    assert.throws(() => botFacingEvidence(population, [768]), /three exact-tick/);
  });
}

test("bot facing cannot use a later frame or invisible actor", () => {
  const population = states();
  population[0].remoteSpriteTrace.forEach((frame) => { frame.presentationRenderTick += 20; });
  assert.throws(() => botFacingEvidence(population, [768]), /three exact-tick/);
  const invisible = states();
  invisible[0].remoteSpriteTrace.forEach((frame) => { frame.entries[0].drawn = false; });
  assert.throws(() => botFacingEvidence(invisible, [768]), /three exact-tick/);
});

test("collection grows the population instead of lowering the three-launch floor", async () => {
  // One launch survives this population, so the floor is unmet and the witness
  // refuses it; extra samples must satisfy the same unmodified assertion.
  const partial = states();
  partial[0].metricsTrace = partial[0].metricsTrace.slice(0, 1);
  partial[0].remoteSpriteTrace = partial[0].remoteSpriteTrace.slice(0, 1);
  assert.throws(() => botFacingEvidence(partial, [768]), /three exact-tick/);
  const evidence = await collectBotFacingEvidence({
    states: partial,
    sample: () => states()[0],
    botIds: [768],
    intervalMs: 0,
    sleep: async () => {},
  });
  assert.equal(evidence.matched, 3);
  assert.equal(evidence.sampledStates, 1);
  assert.equal(evidence.maximumErrorBins, 0);
  assert.ok(evidence.samples.some((sample) => sample.expectedFrame >= 10));
  // The caller's own population is untouched for its other consumers.
  assert.equal(partial.length, 1);
});

test("collection rethrows the witness verdict when extra sampling cannot meet the floor", async () => {
  const empty = { metricsTrace: [], remoteSpriteTrace: [] };
  let samples = 0;
  await assert.rejects(collectBotFacingEvidence({
    states: [empty],
    sample: () => { samples += 1; return empty; },
    botIds: [768],
    timeoutMs: 10,
    intervalMs: 1,
    sleep: async () => {},
  }), /three exact-tick launches; matched 0\/0/);
  assert.ok(samples >= 1);
});

test("collection cannot sample away an actual facing mismatch", async () => {
  const mismatched = states({ displays: [10, 20, 30], frames: [3, 3, 3] });
  await assert.rejects(collectBotFacingEvidence({
    states: mismatched,
    sample: () => states()[0],
    botIds: [768],
    timeoutMs: 15,
    intervalMs: 1,
    sleep: async () => {},
  }), /sprite bin does not match the heading it was given/);
});

test("collection provokes launches before it spends a sample", async () => {
  // A sampled presentation window is worthless while the hostile fires nothing, so the collector
  // must give the caller a chance to produce launches before each extra sample.
  const partial = states();
  partial[0].metricsTrace = partial[0].metricsTrace.slice(0, 1);
  partial[0].remoteSpriteTrace = partial[0].remoteSpriteTrace.slice(0, 1);
  const provoked = [];
  const evidence = await collectBotFacingEvidence({
    states: partial,
    sample: () => states()[0],
    botIds: [768],
    intervalMs: 0,
    provoke: async () => { provoked.push("bait"); return true; },
    sleep: async () => {},
  });
  assert.equal(evidence.matched, 3);
  assert.deepEqual(provoked, ["bait"], "one provoke per extra sample, none for the base population");
});

test("collection never exposes the pilot when the floor is already met", async () => {
  let provoked = 0;
  const evidence = await collectBotFacingEvidence({
    states: states(),
    sample: () => states()[0],
    botIds: [768],
    intervalMs: 0,
    provoke: async () => { provoked += 1; return true; },
    sleep: async () => {},
  });
  assert.equal(evidence.matched, 3);
  assert.equal(provoked, 0, "a sufficient population must not buy more risk");
});

test("collection tells the provoke hook which cycle it is on", async () => {
  // The caller alternates between baiting a launch and relocating the pilot, and it can only do
  // that if the collector identifies the cycle.
  const partial = states();
  partial[0].metricsTrace = partial[0].metricsTrace.slice(0, 1);
  partial[0].remoteSpriteTrace = partial[0].remoteSpriteTrace.slice(0, 1);
  let samples = 0;
  const cycles = [];
  const evidence = await collectBotFacingEvidence({
    states: partial,
    sample: () => { samples += 1; return samples >= 3 ? states()[0] : partial[0]; },
    botIds: [768],
    intervalMs: 0,
    provoke: async (cycle) => { cycles.push(cycle); return true; },
    sleep: async () => {},
  });
  assert.equal(evidence.matched, 3);
  assert.deepEqual(cycles, [0, 1, 2], "one cycle per collection round, counting from the bait phase");
});

// The witness condemns a population whose launches never leave the compressed arc, so the
// collection maneuver has to be measured in the witness's own units: bins of the 40-frame sheet.
const headingTurnsTo = (from, to) =>
  ((Math.atan2(to.x - from.x, -(to.y - from.y)) / (Math.PI * 2)) % 1 + 1) % 1;
const spriteFrame = (turns) => Math.trunc(Math.fround(turns * 40)) % 40;

test("the diversity station moves the hostile across ten sprite bins", () => {
  const hostile = { x: 8_000, y: 8_000 };
  const pilot = { x: 8_000, y: 7_800 };
  const step = facingBearingDiversityStep({
    cycle: 1, playerX: pilot.x, playerY: pilot.y, targetX: hostile.x, targetY: hostile.y,
  });
  assert.ok(step != null, "an engaged pilot at 200 px must offer a station");
  assert.ok(
    Math.abs(Math.hypot(step.x - hostile.x, step.y - hostile.y) - 200) < 1e-6,
    "the station must keep the range the bot is already engaged at",
  );
  const before = spriteFrame(headingTurnsTo(hostile, pilot));
  const after = spriteFrame(headingTurnsTo(hostile, step));
  const distance = Math.min(Math.abs(after - before), 40 - Math.abs(after - before));
  assert.equal(distance, 10, "a quarter turn is ten of the witness's forty bins");
  assert.ok(before >= 10 || after >= 10, "the pair must reach outside the compressed arc");
});

test("the diversity station alternates direction so the pilot does not drift", () => {
  const hostile = { x: 8_000, y: 8_000 };
  const pilot = { x: 8_000, y: 7_800 };
  const common = { playerX: pilot.x, playerY: pilot.y, targetX: hostile.x, targetY: hostile.y };
  const first = facingBearingDiversityStep({ cycle: 1, ...common });
  const second = facingBearingDiversityStep({ cycle: 3, ...common });
  assert.equal(first.quarterTurn, -second.quarterTurn);
  assert.deepEqual(
    facingBearingDiversityStep({ cycle: 2, ...common }),
    null,
    "the bait half of each pair keeps the ordinary lure",
  );
});

test("the diversity station refuses geometry it cannot use", () => {
  const hostile = { x: 8_000, y: 8_000 };
  assert.equal(
    facingBearingDiversityStep({
      cycle: 1, playerX: 8_040, playerY: 8_000, targetX: hostile.x, targetY: hostile.y,
    }),
    null,
    "a pilot inside the minimum radius has no useful quarter turn",
  );
  assert.equal(
    facingBearingDiversityStep({
      cycle: 1, playerX: 400, playerY: 16_300, targetX: 200, targetY: 16_300,
    }),
    null,
    "a station outside the authored map is not a station",
  );
  assert.equal(
    facingBearingDiversityStep({
      cycle: 1, playerX: Number.NaN, playerY: 7_800, targetX: hostile.x, targetY: hostile.y,
    }),
    null,
    "a client that has not reported a position yet gets no maneuver",
  );
});
