import assert from "node:assert/strict";
import test from "node:test";

import {
  assertBrowserDataChannelDeliveryAudit,
  simObservationSummary,
} from "./combat_replay.mjs";
import { rtcPressureSummary } from "../../tools/rtc_gate_audit.mjs";
import {
  snapshotAckWithinHistory,
  snapshotIdForwardDistance,
} from "../../tools/server_events.mjs";

function cleanRtcStatus() {
  return {
    rtc_inbound_packets: { game: 12, event: 3, reliable: 9 },
    rtc_ingress_execution: {limit: 4, workers: 4, in_flight: 0, waiting: 0},
    rtc_inbound_queue_packets: { game: 0, event: 0, reliable: 0 },
    rtc_inbound_peak_queue_packets: { game: 2, event: 1, reliable: 4 },
    rtc_inbound_backpressure: { game: 0, event: 0, reliable: 0 },
    position_input_queue_capacity_rejections: 0,
    realtime_event_queue_capacity_per_client: 3072,
    realtime_event_queue_current: 0,
    realtime_event_queue_peak: 7,
    realtime_event_queue_overflows: 0,
    realtime_event_actions_rejected: 0,
    realtime_event_actions_deferred: 0,
    projectile_pending_hit_capacity_total: 3072,
    projectile_pending_hit_capacity_per_owner: 128,
    projectile_pending_hit_retry_budget_per_tick: 256,
    projectile_pending_hit_current: 0,
    projectile_pending_hit_retained_bytes: 0,
    projectile_pending_damage_transactions: 0,
    projectile_pending_damage_payload_bytes: 0,
    rtc_outbound_reliable_overflows: 0,
    rtc_outbound_queue_packets: 0,
    rtc_outbound_queue_bytes: 0,
    movement_snapbacks_total: 0,
    movement_corrections_awaiting_application: 0,
    movement_corrections_awaiting_receipt_ack: 0,
  };
}

function cleanBrowserDeliveryAudit() {
  const histogram = { samples: 1, saturated: false, min_us: 10, max_us: 10,
    p50: { lower_us: 10, upper_us: 10 }, p95: { lower_us: 10, upper_us: 10 },
    p99: { lower_us: 10, upper_us: 10 } };
  return {
    serverTiming: { scope: "server-lifetime", interval: histogram, absolute_jitter: histogram, work: histogram },
    driverFull: { game: 0, event: 0, reliable: 0, unknown: 0, total: 0 },
    repelDelivery: {
      queuePackets: 0,
      maxTargetDepth: 3,
      awaitingApplication: 0,
      awaitingReceiptAck: 0,
      overflows: 0,
    },
    movementSnapbacks: { total: 0, awaitingApplication: 0, awaitingReceiptAck: 0 },
    ...rtcPressureSummary(cleanRtcStatus()),
  };
}

function fixtureServer(lines) {
  return { logs: [`prefix ignored\n${lines.join("\n")}\n`] };
}

test("sim observation summary counts explicit missing and invalid alignments", () => {
  const server = fixtureServer([
    'sim observation sample player_id=7 deviation=0.5 input_bits=1 input_alignment="stable" bounced=true repel_active=false',
    'sim observation sample player_id=7 deviation=1.5 input_bits=1 input_alignment="transition"',
    // Alignment truth must be counted even if a future malformed log line has
    // no numeric deviation and therefore cannot enter the score distribution.
    'sim observation sample player_id=7 input_bits=1 input_alignment="missing"',
    'sim observation sample player_id=7 deviation=0 input_bits=1 input_alignment="invalid" bounced=false repel_active=true',
    'sim observation sample player_id=8 deviation=0 input_bits=1 input_alignment="stable"',
    "sim observation input missing player_id=7",
  ]);

  const player = simObservationSummary(server, 0, 7);
  assert.deepEqual(player.alignmentCounts, {
    stable: 1,
    transition: 1,
    fixture: 0,
    missing: 1,
    invalid: 1,
    unclassified: 0,
  });
  assert.equal(player.missingInputSamples, 1);
  assert.equal(player.invalidInputSamples, 1);
  assert.equal(player.missingInputDiagnostics, 1);
  assert.equal(player.all.samples, 3);
  assert.equal(player.bouncedSamples, 1);
  assert.equal(player.repelActiveSamples, 1);

  const global = simObservationSummary(server);
  assert.equal(global.alignmentCounts.stable, 2);
  assert.equal(global.missingInputSamples, 1);
  assert.equal(global.invalidInputSamples, 1);
});

test("sim observation summary never silently promotes absent alignment", () => {
  const server = fixtureServer([
    "sim observation sample player_id=9 deviation=0.25 input_bits=1",
  ]);
  const summary = simObservationSummary(server, 0, 9);
  assert.equal(summary.unclassifiedSamples, 1);
  assert.equal(summary.stableActiveInput.samples, 0);
});

test("browser RTC delivery audit retains and validates the exact raw pressure source", () => {
  const audit = cleanBrowserDeliveryAudit();
  assert.equal(assertBrowserDataChannelDeliveryAudit(audit), true);
  assert.deepEqual(audit.sourceStatus, rtcPressureSummary(cleanRtcStatus()).sourceStatus);
  assert.equal(audit.outboundQueuePackets, 0);
  assert.equal(audit.outboundQueueBytes, 0);
  const noTiming = structuredClone(audit);
  delete noTiming.serverTiming;
  assert.throws(() => assertBrowserDataChannelDeliveryAudit(noTiming), /timing scope/);

  for (const mutate of [
    (candidate) => { candidate.sourceStatus = {}; },
    (candidate) => { delete candidate.sourceStatus.rtc_inbound_packets; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets = null; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets.game = "12"; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets.game = Number.NaN; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets.game = Number.POSITIVE_INFINITY; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets.game = -1; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets.game = 0.5; },
    (candidate) => { candidate.sourceStatus.rtc_inbound_packets.extra = 0; },
    (candidate) => { candidate.inboundPackets.game += 1; },
    (candidate) => { candidate.applicationQueuePackets.extra = 0; },
  ]) {
    const candidate = structuredClone(audit);
    mutate(candidate);
    assert.throws(() => assertBrowserDataChannelDeliveryAudit(candidate));
  }
});

test("browser RTC delivery audit rejects every outbound queue tail and driver Full", () => {
  for (const field of [
    "rtc_outbound_reliable_overflows",
    "rtc_outbound_queue_packets",
    "rtc_outbound_queue_bytes",
  ]) {
    const status = cleanRtcStatus();
    status[field] = 1;
    const audit = {
      driverFull: { game: 0, event: 0, reliable: 0, unknown: 0, total: 0 },
      repelDelivery: {
        queuePackets: 0,
        maxTargetDepth: 0,
        awaitingApplication: 0,
        awaitingReceiptAck: 0,
        overflows: 0,
      },
      movementSnapbacks: { total: 0, awaitingApplication: 0, awaitingReceiptAck: 0 },
      ...rtcPressureSummary(status),
    };
    assert.throws(
      () => assertBrowserDataChannelDeliveryAudit(audit),
      /pressure audit failed/,
    );
  }
  const full = cleanBrowserDeliveryAudit();
  full.driverFull.reliable = 1;
  full.driverFull.total = 1;
  assert.throws(() => assertBrowserDataChannelDeliveryAudit(full), /driverFull/);
});

test("browser RTC delivery audit rejects malformed or non-drained Repel FIFO stages", () => {
  const clean = cleanBrowserDeliveryAudit();
  for (const mutate of [
    (candidate) => { delete candidate.repelDelivery; },
    (candidate) => { candidate.repelDelivery.queuePackets = 1; },
    (candidate) => { candidate.repelDelivery.awaitingApplication = 1; },
    (candidate) => { candidate.repelDelivery.awaitingReceiptAck = 1; },
    (candidate) => { candidate.repelDelivery.overflows = 1; },
    (candidate) => { candidate.repelDelivery.maxTargetDepth = -1; },
    (candidate) => { candidate.repelDelivery.maxTargetDepth = 0.5; },
    (candidate) => { candidate.repelDelivery.extra = 0; },
  ]) {
    const candidate = structuredClone(clean);
    mutate(candidate);
    assert.throws(() => assertBrowserDataChannelDeliveryAudit(candidate), /repelDelivery|Repel/);
  }
});

test("browser RTC delivery audit rejects malformed snap-back counts", () => {
  const clean = cleanBrowserDeliveryAudit();
  for (const mutate of [
    (candidate) => { delete candidate.movementSnapbacks; },
    (candidate) => { candidate.movementSnapbacks.total = -1; },
    (candidate) => { candidate.movementSnapbacks.total = 0.5; },
    (candidate) => { candidate.movementSnapbacks.total = Number.NaN; },
    (candidate) => { candidate.movementSnapbacks.awaitingApplication = 1; },
    (candidate) => { candidate.movementSnapbacks.awaitingReceiptAck = 1; },
    (candidate) => { candidate.movementSnapbacks.extra = 0; },
  ]) {
    const candidate = structuredClone(clean);
    mutate(candidate);
    assert.throws(
      () => assertBrowserDataChannelDeliveryAudit(candidate),
      /movementSnapbacks/,
    );
  }
});

test("Compose snapshot readiness accepts only a current wrap-safe application ACK", () => {
  assert.equal(snapshotIdForwardDistance(100, 100), 0);
  assert.equal(snapshotIdForwardDistance(108, 100), 8);
  assert.equal(snapshotIdForwardDistance(3, 0xfffe), 5);
  assert.equal(snapshotIdForwardDistance(100, 108), null);

  assert.equal(snapshotAckWithinHistory(100, 100), true);
  assert.equal(snapshotAckWithinHistory(108, 100), true);
  assert.equal(snapshotAckWithinHistory(3, 0xfffe), true);
  assert.equal(snapshotAckWithinHistory(133, 100), false);
  assert.equal(snapshotAckWithinHistory(100, 108), false);
  for (const malformed of [undefined, null, 0, -1, 1.5, "1", 0x1_0000]) {
    assert.equal(snapshotAckWithinHistory(100, malformed), false);
    assert.equal(snapshotAckWithinHistory(malformed, 100), false);
  }
  assert.throws(() => snapshotAckWithinHistory(100, 100, -1));
  assert.throws(() => snapshotAckWithinHistory(100, 100, 0x8000));
});
