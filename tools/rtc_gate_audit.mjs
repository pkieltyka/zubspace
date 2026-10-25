import { serverLogField, serverLogMessage } from "./server_log_fields.mjs";
const RTC_CHANNELS = ["game", "event", "reliable"];
const RTC_CHANNEL_METRICS = [
  "rtc_inbound_packets",
  "rtc_inbound_queue_packets",
  "rtc_inbound_peak_queue_packets",
  "rtc_inbound_backpressure",
];
const RTC_SCALAR_METRICS = [
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
];

function parseTextField(line, field) {
  const value = serverLogField(line, field);
  if (value !== undefined) return ["string", "number", "boolean"].includes(typeof value) ? String(value) : null;
  line = serverLogMessage(line);
  const quoted = line.match(new RegExp(`${field}="([^"]*)"`));
  if (quoted) return quoted[1];
  return line.match(new RegExp(`${field}=([^\\s]+)`))?.[1] || null;
}

function parseNumericField(line, field) {
  const value = parseTextField(line, field);
  if (value == null || !/^-?\d+$/.test(value)) return null;
  return Number(value);
}

function assertMetric(value, label) {
  if (!Number.isFinite(value) || !Number.isInteger(value) || value < 0) {
    throw new Error(`${label} must be a finite nonnegative integer`);
  }
  return value;
}

function exactChannelMetric(status, field) {
  const values = status[field];
  if (!values || typeof values !== "object" || Array.isArray(values)) {
    throw new Error(`RTC status ${field} must be an object`);
  }
  const keys = Object.keys(values).sort();
  if (JSON.stringify(keys) !== JSON.stringify([...RTC_CHANNELS].sort())) {
    throw new Error(
      `RTC status ${field} must contain exactly game/event/reliable: ${JSON.stringify(keys)}`,
    );
  }
  return Object.fromEntries(RTC_CHANNELS.map((channel) => [
    channel,
    assertMetric(values[channel], `RTC status ${field}.${channel}`),
  ]));
}

function ingressExecutionSummary(status) {
  const value = status.rtc_ingress_execution;
  const fields = ["limit", "workers", "in_flight", "waiting"];
  if (!value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).sort().join() !== [...fields].sort().join() ||
      fields.some(field => !Number.isSafeInteger(value[field]) || value[field] < 0) ||
      value.workers !== value.limit || value.in_flight > value.limit ||
      (value.limit === 0 && value.waiting !== 0)) {
    throw new Error("RTC ingress execution status is missing, malformed or inconsistent");
  }
  return Object.fromEntries(fields.map(field => [field, value[field]]));
}

function rtcPressureSummary(status) {
  if (!status || typeof status !== "object" || Array.isArray(status)) {
    throw new Error("RTC pressure audit requires a status object");
  }
  const sourceStatus = Object.fromEntries([
    ["rtc_ingress_execution", ingressExecutionSummary(status)],
    ...RTC_CHANNEL_METRICS.map((field) => [field, exactChannelMetric(status, field)]),
    ...RTC_SCALAR_METRICS.map((field) => [
      field,
      assertMetric(status[field], `RTC status ${field}`),
    ]),
  ]);
  return {
    sourceStatus,
    ingressExecution: {...sourceStatus.rtc_ingress_execution},
    inboundPackets: { ...sourceStatus.rtc_inbound_packets },
    applicationQueuePackets: { ...sourceStatus.rtc_inbound_queue_packets },
    applicationPeakQueuePackets: { ...sourceStatus.rtc_inbound_peak_queue_packets },
    applicationBackpressure: { ...sourceStatus.rtc_inbound_backpressure },
    positionInputQueueCapacityRejections:
      sourceStatus.position_input_queue_capacity_rejections,
    realtimeEventQueueCapacityPerClient:
      sourceStatus.realtime_event_queue_capacity_per_client,
    realtimeEventQueueCurrent: sourceStatus.realtime_event_queue_current,
    realtimeEventQueuePeak: sourceStatus.realtime_event_queue_peak,
    realtimeEventQueueOverflows: sourceStatus.realtime_event_queue_overflows,
    realtimeEventActionsRejected: sourceStatus.realtime_event_actions_rejected,
    realtimeEventActionsDeferred: sourceStatus.realtime_event_actions_deferred,
    projectilePendingHitCapacityTotal:
      sourceStatus.projectile_pending_hit_capacity_total,
    projectilePendingHitCapacityPerOwner:
      sourceStatus.projectile_pending_hit_capacity_per_owner,
    projectilePendingHitRetryBudgetPerTick:
      sourceStatus.projectile_pending_hit_retry_budget_per_tick,
    projectilePendingHitCurrent: sourceStatus.projectile_pending_hit_current,
    projectilePendingHitRetainedBytes:
      sourceStatus.projectile_pending_hit_retained_bytes,
    projectilePendingDamageTransactions:
      sourceStatus.projectile_pending_damage_transactions,
    projectilePendingDamagePayloadBytes:
      sourceStatus.projectile_pending_damage_payload_bytes,
    outboundReliableOverflows: sourceStatus.rtc_outbound_reliable_overflows,
    outboundQueuePackets: sourceStatus.rtc_outbound_queue_packets,
    outboundQueueBytes: sourceStatus.rtc_outbound_queue_bytes,
  };
}

function assertRtcPressureClean(status, phase, {
  requireQueuesDrained = true, requireIngressDrained = false,
} = {}) {
  const summary = rtcPressureSummary(status);
  const nonzeroBackpressure = RTC_CHANNELS.filter(
    (channel) => summary.applicationBackpressure[channel] !== 0,
  );
  const queued = RTC_CHANNELS.filter(
    (channel) => summary.applicationQueuePackets[channel] !== 0,
  );
  if (
    (requireIngressDrained && (summary.ingressExecution.in_flight !== 0 ||
      summary.ingressExecution.waiting !== 0)) ||
    nonzeroBackpressure.length > 0 ||
    (requireQueuesDrained && queued.length > 0) ||
    summary.positionInputQueueCapacityRejections !== 0 ||
    summary.realtimeEventQueueCapacityPerClient < 3_072 ||
    (requireQueuesDrained && summary.realtimeEventQueueCurrent !== 0) ||
    summary.realtimeEventQueueOverflows !== 0 ||
    summary.realtimeEventActionsRejected !== 0 ||
    summary.realtimeEventActionsDeferred !== 0 ||
    summary.projectilePendingHitCapacityTotal !== 3_072 ||
    summary.projectilePendingHitCapacityPerOwner !== 128 ||
    summary.projectilePendingHitRetryBudgetPerTick !== 256 ||
    (requireQueuesDrained && summary.projectilePendingHitCurrent !== 0) ||
    (requireQueuesDrained && summary.projectilePendingHitRetainedBytes !== 0) ||
    (requireQueuesDrained && summary.projectilePendingDamageTransactions !== 0) ||
    (requireQueuesDrained && summary.projectilePendingDamagePayloadBytes !== 0) ||
    summary.outboundReliableOverflows !== 0 ||
    summary.outboundQueuePackets !== 0 ||
    summary.outboundQueueBytes !== 0
  ) {
    throw new Error(`RTC pressure audit failed at ${phase}: ${JSON.stringify({
      summary,
      nonzeroBackpressure,
      queued,
    })}`);
  }
  return summary;
}

function assertRtcPressureSummaryClean(summary, phase, options = {}) {
  if (!summary || typeof summary !== "object" || Array.isArray(summary)) {
    throw new Error(`RTC pressure summary at ${phase} must be an object`);
  }
  const expected = assertRtcPressureClean(summary.sourceStatus, phase, options);
  const expectedKeys = Object.keys(expected).sort();
  const actualKeys = Object.keys(summary).sort();
  if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
    throw new Error(
      `RTC pressure summary at ${phase} must contain exactly ${expectedKeys.join("/")}: ` +
        JSON.stringify(actualKeys),
    );
  }
  for (const key of expectedKeys) {
    if (JSON.stringify(summary[key]) !== JSON.stringify(expected[key])) {
      throw new Error(
        `RTC pressure summary at ${phase} diverges from source status for ${key}`,
      );
    }
  }
  return expected;
}

function rtcServerLogAudit(logs) {
  const text = Array.isArray(logs) ? logs.join("") : String(logs || "");
  const lines = text.split(/\r?\n/);
  const labels = new Map();
  for (const line of lines) {
    if (!line.includes("server rtc data channel announced")) continue;
    const id = parseNumericField(line, "channel_id");
    const label = parseTextField(line, "label");
    if (Number.isInteger(id) && label) labels.set(id, label);
  }
  const driverFull = { game: 0, event: 0, reliable: 0, unknown: 0, total: 0 };
  const securityRejects = [];
  const queueFailures = [];
  for (const line of lines) {
    const full = line.match(/Failed to send DataChannelMessage to data channel (\d+): Full/);
    if (full) {
      const label = labels.get(Number(full[1]));
      const channel = label === "zubspace-game"
        ? "game"
        : label === "zubspace-events"
          ? "event"
          : label === "zubspace-reliable"
            ? "reliable"
            : "unknown";
      driverFull[channel] += 1;
      driverFull.total += 1;
    }
    if (line.includes("server security reject") && !line.includes("security reject summary")) {
      securityRejects.push(line);
    }
    if (
      line.includes("RTC application ingress queue overflowed") ||
      line.includes("RTC reliable outbound queue overflowed")
    ) {
      queueFailures.push(line);
    }
  }
  return {
    driverFull,
    structuredSecurityRejectCount: securityRejects.length,
    structuredSecurityRejectLines: securityRejects,
    applicationQueueFailureCount: queueFailures.length,
    applicationQueueFailureLines: queueFailures,
  };
}

function assertRtcServerLogClean(logs, phase, { requireRuntimeEvidence = false } = {}) {
  if (requireRuntimeEvidence) {
    const text = Array.isArray(logs) ? logs.join("") : String(logs || "");
    const lines = text.split(/\r?\n/);
    // These are INFO events under the shipped container filter. Per-channel
    // announcements are DEBUG; the browser/status checks prove those channels.
    const missing = ["zubspace server started", "server rtc player connected", "server rtc player disconnected"]
      .filter(message => !lines.some(line => line.includes(message)));
    if (missing.length) {
      throw new Error(`RTC server log evidence missing at ${phase}: ${missing.join(", ")}`);
    }
  }
  const audit = rtcServerLogAudit(logs);
  if (
    audit.driverFull.total !== 0 ||
    audit.driverFull.unknown !== 0 ||
    audit.structuredSecurityRejectCount !== 0 ||
    audit.applicationQueueFailureCount !== 0
  ) {
    throw new Error(`RTC server log audit failed at ${phase}: ${JSON.stringify(audit)}`);
  }
  return audit;
}

export {
  RTC_CHANNELS,
  assertRtcPressureClean,
  assertRtcPressureSummaryClean,
  assertRtcServerLogClean,
  rtcPressureSummary,
  rtcServerLogAudit,
};
