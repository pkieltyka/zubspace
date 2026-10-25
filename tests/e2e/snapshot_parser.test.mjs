import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

import { parseSnapshotPositionUpdates } from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  assertNoRemoteSnapDuringContinuousVisibility,
  assertRemoteRotationSmooth,
  appliedSnapshotGapSummary,
  canonicalDoorRelocationsForActor,
  matchingReliableDeathEvents,
  parseCanonicalDoorRelocations,
} from "./combat_replay.mjs";

test("remote rotation counts direction transitions, not every settling sample", () => {
  const orientations = [
    ...Array.from({ length: 11 }, (_, index) => index * 0.01),
    ...Array.from({ length: 6 }, (_, index) => 0.1 - (index + 1) * 0.01),
    ...Array(10).fill(0.04),
  ];
  const result = assertRemoteRotationSmooth("settling rotation", {
    metricsTrace: orientations.map((remoteDisplayOrientation, index) => ({
      at: index,
      remoteDisplayOrientation,
      remoteVisiblePlayers: 1,
      remoteSnapCount: 0,
    })),
  });
  assert.equal(result.signFlips, 1);
  assert.ok(result.totalAbsDelta >= 0.15);
});

test("snapshot cadence excludes only explicit completed CDP full-state reads", () => {
  const state = {
    metricsTrace: [
      { at: 0, latestSnapshotId: 1 },
      { at: 16, latestSnapshotId: 2 },
      { at: 1_016, latestSnapshotId: 3 },
      { at: 1_032, latestSnapshotId: 4 },
    ],
    instrumentationReadWindows: [{ startedAt: 20, endedAt: 1_010 }],
  };
  assert.deepEqual(appliedSnapshotGapSummary(state), {
    count: 2,
    p50Ms: 16,
    p95Ms: 16,
    maxMs: 16,
  });

  state.instrumentationReadWindows = [{ startedAt: 20, endedAt: null }];
  assert.equal(appliedSnapshotGapSummary(state).maxMs, 1_000);
});

const FULL = 1;
const X = 1 << 0;
const Y = 1 << 1;
const VELOCITY_X = 1 << 2;
const VELOCITY_Y = 1 << 3;
const DIRECTION = 1 << 4;
const ENERGY = 1 << 5;
const WEAPON = 1 << 6;
const STATUS = 1 << 7;
const BOUNTY = 1 << 8;
const PING = 1 << 9;
const FLAG_TIMER = 1 << 10;
const NEAR_COMPLETE =
  X | Y | VELOCITY_X | VELOCITY_Y | DIRECTION | ENERGY | WEAPON |
  STATUS | BOUNTY | PING | FLAG_TIMER;

const WATCHED_ACTOR = { playerId: 7, lifeSeq: 3 };
const membership = (snapshotId, id = 7, lifeSeq = 3) => ({
  latestSnapshotId: snapshotId,
  id,
  lifeSeq,
});
const presentation = (id = 7, lifeSeq = 3, snapCount = 4, initialCount = 1) => ({
  id,
  lifeSeq,
  snapCount,
  initialCount,
  lastSnapReason: initialCount > 0 ? 1 : 0,
});
const sample = (
  snapshotId,
  {
    epoch = 1,
    serial = snapshotId,
    overflow = 0,
    streamError = null,
    actors = [membership(snapshotId)],
    presentations = [presentation()],
    remotePlayers = [{ id: 7, lifeSeq: 3 }],
    remoteVisiblePlayers = remotePlayers.length,
  } = {},
) => ({
  latestSnapshotId: snapshotId,
  authoritativeSnapshotId: snapshotId,
  authoritativeSnapshotActors: actors,
  authoritativeSnapshotActorTrace: [{
    epoch,
    serial,
    latestSnapshotId: snapshotId,
    actors: actors.map(({ latestSnapshotId: _ignored, ...actor }) => actor),
  }],
  authoritativeSnapshotActorStream: {
    epoch,
    latestSerial: serial,
    overflow,
    error: streamError,
  },
  remoteActorPresentation: presentations,
  remoteVisiblePlayers,
  remotePlayers,
  // Deliberately noisy aggregate: the actor-bound audit must never read it.
  remoteSnapCount: 999,
});
const absent = (snapshotId, options = {}) => sample(snapshotId, {
  actors: [],
  remotePlayers: [],
  remoteVisiblePlayers: 0,
  ...options,
});

test("loss audit permits Initial only after forward actor-specific absence", () => {
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("loss", [
    sample(100),
    absent(101),
    sample(102, { presentations: [presentation(7, 3, 4, 2)] }),
    sample(103, { presentations: [presentation(7, 3, 4, 2)] }),
  ], WATCHED_ACTOR));
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("wrap", [
    sample(0xffff, { serial: 100 }),
    absent(1, { serial: 101 }),
    sample(2, { serial: 102, presentations: [presentation(7, 3, 4, 2)] }),
  ], WATCHED_ACTOR));
});

test("cloak or aggregate disappearance cannot reset raw wire continuity", () => {
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("cloak", [
    sample(100),
    sample(101, { remotePlayers: [], remoteVisiblePlayers: 0 }),
    sample(102),
  ], WATCHED_ACTOR));
  assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("same aggregate", [
    sample(100),
    sample(100, {
      actors: [],
      remotePlayers: [],
      remoteVisiblePlayers: 0,
    }),
    sample(101, { presentations: [presentation(7, 3, 5, 1)] }),
  ], WATCHED_ACTOR), /serial gap|continuous active motion/);
});

test("wire omission resets immediately even while the render proxy is retained", () => {
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("proxy retained", [
    sample(100),
    absent(101, { remotePlayers: [{ id: 7, lifeSeq: 3 }], remoteVisiblePlayers: 1 }),
    sample(102, { presentations: [presentation(7, 3, 4, 2)] }),
  ], WATCHED_ACTOR));
});

test("loss audit observes omission and reentry drained within one render frame", () => {
  const afterBatch = sample(103, {
    presentations: [presentation(7, 3, 4, 2)],
  });
  afterBatch.authoritativeSnapshotActorTrace = [
    { epoch: 1, serial: 101, latestSnapshotId: 101, actors: [] },
    { epoch: 1, serial: 102, latestSnapshotId: 102, actors: [] },
    { epoch: 1, serial: 103, latestSnapshotId: 103, actors: [{ id: 7, lifeSeq: 3 }] },
  ];
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("batched leave", [
    sample(100),
    afterBatch,
  ], WATCHED_ACTOR));

  const hiddenReentry = sample(103, {
    presentations: [presentation(7, 3, 4, 2)],
  });
  hiddenReentry.authoritativeSnapshotActorTrace = [
    { epoch: 1, serial: 101, latestSnapshotId: 101, actors: [{ id: 8, lifeSeq: 4 }] },
    { epoch: 1, serial: 102, latestSnapshotId: 102, actors: [{ id: 7, lifeSeq: 3 }] },
    { epoch: 1, serial: 103, latestSnapshotId: 103, actors: [{ id: 7, lifeSeq: 3 }] },
  ];
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("batched exact absence", [
    sample(100),
    hiddenReentry,
  ], WATCHED_ACTOR));
});

test("loss audit fails closed on missing or malformed actor evidence", () => {
  for (const malformed of [
    {},
    { authoritativeSnapshotId: 101, authoritativeSnapshotActors: [] },
    { ...sample(101), authoritativeSnapshotActorTrace: null },
    { ...sample(101), authoritativeSnapshotActorTrace: [] },
    { ...sample(101), authoritativeSnapshotActorTrace: [{ latestSnapshotId: 101, actors: null }] },
    { ...sample(101), authoritativeSnapshotActors: null },
    { ...sample(101), remoteActorPresentation: null },
    { ...sample(101), authoritativeSnapshotActors: [{ id: 7, lifeSeq: 3 }] },
    { ...sample(101), remoteActorPresentation: [{ id: 7, lifeSeq: 3 }] },
  ]) {
    assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("malformed", [
      sample(100),
      malformed,
    ], WATCHED_ACTOR), /malformed/);
  }
});

test("loss audit tracks the watched actor through three-player membership", () => {
  const actors = (snapshotId) => [
    membership(snapshotId, 7, 3),
    membership(snapshotId, 8, 2),
    membership(snapshotId, 9, 5),
  ];
  const before = [presentation(), presentation(8, 2, 0, 1), presentation(9, 5, 0, 1)];
  const otherSnapped = [presentation(), presentation(8, 2, 1, 1), presentation(9, 5, 0, 1)];
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("other snap", [
    sample(100, { actors: actors(100), presentations: before }),
    sample(101, { actors: actors(101), presentations: otherSnapped }),
  ], WATCHED_ACTOR));
  assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("wrong actor", [
    sample(100, { actors: actors(100), presentations: before }),
    sample(101, {
      actors: [membership(101, 7, 3), membership(101, 9, 5)],
      presentations: before,
    }),
    sample(102, {
      actors: actors(102),
      presentations: [presentation(7, 3, 5, 1), ...before.slice(1)],
    }),
  ], WATCHED_ACTOR), /serial gap|continuous active motion/);
});

test("actor membership stream retains overflow and fails closed when eviction obscures a serial", () => {
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("late observed slice", [
    sample(100, { overflow: 68 }),
    sample(101, { overflow: 69 }),
  ], WATCHED_ACTOR));
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("observed eviction", [
    sample(100),
    sample(101, { overflow: 1 }),
  ], WATCHED_ACTOR));
  for (const trace of [
    [sample(100), sample(102, { overflow: 1 })],
    [sample(100), sample(102)],
    [sample(100), sample(101, { streamError: "authoritative actor stream serial gap" })],
    [sample(100, { overflow: 2 }), sample(101, { overflow: 1 })],
  ]) {
    assert.throws(
      () => assertNoRemoteSnapDuringContinuousVisibility("stream failure", trace, WATCHED_ACTOR),
      /malformed actor-continuity evidence|serial gap|overflow/,
    );
  }
});

test("actor membership stream resets on a new epoch and supports serial wrap", () => {
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("new epoch", [
    sample(600, { epoch: 8, serial: 90 }),
    sample(3, { epoch: 9, serial: 1, presentations: [presentation(7, 3, 4, 2)] }),
  ], WATCHED_ACTOR));
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("serial wrap", [
    sample(100, { serial: 0xffff_ffff }),
    absent(101, { serial: 0 }),
    sample(102, { serial: 1, presentations: [presentation(7, 3, 4, 2)] }),
  ], WATCHED_ACTOR));
});

test("loss audit ignores backward and wrap-stale absence as a continuity reset", () => {
  assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("backward", [
    sample(100),
    absent(99),
    sample(101, { presentations: [presentation(7, 3, 5, 1)] }),
  ], WATCHED_ACTOR), /serial gap|continuous active motion/);
  assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("wrap stale", [
    sample(2),
    absent(0xffff),
    sample(3, { presentations: [presentation(7, 3, 5, 1)] }),
  ], WATCHED_ACTOR), /serial gap|continuous active motion/);
});

test("loss audit rejects exact actor snap or Initial increments while membership continues", () => {
  assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("continuous", [
    sample(100),
    sample(101, { presentations: [presentation(7, 3, 5, 1)] }),
  ], WATCHED_ACTOR), /continuous active motion/);
  assert.throws(() => assertNoRemoteSnapDuringContinuousVisibility("continuous Initial", [
    sample(100),
    sample(101, { presentations: [presentation(7, 3, 4, 2)] }),
  ], WATCHED_ACTOR), /continuous active motion/);
});

test("forward authoritative life replacement explicitly omits the old generation", () => {
  assert.doesNotThrow(() => assertNoRemoteSnapDuringContinuousVisibility("replacement", [
    sample(100),
    sample(101, {
      actors: [membership(101, 7, 4)],
      presentations: [presentation(7, 4, 0, 1)],
      remotePlayers: [{ id: 7, lifeSeq: 4 }],
    }),
    sample(102, {
      actors: [membership(102, 7, 4)],
      presentations: [presentation(7, 4, 1, 1)],
      remotePlayers: [{ id: 7, lifeSeq: 4 }],
    }),
  ], WATCHED_ACTOR));
});

test("stale-door relocation evidence is exact-player and exact-life scoped", () => {
  const line = (playerId, lifeSeq, clientSequence = 12) =>
    `DEBUG player_id=${playerId} life_seq=${lifeSeq} client_sequence=${clientSequence} ` +
    "server_tick=900 x=8200 y=8200 canonical door-close relocation observed and movement simulation rebased";
  const log = [line(7, 3), line(7, 2), line(8, 3)].join("\n");
  assert.equal(parseCanonicalDoorRelocations(log).length, 3);
  assert.deepEqual(
    canonicalDoorRelocationsForActor(log, { playerId: 7, lifeSeq: 3 })
      .map(({ playerId, lifeSeq, clientSequence }) => ({ playerId, lifeSeq, clientSequence })),
    [{ playerId: 7, lifeSeq: 3, clientSequence: 12 }],
  );
  assert.equal(
    canonicalDoorRelocationsForActor(log, { playerId: 7, lifeSeq: 4 }).length,
    0,
    "a peer or another generation cannot satisfy the intended actor proof",
  );
  assert.equal(
    canonicalDoorRelocationsForActor(`${line(7, 3)}\n${line(7, 3, 13)}`, {
      playerId: 7,
      lifeSeq: 3,
    }).length,
    2,
    "duplicates remain visible so the gate's exactly-one check rejects them",
  );
});

test("stale-door relocation parser fails closed on malformed matching logs", () => {
  for (const malformed of [
    "canonical door-close relocation observed",
    "player_id=7 life_seq=3 client_sequence=1 server_tick=2 x=3 canonical door-close relocation observed",
    "player_id=7 life_seq=0 client_sequence=1 server_tick=2 x=3 y=4 canonical door-close relocation observed",
    "player_id=7 life_seq=3 client_sequence=65536 server_tick=2 x=3 y=4 canonical door-close relocation observed",
  ]) {
    assert.throws(() => parseCanonicalDoorRelocations(malformed), /malformed canonical door/);
  }
  assert.throws(
    () => canonicalDoorRelocationsForActor("", { playerId: 7 }),
    /malformed canonical door watched actor/,
  );
});

function u16(value) {
  return [value & 0xff, (value >>> 8) & 0xff];
}

function u32(value) {
  return [value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff];
}

function encodeEntity(entity) {
  const bytes = [...u16(entity.source), ...u16(entity.fieldMask)];
  const fields = [
    [X, "x", 2],
    [Y, "y", 2],
    [VELOCITY_X, "velocityX", 2],
    [VELOCITY_Y, "velocityY", 2],
    [DIRECTION, "direction", 1],
    [ENERGY, "energy", 2],
    [WEAPON, "weapon", 2],
    [STATUS, "status", 1],
    [BOUNTY, "bounty", 2],
    [PING, "ping", 1],
    [FLAG_TIMER, "flagTimer", 2],
    [1 << 11, "lifeSeq", 4],
  ];
  for (const [bit, name, size] of fields) {
    if ((entity.fieldMask & bit) === 0) continue;
    if ((entity.fieldMask & (1 << 14)) !== 0 && (name === "x" || name === "y")) {
      bytes.push(entity[name === "x" ? "xOffset" : "yOffset"] & 0xff);
      continue;
    }
    assert.ok(Number.isInteger(entity[name]), `${name} is required by the fixture mask`);
    bytes.push(...(size === 1 ? [entity[name] & 0xff] : size === 4 ? u32(entity[name]) : u16(entity[name])));
  }
  if ((entity.fieldMask & (1 << 12)) !== 0) {
    // Compact frames carry one byte, or 0xff then the five-byte form.
    if (entity.compactSampleTime !== undefined) bytes.push(entity.compactSampleTime);
    if (entity.compactSampleTime === undefined || entity.compactSampleTime === 0xff) {
      bytes.push(...u32(entity.sampleTime.tick), entity.sampleTime.third);
    }
  }
  if ((entity.fieldMask & (1 << 13)) !== 0) {
    bytes.push(...u32(entity.motionSegment));
    if (entity.relocationGeneration !== undefined) bytes.push(...u32(entity.relocationGeneration));
  }
  return bytes;
}

function snapshotFrame({ flags = 0, snapshotId, baseId = 0, near = [], mid = [] }) {
  return Buffer.from([
    0x70,
    flags,
    ...u16(snapshotId),
    ...u16(baseId),
    ...u32(snapshotId),
    ...u32(1),
    ...u32(0),
    near.length,
    mid.length,
    0,
    ...near.flatMap(encodeEntity),
    ...mid.flatMap(encodeEntity),
  ]);
}

// Exercise the actual opt-in DataChannel impairment detector, not a second
// implementation of its entity scanner. A missed field silently disables loss.
const browserEntry = fs.readFileSync(new URL("../../index.html", import.meta.url), "utf8");
const detectorContext = vm.createContext({});
for (const name of ["readLeU16", "readLeU32", "snapshotRepelDelivery", "maybeImpairMovementCorrectionAcceptance"]) {
  const start = browserEntry.indexOf(`          function ${name}(`);
  assert.ok(start >= 0, `${name} exists in the browser entry`);
  const end = browserEntry.indexOf("\n          }", start);
  assert.ok(end > start);
  vm.runInContext(browserEntry.slice(start, end + "\n          }".length), detectorContext);
}

test("correction loss detector drops the v3 terminal once and observes its exact retry", () => {
  const state = {
    movementCorrectionAcceptancesSeen: 0,
    movementCorrectionAcceptancesDropped: 0,
    movementCorrectionAcceptancesRecovered: 0,
    movementCorrectionAcceptanceReliableId: null,
  };
  Object.assign(detectorContext, {
    movementCorrectionImpairmentConfigured: true,
    movementCorrectionPhaseControlled: false, // This fixture exercises the original automatic retry mode.
    state,
    window: { __zubspaceNetworkStats: {} },
    packetLog: () => {},
  });
  const packet = Buffer.alloc(6 + 49);
  packet[1] = 0x03;
  packet.writeUInt32LE(123, 2);
  packet[6] = 0x7c;
  packet[7] = 3;
  packet[8] = 1;
  const detect = (value) => detectorContext.maybeImpairMovementCorrectionAcceptance(value, "reliable");
  assert.equal(detect(packet), true, "the first current terminal copy must actually be lost");
  assert.equal(detect(packet), false, "the exact retry must be delivered");
  assert.equal(state.movementCorrectionAcceptancesSeen, 2);
  assert.equal(state.movementCorrectionAcceptancesDropped, 1);
  assert.equal(state.movementCorrectionAcceptancesRecovered, 1);
  const wrongVersion = Buffer.from(packet);
  wrongVersion[7] = 2;
  assert.equal(detect(wrongVersion), false);
  assert.equal(detect(packet.subarray(0, 51)), false);
  assert.equal(state.movementCorrectionAcceptancesSeen, 2, "wrong layouts cannot satisfy impairment evidence");
  detectorContext.movementCorrectionImpairmentConfigured = false;
  assert.equal(detect(packet), false);
  assert.equal(state.movementCorrectionAcceptancesSeen, 2);
});

function withPrivateRepel(frame) {
  const body = Buffer.alloc(42);
  body[4] = 1 << 3;
  body.writeUInt16LE(135, 13);
  body.writeUInt32LE(7, 15);
  const packet = Buffer.concat([frame, Buffer.from([0x03, body.length]), body]);
  packet[20] = 1;
  return packet;
}

test("Repel loss detector finds private events after source-timed full and delta entities", () => {
  for (const full of [false, true]) {
    const near = [7, 8].map((source) => nearEntity(source, {
      fieldMask: (full ? NEAR_COMPLETE : X) | (1 << 11) | (1 << 12),
      lifeSeq: 3,
      sampleTime: { tick: 100, third: 2 },
    }));
    const packet = withPrivateRepel(snapshotFrame({
      flags: 2 | (full ? FULL : 0), snapshotId: 42, near,
      mid: [{ source: 9, fieldMask: X | Y, x: 20, y: 30 }],
    }));
    const actual = detectorContext.snapshotRepelDelivery(packet);
    assert.ok(actual, `source-timed ${full ? "full" : "delta"} delivery detected`);
    assert.deepEqual({ ...actual }, { snapshotId: 42, activeSteps: 135, deliveryGeneration: 7 });
    assert.equal(detectorContext.snapshotRepelDelivery(packet.subarray(0, packet.length - 1)), null);
  }
});

test("compact frames read one-byte sample times relative to the frame tick and escaped full ones", () => {
  const COMPACT = 0x10;
  const history = new Map();
  const late = { tick: 0x7fffffff, third: 2 };
  // Frame tick 20 (snapshotFrame uses the snapshot id): 2 behind, third 1.
  const near = [
    nearEntity(7, { fieldMask: NEAR_COMPLETE | (1 << 12), compactSampleTime: 2 * 3 + 1 }),
    nearEntity(8, { fieldMask: NEAR_COMPLETE | (1 << 12), compactSampleTime: 0xff, sampleTime: late }),
  ];
  const frame = snapshotFrame({ flags: FULL | 2 | COMPACT, snapshotId: 20, near });
  assert.equal(parseSnapshotPositionUpdates(frame, history).length, 2);
  assert.deepEqual(history.get(20).get(7).sampleTime, { tick: 18, third: 1 });
  assert.deepEqual(history.get(20).get(8).sampleTime, late);

  const repel = withPrivateRepel(snapshotFrame({ flags: FULL | 2 | COMPACT, snapshotId: 42, near: [
    nearEntity(7, { fieldMask: NEAR_COMPLETE | (1 << 11) | (1 << 12), lifeSeq: 3, compactSampleTime: 4 }),
    nearEntity(8, { fieldMask: NEAR_COMPLETE | (1 << 11) | (1 << 12), lifeSeq: 3, compactSampleTime: 0xff,
      sampleTime: late }),
  ] }));
  assert.deepEqual({ ...detectorContext.snapshotRepelDelivery(repel) },
    { snapshotId: 42, activeSteps: 135, deliveryGeneration: 7 });
});

test("position offsets resolve against the acknowledged base and keep entry boundaries", () => {
  const history = new Map();
  parseSnapshotPositionUpdates(snapshotFrame({ flags: FULL | 2, snapshotId: 30, near: [
    nearEntity(7, { fieldMask: NEAR_COMPLETE | (1 << 12), x: 1000, y: 2000, sampleTime: { tick: 1, third: 0 } }),
  ] }), history);
  // x moves +5, y moves -128 (the smallest offset); the next entry must
  // still parse after the two one-byte offsets.
  const updates = parseSnapshotPositionUpdates(snapshotFrame({ flags: 2, snapshotId: 31, baseId: 30, near: [
    { source: 7, fieldMask: X | Y | (1 << 14), xOffset: 5, yOffset: -128 },
    nearEntity(8, { fieldMask: NEAR_COMPLETE | (1 << 12), x: 3000, y: 4000, sampleTime: { tick: 2, third: 0 } }),
  ] }), history);
  assert.deepEqual(updates.map(({ source, x, y }) => ({ source, x, y })), [
    { source: 7, x: 1005, y: 1872 },
    { source: 8, x: 3000, y: 4000 },
  ]);
  const repel = withPrivateRepel(snapshotFrame({ flags: 2, snapshotId: 42, baseId: 30, near: [
    { source: 7, fieldMask: X | Y | (1 << 14), xOffset: 1, yOffset: 2 },
  ] }));
  assert.deepEqual({ ...detectorContext.snapshotRepelDelivery(repel) },
    { snapshotId: 42, activeSteps: 135, deliveryGeneration: 7 });
});

test("Repel loss detector rejects unknown entity fields instead of scanning a guessed event offset", () => {
  const packet = withPrivateRepel(snapshotFrame({
    flags: 2, snapshotId: 42, near: [{ source: 7, fieldMask: 1 << 15 }],
  }));
  assert.equal(detectorContext.snapshotRepelDelivery(packet), null);
});

function nearEntity(source, overrides = {}) {
  return {
    source,
    fieldMask: NEAR_COMPLETE,
    x: 100,
    y: 200,
    velocityX: 7,
    velocityY: -3,
    direction: 9,
    energy: 1500,
    weapon: 0x42,
    status: 3,
    bounty: 5,
    ping: 8,
    flagTimer: 10,
    ...overrides,
  };
}

test("source-time snapshot revision retains fractional times across delta and actor boundaries", () => {
  const history = new Map();
  const a = { tick: 0x7fffffff, third: 2 };
  const b = { tick: 0, third: 1 };
  const near = [7, 8].map((source, i) => nearEntity(source, {
    fieldMask: NEAR_COMPLETE | (1 << 12), sampleTime: i === 0 ? a : b,
  }));
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({ flags: FULL | 2, snapshotId: 20, near }), history).length, 2);
  assert.deepEqual(history.get(20).get(7).sampleTime, a);
  assert.deepEqual(history.get(20).get(8).sampleTime, b);
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({ flags: 2, snapshotId: 21, baseId: 20,
    near: [{ source: 7, fieldMask: 1 << 12, sampleTime: b }, { source: 8, fieldMask: 0 }],
  }), history).length, 2);
  assert.deepEqual(history.get(21).get(7).sampleTime, b);
  assert.deepEqual(history.get(21).get(8).sampleTime, b);
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({ flags: FULL | 2, snapshotId: 22,
    near: [nearEntity(7, { fieldMask: NEAR_COMPLETE | (1 << 12), sampleTime: { tick: 1, third: 3 } })],
  }), history).length, 0);
  assert.equal(history.has(22), false);
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({ flags: FULL | 2, snapshotId: 23,
    near: [nearEntity(7, { fieldMask: NEAR_COMPLETE | (1 << 12), sampleTime: { tick: 0x80000000, third: 0 } })],
  }), history).length, 0);
  assert.equal(history.has(23), false);
});

test("snapshot parser consumes u32 actor lives before the next actor and preserves delta bases", () => {
  const history = new Map();
  const full = parseSnapshotPositionUpdates(snapshotFrame({
    flags: FULL, snapshotId: 20,
    near: [nearEntity(7, {fieldMask: NEAR_COMPLETE | (1 << 11), lifeSeq: 70001}),
      nearEntity(8, {fieldMask: NEAR_COMPLETE | (1 << 11), lifeSeq: 90002, x: 300})],
  }), history);
  assert.deepEqual(full.map(({source, x}) => ({source, x})), [{source: 7, x: 100}, {source: 8, x: 300}]);
  assert.equal(history.get(20).get(7).lifeSeq, 70001);
  assert.equal(history.get(20).get(8).lifeSeq, 90002);
  const delta = parseSnapshotPositionUpdates(snapshotFrame({snapshotId: 21, baseId: 20,
    near: [{source: 7, fieldMask: 1, x: 101}, {source: 8, fieldMask: 1, x: 301}],
  }), history);
  assert.deepEqual(delta.map(({source, x}) => ({source, x})), [{source: 7, x: 101}, {source: 8, x: 301}]);
  assert.equal(history.get(21).get(7).lifeSeq, 70001);
  assert.equal(history.get(21).get(8).lifeSeq, 90002);
});

test("loss gate binds reliable death evidence to the currently impaired wrapper", () => {
  const events = [
    { at: 100, reliableId: 7, killerId: 1, killedId: 2 },
    { at: 200, reliableId: 7, killerId: 1, killedId: 2 },
    { at: 210, reliableId: 8, killerId: 1, killedId: 2 },
    { at: 220, reliableId: 8, killerId: 1, killedId: 2 },
    { at: 230, reliableId: 8, killerId: 2, killedId: 1 },
  ];
  assert.deepEqual(
    matchingReliableDeathEvents(events, {
      reliableId: 8,
      killerId: 1,
      killedId: 2,
      observedAfter: 150,
    }),
    [events[2], events[3]],
  );
});

test("diagnostic parser retains an ACKed FULL base across a later in-flight FULL", () => {
  const history = new Map();
  parseSnapshotPositionUpdates(
    snapshotFrame({ flags: FULL, snapshotId: 10, near: [nearEntity(7)] }),
    history,
  );
  parseSnapshotPositionUpdates(
    snapshotFrame({
      flags: FULL,
      snapshotId: 11,
      near: [nearEntity(7, { x: 900, velocityX: 20 })],
    }),
    history,
  );

  const updates = parseSnapshotPositionUpdates(
    snapshotFrame({
      snapshotId: 12,
      baseId: 10,
      near: [{ source: 7, fieldMask: 0 }],
    }),
    history,
  );

  assert.equal(updates.length, 1);
  assert.equal(updates[0].x, 100);
  assert.equal(updates[0].velocityX, 7);
  assert.ok(history.has(10));
  assert.ok(history.has(11));
  assert.ok(history.has(12));
});

test("diagnostic parser preserves the referenced base while trimming history", () => {
  const history = new Map();
  for (let snapshotId = 1; snapshotId <= 32; snapshotId += 1) {
    parseSnapshotPositionUpdates(
      snapshotFrame({
        flags: FULL,
        snapshotId,
        near: [nearEntity(7, { x: snapshotId })],
      }),
      history,
    );
  }

  const firstDelta = parseSnapshotPositionUpdates(
    snapshotFrame({ snapshotId: 33, baseId: 1, near: [{ source: 7, fieldMask: 0 }] }),
    history,
  );
  const secondDelta = parseSnapshotPositionUpdates(
    snapshotFrame({ snapshotId: 34, baseId: 1, near: [{ source: 7, fieldMask: 0 }] }),
    history,
  );

  assert.equal(firstDelta[0].x, 1);
  assert.equal(secondDelta[0].x, 1);
  assert.equal(history.size, 32);
  assert.ok(history.has(1));
  assert.ok(!history.has(2));
});

test("diagnostic parser normalizes mid-tier bases before a return to near", () => {
  const history = new Map();
  parseSnapshotPositionUpdates(
    snapshotFrame({ flags: FULL, snapshotId: 1, near: [nearEntity(7)] }),
    history,
  );
  parseSnapshotPositionUpdates(
    snapshotFrame({
      snapshotId: 2,
      baseId: 1,
      mid: [{ source: 7, fieldMask: X, x: 120 }],
    }),
    history,
  );

  assert.deepEqual(history.get(2).get(7), {
    source: 7,
    fieldMask: X,
    x: 120,
    y: 200,
    velocityX: 0,
    velocityY: 0,
    direction: 0,
    energy: 0,
    weapon: 0,
    status: 0,
    bounty: 0,
    ping: 0,
    flagTimer: 0,
  });

  const updates = parseSnapshotPositionUpdates(
    snapshotFrame({ snapshotId: 3, baseId: 2, near: [{ source: 7, fieldMask: 0 }] }),
    history,
  );
  assert.equal(updates.length, 1);
  assert.equal(updates[0].x, 120);
  assert.equal(updates[0].y, 200);
  assert.equal(updates[0].velocityX, 0);
  assert.equal(updates[0].velocityY, 0);
  assert.equal(updates[0].weapon, 0);
});

test("motion-segment revision retains generations across dropped snapshots and unchanged deltas", () => {
  const history = new Map();
  const near = nearEntity(7, {
    fieldMask: NEAR_COMPLETE | (1 << 12) | (1 << 13),
    sampleTime: { tick: 100, third: 0 }, motionSegment: 1,
  });
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({flags: 7, snapshotId: 1, near: [near]}), history).length, 1);
  // Snapshot 2 is not delivered; 3 still references acknowledged base 1.
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({flags: 6, snapshotId: 3, baseId: 1,
    near: [{source: 7, fieldMask: X | (1 << 13), x: 130, motionSegment: 2}]}), history)[0].x, 130);
  assert.equal(history.get(3).get(7).motionSegment, 2);
  parseSnapshotPositionUpdates(snapshotFrame({flags: 6, snapshotId: 4, baseId: 3,
    near: [{source: 7, fieldMask: 0}]}), history);
  assert.equal(history.get(4).get(7).motionSegment, 2);
  const packet = withPrivateRepel(snapshotFrame({flags: 7, snapshotId: 5, near: [near]}));
  assert.ok(detectorContext.snapshotRepelDelivery(packet), "new field cannot hide the private Repel event");
  assert.equal(parseSnapshotPositionUpdates(packet.subarray(0, 24), new Map()).length, 0);
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({flags: 7, snapshotId: 5,
    near: [nearEntity(7, {fieldMask: NEAR_COMPLETE | (1 << 12), sampleTime: {tick: 100, third: 0}})]}), new Map()).length, 0);
});

test("relocation generation survives a lost warp followed by a velocity-only impulse", () => {
  const history = new Map();
  const near = nearEntity(7, {
    fieldMask: NEAR_COMPLETE | (1 << 12) | (1 << 13),
    sampleTime: { tick: 100, third: 0 }, motionSegment: 1, relocationGeneration: 1,
  });
  assert.equal(parseSnapshotPositionUpdates(snapshotFrame({ flags: 15, snapshotId: 1, near: [near] }), history).length, 1);
  // Relocation 2 was omitted. Impulse 3 keeps relocation 2 visible.
  const delta = snapshotFrame({ flags: 14, snapshotId: 3, baseId: 1,
    near: [{ source: 7, fieldMask: X | (1 << 13), x: 130, motionSegment: 3, relocationGeneration: 2 }] });
  assert.equal(parseSnapshotPositionUpdates(delta, history)[0].x, 130);
  assert.equal(history.get(3).get(7).motionSegment, 3);
  assert.equal(history.get(3).get(7).relocationGeneration, 2);
  parseSnapshotPositionUpdates(snapshotFrame({ flags: 14, snapshotId: 4, baseId: 3,
    near: [{ source: 7, fieldMask: 0 }] }), history);
  assert.equal(history.get(4).get(7).relocationGeneration, 2);
  const packet = withPrivateRepel(snapshotFrame({ flags: 15, snapshotId: 5, near: [near] }));
  assert.ok(detectorContext.snapshotRepelDelivery(packet), "paired motion fields must not displace private Repel parsing");
  for (let tail = 1; tail <= 8; tail += 1) {
    const full = snapshotFrame({ flags: 15, snapshotId: 6, near: [near] });
    assert.deepEqual(parseSnapshotPositionUpdates(full.subarray(0, full.length - tail), new Map()), []);
  }
});
