import assert from "node:assert/strict";
import test from "node:test";

import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
  rtcPressureSummary,
  rtcServerLogAudit,
} from "../tools/rtc_gate_audit.mjs";

test("RTC cleanup rejects handed-off work while live traffic may retain it", () => {
  for (const field of ["in_flight", "waiting"]) {
    const status = cleanStatus();
    status.rtc_ingress_execution[field] = 1;
    assert.doesNotThrow(() => assertRtcPressureClean(status, "live"));
    assert.throws(() => assertRtcPressureClean(status, "cleanup", {requireIngressDrained: true}));
  }
  assert.doesNotThrow(() => assertRtcPressureClean(cleanStatus(), "cleanup", {requireIngressDrained: true}));
});

test("RTC ingress evidence rejects missing, malformed and impossible worker state", () => {
  for (const value of [undefined, null, [], {}, {limit:4,workers:3,in_flight:0,waiting:0},
    {limit:4,workers:4,in_flight:5,waiting:0}, {limit:0,workers:0,in_flight:0,waiting:1}]) {
    assert.throws(() => rtcPressureSummary({...cleanStatus(), rtc_ingress_execution:value}));
  }
  for (const field of ["limit", "workers", "in_flight", "waiting"]) {
    for (const value of [-1, 0.5, "1", NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      const status = cleanStatus();
      status.rtc_ingress_execution[field] = value;
      assert.throws(() => rtcPressureSummary(status));
    }
  }
  assert.doesNotThrow(() => rtcPressureSummary({...cleanStatus(),
    rtc_ingress_execution:{limit:0,workers:0,in_flight:0,waiting:0}}));
});

function cleanStatus() {
  return {
    rtc_ingress_execution: {limit: 4, workers: 4, in_flight: 0, waiting: 0},
    rtc_inbound_packets: { game: 12, event: 3, reliable: 9 },
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
  };
}

test("RTC pressure audit reports every channel and rejects any pressure or queued tail", () => {
  assert.deepEqual(assertRtcPressureClean(cleanStatus(), "clean"), {
    sourceStatus: cleanStatus(),
    ingressExecution: cleanStatus().rtc_ingress_execution,
    inboundPackets: { game: 12, event: 3, reliable: 9 },
    applicationQueuePackets: { game: 0, event: 0, reliable: 0 },
    applicationPeakQueuePackets: { game: 2, event: 1, reliable: 4 },
    applicationBackpressure: { game: 0, event: 0, reliable: 0 },
    positionInputQueueCapacityRejections: 0,
    realtimeEventQueueCapacityPerClient: 3072,
    realtimeEventQueueCurrent: 0,
    realtimeEventQueuePeak: 7,
    realtimeEventQueueOverflows: 0,
    realtimeEventActionsRejected: 0,
    realtimeEventActionsDeferred: 0,
    projectilePendingHitCapacityTotal: 3072,
    projectilePendingHitCapacityPerOwner: 128,
    projectilePendingHitRetryBudgetPerTick: 256,
    projectilePendingHitCurrent: 0,
    projectilePendingHitRetainedBytes: 0,
    projectilePendingDamageTransactions: 0,
    projectilePendingDamagePayloadBytes: 0,
    outboundReliableOverflows: 0,
    outboundQueuePackets: 0,
    outboundQueueBytes: 0,
  });
  for (const [field, value] of [
    ["rtc_inbound_queue_packets", { game: 0, event: 0, reliable: 1 }],
    ["rtc_inbound_backpressure", { game: 1, event: 0, reliable: 0 }],
  ]) {
    assert.throws(
      () => assertRtcPressureClean({ ...cleanStatus(), [field]: value }, field),
      /pressure audit failed/,
    );
  }
  assert.throws(
    () => assertRtcPressureClean({ ...cleanStatus(), rtc_outbound_reliable_overflows: 1 }, "out"),
    /pressure audit failed/,
  );
  for (const field of ["rtc_outbound_queue_packets", "rtc_outbound_queue_bytes"]) {
    assert.throws(
      () => assertRtcPressureClean({ ...cleanStatus(), [field]: 1 }, field),
      /pressure audit failed/,
    );
  }
  for (const field of [
    "position_input_queue_capacity_rejections",
    "realtime_event_queue_current",
    "realtime_event_queue_overflows",
    "realtime_event_actions_rejected",
    "realtime_event_actions_deferred",
    "projectile_pending_hit_current",
    "projectile_pending_hit_retained_bytes",
    "projectile_pending_damage_transactions",
    "projectile_pending_damage_payload_bytes",
  ]) {
    assert.throws(
      () => assertRtcPressureClean({ ...cleanStatus(), [field]: 1 }, field),
      /pressure audit failed/,
    );
  }
  assert.throws(
    () => assertRtcPressureClean({ ...cleanStatus(), realtime_event_queue_capacity_per_client: 3071 }, "capacity"),
    /pressure audit failed/,
  );
  assert.equal(rtcPressureSummary(cleanStatus()).applicationPeakQueuePackets.reliable, 4);
});

test("RTC pressure audit fails closed on missing or malformed source metrics", () => {
  for (const status of [null, undefined, [], "status"]) {
    assert.throws(() => rtcPressureSummary(status), /requires a status object/);
  }
  for (const field of [
    "rtc_inbound_packets",
    "rtc_inbound_queue_packets",
    "rtc_inbound_peak_queue_packets",
    "rtc_inbound_backpressure",
  ]) {
    for (const replacement of [undefined, null, [], 0]) {
      const status = cleanStatus();
      if (replacement === undefined) delete status[field];
      else status[field] = replacement;
      assert.throws(() => rtcPressureSummary(status), new RegExp(field));
    }
    for (const mutate of [
      (values) => { delete values.reliable; },
      (values) => { values.unknown = 0; },
      (values) => { values.game = null; },
      (values) => { values.game = "0"; },
      (values) => { values.game = Number.NaN; },
      (values) => { values.game = Number.POSITIVE_INFINITY; },
      (values) => { values.game = -1; },
      (values) => { values.game = 0.5; },
    ]) {
      const status = cleanStatus();
      mutate(status[field]);
      assert.throws(() => rtcPressureSummary(status), new RegExp(field));
    }
  }
  for (const field of [
    "position_input_queue_capacity_rejections",
    "realtime_event_queue_capacity_per_client",
    "realtime_event_queue_current",
    "realtime_event_queue_peak",
    "realtime_event_queue_overflows",
    "realtime_event_actions_rejected",
    "realtime_event_actions_deferred",
    "projectile_pending_hit_capacity_total",
    "projectile_pending_hit_capacity_per_owner",
    "projectile_pending_hit_retry_budget_per_tick",
    "projectile_pending_hit_current",
    "projectile_pending_hit_retained_bytes",
    "projectile_pending_damage_transactions",
    "projectile_pending_damage_payload_bytes",
    "rtc_outbound_reliable_overflows",
    "rtc_outbound_queue_packets",
    "rtc_outbound_queue_bytes",
  ]) {
    for (const replacement of [undefined, null, "0", Number.NaN, Number.POSITIVE_INFINITY, -1, 0.5]) {
      const status = cleanStatus();
      if (replacement === undefined) delete status[field];
      else status[field] = replacement;
      assert.throws(() => rtcPressureSummary(status), new RegExp(field));
    }
  }
});

test("RTC log audit maps driver Full to the announced channel and fails closed on rejects", () => {
  const logs = [
    'server rtc data channel announced channel_id=5 label="zubspace-reliable"\n',
    "Failed to send DataChannelMessage to data channel 5: Full\n",
    'server security reject player_id=7 category="position" reason="forged"\n',
    "server RTC application ingress queue overflowed; disconnecting peer\n",
  ];
  const audit = rtcServerLogAudit(logs);
  assert.equal(audit.driverFull.reliable, 1);
  assert.equal(audit.driverFull.total, 1);
  assert.equal(audit.structuredSecurityRejectCount, 1);
  assert.equal(audit.applicationQueueFailureCount, 1);
  assert.throws(() => assertRtcServerLogClean(logs, "fixture"), /log audit failed/);
  assert.deepEqual(assertRtcServerLogClean([], "empty").driverFull, {
    game: 0,
    event: 0,
    reliable: 0,
    unknown: 0,
    total: 0,
  });
});

test("container log audit requires native startup and an actual RTC session lifecycle", () => {
  const options = { requireRuntimeEvidence: true };
  const startup = "[zubspace-server] | INFO zubspace server started\n";
  const sessions = ["server rtc player connected", "server rtc player disconnected"].map(message =>
    `[zubspace-server] | INFO ${message} player_id=0\n`);
  assert.throws(() => assertRtcServerLogClean([], "empty", options), /evidence missing/);
  assert.throws(() => assertRtcServerLogClean('[zubspace-client] | GET /v1/status 200\n', "nginx only", options), /evidence missing/);
  assert.throws(() => assertRtcServerLogClean(sessions, "no startup", options), /server started/);
  for (let i = 0; i < sessions.length; ++i) {
    assert.throws(() => assertRtcServerLogClean([startup, ...sessions.filter((_, j) => j !== i)], "missing lifecycle event", options), /evidence missing/);
  }
  assert.equal(assertRtcServerLogClean([startup, ...sessions], "complete", options).driverFull.total, 0);
  assert.throws(() => assertRtcServerLogClean([startup, ...sessions, 'server security reject category="position"\n'], "reject", options), /log audit failed/);
});
