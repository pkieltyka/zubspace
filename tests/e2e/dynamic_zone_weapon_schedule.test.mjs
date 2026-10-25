import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import {
  isExactPlayerEnteringPayload,
  cancelledProjectileAfterRosterReplacement,
  immutableProjectileSourceMatches,
  moveOutOfSafeZoneForWeaponProof,
  mutualRosterConverged,
  orderedZoneFilesSettledForLifecycle,
  retainedProjectileAfterDamageDeath,
  selfJoinBootstrapSettled,
  shipResetLifecycleSettled,
  lifecycleBaselineWireEvidence,
  shipResetBefore,
  initialSpawnLifecycleEvidence,
  assertInitialSpawnPositionStream,
} from "../../tools/dynamic_zone_weapon_schedule.mjs";

test("intentional ship lifecycle input waits for the ordered zone-file tail", () => {
  assert.equal(
    orderedZoneFilesSettledForLifecycle({
      localGameplayState: { zoneMapReady: true, zoneFilesReady: false },
    }),
    false,
  );
  assert.equal(
    orderedZoneFilesSettledForLifecycle({
      localGameplayState: { zoneMapReady: true, zoneFilesReady: true },
    }),
    true,
  );
  assert.equal(orderedZoneFilesSettledForLifecycle(null), false);
});

function lifecycleMetrics(overrides = {}) {
  return {
    selfPlayerId: 1,
    playerLifeSeq: 2,
    playerX: 8200,
    playerY: 8200,
    playerVelocityX: 0,
    playerVelocityY: 0,
    lifecycleWireEvidence: { lastPacketSequence: 100,
      baseline: { packetSequence: 100, lifeSeq: 2, deliveryId: 7,
        positionSequence: 123, acceptedServerTick: 1000, x: 8200, y: 8200 } },
    localGameplayState: {
      authorityScopeServerSessionId: "77",
      authorityScopeArenaStreamId: "3",
      authorityScopePlayerId: 1,
      authorityScopeLifeSeq: 2,
      authorityScopeAlive: true,
      authorityPhase: "Live",
      authorityTransitionKind: null,
      authorityApplicationBarrierKind: null,
      authorityApplicationBarrierCount: 0,
      pendingShipResetPosition: false,
      pendingShipResetDeliveryId: null,
      pendingShipResetLifeSeq: null,
      pendingShipResetSequence: null,
      pendingFrequencyBaselineDeliveryId: null,
      lifecycleBarrierElapsed: null,
      lifecycleBaselineReceiptsSeen: 1,
      lifecycleBaselineReceiptsMatched: 1,
      lifecycleBaselineReceiptsUnmatched: 0,
    },
    ...overrides,
  };
}

function lifecycleBefore() {
  return { selfPlayerId: 1, serverSessionId: "77", arenaStreamId: "3", lastPacketSequence: 50 };
}

function acceptedLifecyclePacket({ lifeSeq = 2, x = 7208, y = 6920, sequence = 100 } = {}) {
  const bytes = Buffer.alloc(26);
  bytes[1] = 3; bytes[6] = 0x75; bytes[7] = 3;
  bytes.writeUInt32LE(7, 8); bytes.writeUInt32LE(lifeSeq, 12);
  bytes.writeUInt16LE(123, 16); bytes.writeUInt32LE(1000, 18);
  bytes.writeUInt16LE(x, 22); bytes.writeUInt16LE(y, 24);
  return { seq: sequence, direction: "received", detail: { channel: "zubspace-reliable" },
    byteLength: 26, bytesTruncated: false, bytes: [...bytes] };
}

function initialSpawnFixture() {
  // Native wire fixture using the captured dynamic-zone initial coordinates;
  // the original browser artifact did not retain its receipt bytes.
  return lifecycleMetrics({ playerX: 7400, playerY: 7608,
    lifecycleWireEvidence: lifecycleBaselineWireEvidence([
      acceptedLifecyclePacket({ x: 7400, y: 7608 })], 2) });
}

test("initial smoke positions follow the exact accepted random spawn instead of a fixed centre", () => {
  const metrics = initialSpawnFixture(), evidence = initialSpawnLifecycleEvidence(metrics);
  assert.ok(evidence);
  assert.deepEqual([evidence.baseline.x, evidence.baseline.y], [7400, 7608]);
  assert.deepEqual(evidence.baseline.wireBytes, acceptedLifecyclePacket({ x: 7400, y: 7608 }).bytes);
  assert.doesNotThrow(() => assertInitialSpawnPositionStream("alpha", {
    sentPositions: Array.from({ length: 5 }, () => ({ x: 7400, y: 7608 })),
  }, evidence));
  for (const bad of [[], [{ x: 8200, y: 8200 }], [{ x: NaN, y: 7608 }],
    [{ x: 7593, y: 7608 }]]) {
    assert.throws(() => assertInitialSpawnPositionStream("alpha", { sentPositions: bad }, evidence),
      /did not match the accepted initial spawn/);
  }
});

test("captured initial positions must be checked before a genuine ship change replaces that spawn", () => {
  const metrics = initialSpawnFixture();
  const evidence = initialSpawnLifecycleEvidence(metrics);
  const initial = Array.from({ length: 3 }, () => ({ x: 7400, y: 7608 }));
  const replacement = [{ x: 8360, y: 7432 }, { x: 8360, y: 7432 }];
  assert.doesNotThrow(() => assertInitialSpawnPositionStream("beta", { sentPositions: initial }, evidence));
  assert.throws(() => assertInitialSpawnPositionStream("beta", {
    sentPositions: [...initial, ...replacement],
  }, evidence), /did not match the accepted initial spawn/);
  const smoke = fs.readFileSync(new URL("../../tools/browser_real_server_smoke.mjs", import.meta.url), "utf8");
  const firstCheck = smoke.indexOf('assertInitialSpawnPositionStream("alpha",');
  const driver = smoke.indexOf('await Promise.all([alpha.drive(), beta.drive()])');
  assert.ok(firstCheck > 0 && firstCheck < driver,
    "the unchanged oracle must run before the driver intentionally replaces life");
});

test("initial spawn evidence refuses incomplete wire, wrong life, identity, barriers or a different pose", () => {
  for (const change of [
    m => { m.lifecycleWireEvidence.baseline = null; },
    m => { m.lifecycleWireEvidence.baseline.wireBytes = null; },
    m => { m.lifecycleWireEvidence.baseline.lifeSeq++; },
    m => { m.localGameplayState.authorityScopePlayerId++; },
    m => { m.localGameplayState.authorityScopeLifeSeq++; },
    m => { m.localGameplayState.authorityScopeServerSessionId = null; },
    m => { m.localGameplayState.authorityScopeAlive = false; },
    m => { m.localGameplayState.authorityPhase = "Spectating"; },
    m => { m.localGameplayState.authorityApplicationBarrierCount = 1; },
    m => { m.localGameplayState.lifecycleBaselineReceiptsSeen++; },
    m => { m.localGameplayState.lifecycleBaselineReceiptsUnmatched++; },
    m => { m.playerX += 5; }, m => { m.playerY = NaN; },
    m => { m.playerVelocityX = 65; },
  ]) {
    const metrics = initialSpawnFixture(); change(metrics);
    assert.equal(initialSpawnLifecycleEvidence(metrics), null);
  }
});

test("ship lifecycle wire proof selects only a complete current-life v3 reliable receipt", () => {
  const exact = acceptedLifecyclePacket();
  const evidence = lifecycleBaselineWireEvidence([exact, { seq: 120, direction: "sent" }], 2);
  assert.equal(evidence.lastPacketSequence, 120);
  assert.deepEqual([evidence.baseline.x, evidence.baseline.y], [7208, 6920]);
  assert.deepEqual(evidence.baseline.wireBytes, exact.bytes);
  for (const invalid of [
    { ...exact, bytesTruncated: true }, { ...exact, byteLength: 50 },
    { ...exact, direction: "sent" }, { ...exact, detail: { channel: "zubspace-game" } },
    acceptedLifecyclePacket({ lifeSeq: 3 }), acceptedLifecyclePacket({ x: 16384 }),
    { ...exact, bytes: exact.bytes.slice(0, -1) },
    { ...exact, bytes: exact.bytes.map((v, i) => i === 7 ? 2 : v) },
  ]) assert.equal(lifecycleBaselineWireEvidence([invalid], 2).baseline, null);
});

test("captured randomized poses require a matching v3 receipt in native selector checks", () => {
  const before = { ...lifecycleBefore(), playerLifeSeq: 1,
    lifecycleBaselineReceiptsMatched: 0, lifecycleBaselineReceiptsUnmatched: 0 };
  for (const [x, y] of [[8872, 8264], [7208, 6920]]) {
    const metrics = lifecycleMetrics({ playerX: x, playerY: y,
      lifecycleWireEvidence: lifecycleBaselineWireEvidence([acceptedLifecyclePacket({ x, y })], 2) });
    assert(shipResetLifecycleSettled(before, metrics));
    assert(!shipResetLifecycleSettled(before, { ...metrics, playerX: x + 5 }));
    assert(!shipResetLifecycleSettled({ ...before, lastPacketSequence: 100 }, metrics));
    assert(!shipResetLifecycleSettled(before, { ...metrics, lifecycleWireEvidence: null }));
    assert(!shipResetLifecycleSettled({ ...before, serverSessionId: "old" }, metrics));
    assert(!shipResetLifecycleSettled({ ...before, arenaStreamId: "old" }, metrics));
    assert(!shipResetLifecycleSettled(before, { ...metrics, selfPlayerId: 9 }));
    const preparedBefore = shipResetBefore(metrics);
    assert.equal(preparedBefore.lastPacketSequence, 100);
    assert.equal(preparedBefore.serverSessionId, "77");
  }
});

test("ship reset waits for the exact application receipt instead of roster position alone", () => {
  const before = {
    ...lifecycleBefore(),
    playerLifeSeq: 1,
    lifecycleBaselineReceiptsMatched: 0,
    lifecycleBaselineReceiptsUnmatched: 0,
  };
  const premature = lifecycleMetrics({
    localGameplayState: {
      ...lifecycleMetrics().localGameplayState,
      pendingShipResetPosition: true,
      lifecycleBaselineReceiptsSeen: 0,
      lifecycleBaselineReceiptsMatched: 0,
    },
  });
  assert.equal(shipResetLifecycleSettled(before, premature), false);
  assert.equal(shipResetLifecycleSettled(before, lifecycleMetrics()), true);
});

test("ship reset rejects unmatched or still-pending lifecycle deliveries", () => {
  const before = {
    ...lifecycleBefore(),
    playerLifeSeq: 1,
    lifecycleBaselineReceiptsMatched: 4,
    lifecycleBaselineReceiptsUnmatched: 0,
  };
  const pending = lifecycleMetrics({
    localGameplayState: {
      ...lifecycleMetrics().localGameplayState,
      pendingShipResetDeliveryId: 77,
      lifecycleBaselineReceiptsSeen: 5,
      lifecycleBaselineReceiptsMatched: 5,
    },
  });
  assert.equal(shipResetLifecycleSettled(before, pending), false);
  const unmatched = lifecycleMetrics({
    localGameplayState: {
      ...lifecycleMetrics().localGameplayState,
      lifecycleBaselineReceiptsSeen: 6,
      lifecycleBaselineReceiptsMatched: 5,
      lifecycleBaselineReceiptsUnmatched: 1,
    },
  });
  assert.equal(shipResetLifecycleSettled(before, unmatched), false);
});

test("ship reset reads the typed sequencer barrier instead of retired elapsed state", () => {
  const before = {
    ...lifecycleBefore(),
    playerLifeSeq: 1,
    lifecycleBaselineReceiptsMatched: 0,
    lifecycleBaselineReceiptsUnmatched: 0,
  };
  const activeBarrier = lifecycleMetrics({
    localGameplayState: {
      ...lifecycleMetrics().localGameplayState,
      authorityApplicationBarrierKind: "ShipResetBaseline",
      authorityApplicationBarrierCount: 1,
      lifecycleBarrierElapsed: null,
    },
  });
  assert.equal(shipResetLifecycleSettled(before, activeBarrier), false);
  assert.equal(shipResetLifecycleSettled(before, lifecycleMetrics()), true);
});

test("safe-zone exit uses only lightweight metrics while the ship is coasting", async () => {
  const states = [true, true, false];
  let cruises = 0;
  const client = {
    async uiMetrics() {
      return { inSafeZone: states.shift() };
    },
    async cruise(durationMs) {
      assert.equal(durationMs, 250);
      cruises += 1;
    },
    async pageState() {
      assert.fail("full screenshot/page-state probes are forbidden during safe-zone exit");
    },
  };
  const result = await moveOutOfSafeZoneForWeaponProof(client);
  assert.equal(result.inSafeZone, false);
  assert.equal(cruises, 2);
});

test("simultaneous joins separate self bootstrap from reliable peer-roster convergence", () => {
  const alpha = {
    selfId: 1,
    receivedJoinGameCount: 1,
    rosterNames: ["alpha"],
  };
  const beta = {
    selfId: 2,
    receivedJoinGameCount: 1,
    rosterNames: ["beta"],
  };
  assert.equal(selfJoinBootstrapSettled(alpha, "alpha"), true);
  assert.equal(selfJoinBootstrapSettled(beta, "beta"), true);
  assert.equal(mutualRosterConverged(alpha, beta), false);

  // A large already-sequenced map transfer may precede the cross-session
  // PlayerEntering fanout. Gameplay remains closed until this later state.
  alpha.rosterNames.push("beta");
  beta.rosterNames.push("alpha");
  assert.equal(mutualRosterConverged(alpha, beta), true);
});

test("roster framing rejects transfer payloads that merely begin with 0x03", () => {
  assert.equal(isExactPlayerEnteringPayload(Buffer.alloc(64, 3)), true);
  assert.equal(isExactPlayerEnteringPayload(Buffer.alloc(65, 3)), false);
  assert.equal(isExactPlayerEnteringPayload(Buffer.alloc(63, 3)), false);
  assert.equal(isExactPlayerEnteringPayload(Buffer.alloc(64, 2)), false);
  const extended = Buffer.alloc(84);
  extended[0] = 3;
  extended[64] = 9;
  extended[79] = 1;
  extended.writeUInt16LE(7, 51);
  extended.writeUInt16LE(7, 65);
  extended.writeUInt32LE(123, 43);
  extended.writeUInt32LE(123, 67);
  extended.writeUInt32LE(3, 80);
  assert.equal(isExactPlayerEnteringPayload(extended), true);
  assert.equal(isExactPlayerEnteringPayload(extended.subarray(0, 83)), false);
  extended[65] = 8;
  assert.equal(isExactPlayerEnteringPayload(extended), false);
  extended[65] = 7;
  extended[79] = 2;
  assert.equal(isExactPlayerEnteringPayload(extended), false);
});

function projectileSource(overrides = {}) {
  return {
    playerId: 9,
    authenticated: true,
    sessionId: "18446744073709551615",
    lifeSeq: 0xffffffff,
    serverSessionId: "77",
    arenaStreamId: "88",
    kind: "mine",
    fireX: 100,
    fireY: 200,
    fireVelocityX: 3,
    fireVelocityY: -4,
    fireOrientation: 0.75,
    frequency: 7,
    ship: 0,
    bombGravity: 40,
    bombBounces: 7,
    ...overrides,
  };
}

test("damage-death projectile remains exact across same-id session and life wrap replacement", () => {
  const old = projectileSource();
  const retained = retainedProjectileAfterDamageDeath(old, 1, [
    projectileSource({ sessionId: "1", lifeSeq: 1, bombBounces: 1 }),
    { ...old },
  ]);
  assert.deepEqual(retained, old);
  assert.equal(immutableProjectileSourceMatches(old, retained), true);
});

test("replacement projectile cannot stand in for an old runtime identity", () => {
  const old = projectileSource();
  const replacement = projectileSource({
    sessionId: "1",
    lifeSeq: 1,
    fireX: 999,
    bombGravity: -40,
    bombBounces: 1,
  });
  assert.equal(
    retainedProjectileAfterDamageDeath(old, 1, [replacement]),
    null,
  );
  assert.equal(immutableProjectileSourceMatches(old, replacement), false);
});

test("same-life or mutated historical state cannot satisfy lifecycle proof", () => {
  const old = projectileSource();
  assert.equal(
    retainedProjectileAfterDamageDeath(old, old.lifeSeq, [{ ...old }]),
    null,
  );
  assert.equal(
    retainedProjectileAfterDamageDeath(old, 1, [
      projectileSource({ bombBounces: 1 }),
    ]),
    null,
  );
});


function cancellation(overrides = {}) {
  return { source: 9, sourceSessionId: "18446744073709551615", serverSessionId: "77", arenaStreamId: "88", throughShotGroupId: "12", ownerLifeSeq: 1, ...overrides };
}
test("intentional ship replacement needs an authenticated cutoff and absent old flight", () => {
  const old = projectileSource({ shotGroupId: "10" });
  assert.equal(cancelledProjectileAfterRosterReplacement(old, 1, [{ ...old }], [cancellation()]), null);
  assert.equal(cancelledProjectileAfterRosterReplacement(old, 1, [], []), null);
  assert.deepEqual(cancelledProjectileAfterRosterReplacement(old, 1, [], [cancellation()]), cancellation());
});
test("roster cancellation keeps exact scope and source across wrap and coalesced ship cycles", () => {
  const old = projectileSource({ shotGroupId: "10" });
  for (const changed of [{ source: 8 }, { sourceSessionId: "1" }, { serverSessionId: "78" }, { arenaStreamId: "89" }, { ownerLifeSeq: 0xffffffff }, { ownerLifeSeq: 3 }, { throughShotGroupId: "9" }]) {
    assert.equal(cancelledProjectileAfterRosterReplacement(old, 2, [], [cancellation(changed)]), null);
  }
  assert.ok(cancelledProjectileAfterRosterReplacement(old, 2, [projectileSource({ shotGroupId: "13", lifeSeq: 2 }), projectileSource({ shotGroupId: "1", sessionId: "2" })], [cancellation()]));
  assert.equal(cancelledProjectileAfterRosterReplacement(old, 2, [projectileSource({ shotGroupId: "11", lifeSeq: 2 })], [cancellation()]), null);
});
