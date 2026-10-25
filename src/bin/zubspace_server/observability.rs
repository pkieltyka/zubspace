//! Bounded, diagnostic-only production telemetry. Reports cannot affect authority.
use crate::authority_identity::PlayerSessionIdentity;
use crate::state::{ServerState, timing::TimingHistogram};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const PLAYER_REPORT_TYPE: u8 = 0x80;
const REPORT_LEN: usize = 54;
/// Version 2 appends the game channel's peak send queue and dropped sends.
const REPORT_V2_LEN: usize = 62;
const MAX_REPORTED_BUFFERED_PEAK: u32 = 16 * 1024 * 1024;
const MAX_REPORTED_DROPPED_SENDS: u32 = 10_000;

/// Fixed-size diagnostic pacing, separate from gameplay clocks/authority.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DiagnosticRateGate {
    last: Option<(u32, Instant)>,
}

impl DiagnosticRateGate {
    pub(crate) fn admit(&mut self, scope: u32, now: Instant, interval: Duration) -> bool {
        if self.last.is_some_and(|(previous_scope, previous)| {
            previous_scope == scope
                && now
                    .checked_duration_since(previous)
                    .is_none_or(|elapsed| elapsed < interval)
        }) {
            return false;
        }
        self.last = Some((scope, now));
        true
    }
}

pub(crate) const TICK_PHASE_METRICS: [&str; 5] = [
    "zubspace_tick_admission_seconds",
    "zubspace_tick_bots_seconds",
    "zubspace_tick_world_and_publish_seconds",
    "zubspace_tick_snapshots_seconds",
    "zubspace_tick_finish_seconds",
];

#[derive(Default)]
pub(crate) struct Telemetry {
    pub(crate) zone_files_sent: u64,
    pub(crate) zone_files_held: u64,
    pub(crate) zone_bytes_sent: u64,
    pub(crate) zone_bytes_held: u64,
    pub(crate) client_debug: crate::client_debug::ClientDebugMetrics,
    pub(crate) snapshot_ack: TimingHistogram,
    snapshot_full_bytes: TimingHistogram,
    snapshot_delta_bytes: TimingHistogram,
    snapshots_paced_out: u64,
    realtime_events_paced_out: u64,
    realtime_events_sent: [u64; REALTIME_EVENT_KINDS.len()],
    snapshot_entities_held: u64,
    snapshots_with_holds: u64,
    pub(crate) tick_phases: [TimingHistogram; 5],
    tick_phase_log_gate: DiagnosticRateGate,
    tick_phase_logs_suppressed: u64,
    browser_ice_rtt: TimingHistogram,
    browser_frame_mean: TimingHistogram,
    browser_frame_max: TimingHistogram,
    browser_game_buffered_peak: TimingHistogram,
    browser_game_sends_dropped: u64,
    pub(crate) tick_overruns: u64,
    pub(crate) sessions_started: u64,
    pub(crate) sessions_ended: u64,
    reports: u64,
    frames: u64,
    slow_frames: u64,
    unready_frames: u64,
    corrections: u64,
    last_report: HashMap<PlayerSessionIdentity, Instant>,
}

/// What one tick's snapshot emission sent, gathered while each recipient's
/// client handle is borrowed and recorded into [`Telemetry`] afterwards.
#[derive(Default)]
pub(crate) struct SnapshotEmission {
    sent: Vec<(bool, usize)>,
    snapshots_paced_out: u64,
    realtime_events_paced_out: u64,
    realtime_events_sent: [u64; REALTIME_EVENT_KINDS.len()],
    snapshot_entities_held: u64,
    snapshots_with_holds: u64,
}

/// `0x7d` kinds 1-5, in order, as metric labels.
const REALTIME_EVENT_KINDS: [&str; 5] = [
    "weapon",
    "watch_damage",
    "repel_mutations",
    "projectile_state",
    "owner_weapons_cancelled",
];

impl SnapshotEmission {
    pub(crate) fn sent(&mut self, full: bool, bytes: usize) {
        self.sent.push((full, bytes));
    }

    /// The per-client send budget was spent, so the snapshot was skipped.
    pub(crate) fn snapshot_paced_out(&mut self) {
        self.snapshots_paced_out += 1;
    }

    /// The per-client send budget was spent, so an event waits for a later tick.
    pub(crate) fn realtime_event_paced_out(&mut self) {
        self.realtime_events_paced_out += 1;
    }

    /// A sent snapshot held this many listed actors at their acknowledged
    /// state to fit the send budget.
    pub(crate) fn entities_held(&mut self, held: usize) {
        if held > 0 {
            self.snapshot_entities_held += held as u64;
            self.snapshots_with_holds += 1;
        }
    }

    /// One `0x7d` event of this kind was queued for its recipient.
    pub(crate) fn realtime_event_sent(&mut self, kind: u8) {
        if let Some(count) = self
            .realtime_events_sent
            .get_mut(usize::from(kind).wrapping_sub(1))
        {
            *count += 1;
        }
    }
}

impl Telemetry {
    pub(crate) fn record_snapshot_emission(&mut self, emission: SnapshotEmission) {
        for (full, bytes) in emission.sent {
            if full {
                self.snapshot_full_bytes.record(bytes as u64);
            } else {
                self.snapshot_delta_bytes.record(bytes as u64);
            }
        }
        self.snapshots_paced_out = self
            .snapshots_paced_out
            .saturating_add(emission.snapshots_paced_out);
        self.realtime_events_paced_out = self
            .realtime_events_paced_out
            .saturating_add(emission.realtime_events_paced_out);
        for (total, sent) in self
            .realtime_events_sent
            .iter_mut()
            .zip(emission.realtime_events_sent)
        {
            *total = total.saturating_add(sent);
        }
        self.snapshot_entities_held = self
            .snapshot_entities_held
            .saturating_add(emission.snapshot_entities_held);
        self.snapshots_with_holds = self
            .snapshots_with_holds
            .saturating_add(emission.snapshots_with_holds);
    }

    pub(crate) fn record_tick_phases(
        &mut self,
        phases: [u64; 5],
        budget_us: f64,
        now: Instant,
    ) -> bool {
        for (distribution, sample) in self.tick_phases.iter_mut().zip(phases) {
            distribution.record(sample);
        }
        let total = phases.into_iter().fold(0_u64, u64::saturating_add);
        if (total as f64) < budget_us {
            return false;
        }
        if self
            .tick_phase_log_gate
            .admit(0, now, Duration::from_secs(1))
        {
            return true;
        }
        self.tick_phase_logs_suppressed = self.tick_phase_logs_suppressed.saturating_add(1);
        false
    }
}

pub(crate) fn receive_player_report(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    packet: &[u8],
) {
    // Browsers still running an older build keep sending version 1.
    let version_length = match packet.get(1) {
        Some(1) => REPORT_LEN,
        Some(2) => REPORT_V2_LEN,
        _ => return,
    };
    if packet.len() != version_length || packet[0] != PLAYER_REPORT_TYPE {
        return;
    }
    let u64_at = |offset| u64::from_le_bytes(packet[offset..offset + 8].try_into().unwrap());
    let u32_at = |offset| u32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap());
    let fields: [u32; 8] = std::array::from_fn(|index| u32_at(22 + index * 4));
    let [
        frames,
        slow,
        sum_us,
        max_us,
        ice_us,
        corrections,
        unready,
        window_us,
    ] = fields;
    // A report is a visible-page window, not authoritative game state. Bound every
    // contribution and reject malformed populations before touching aggregate counters.
    if frames == 0
        || frames > 10_000
        || slow > frames
        || unready > frames
        || corrections > 10_000
        || !(1_000_000..=30_000_000).contains(&window_us)
        || sum_us > 30_000_000
        || max_us > sum_us
        || u64::from(sum_us) > u64::from(frames) * u64::from(max_us)
        || (ice_us != u32::MAX && ice_us > 10_000_000)
    {
        return;
    }
    // (peak send queue bytes, sends dropped at the page's limit), version 2 only.
    let game_sending = (packet.len() == REPORT_V2_LEN).then(|| (u32_at(54), u32_at(58)));
    if game_sending.is_some_and(|(peak, dropped)| {
        peak > MAX_REPORTED_BUFFERED_PEAK || dropped > MAX_REPORTED_DROPPED_SENDS
    }) {
        return;
    }
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(client) = state.clients.get(&identity.player_id) else {
        return;
    };
    if !client.joined
        || client.session_id != identity.session_id
        || client.last_security_valid != Some(true)
        || u64_at(2) != crate::wire::server_session_id()
        || u64_at(10) != client.arena_stream_id
        || u32_at(18) != client.life_seq
    {
        return;
    }
    let now = Instant::now();
    if state
        .telemetry
        .last_report
        .get(&identity)
        .is_some_and(|last| now.duration_since(*last) < Duration::from_secs(2))
    {
        return;
    }
    let t = &mut state.telemetry;
    t.last_report.insert(identity, now);
    t.reports += 1;
    t.frames += u64::from(frames);
    t.slow_frames += u64::from(slow);
    t.unready_frames += u64::from(unready);
    t.corrections += u64::from(corrections);
    t.browser_frame_mean.record(u64::from(sum_us / frames));
    t.browser_frame_max.record(u64::from(max_us));
    if ice_us != u32::MAX {
        t.browser_ice_rtt.record(u64::from(ice_us));
    }
    if let Some((peak, dropped)) = game_sending {
        t.browser_game_buffered_peak.record(u64::from(peak));
        t.browser_game_sends_dropped = t
            .browser_game_sends_dropped
            .saturating_add(u64::from(dropped));
    }
    tracing::debug!(event = "player_experience_window", player_id = identity.player_id,
        session_id = ?identity.session_id, arena_stream_id = u64_at(10), life_seq = u32_at(18),
        frames, slow_frames = slow, frame_mean_us = sum_us / frames, frame_max_us = max_us,
        ice_rtt_us = (ice_us != u32::MAX).then_some(ice_us), corrections, unready_frames = unready,
        game_buffered_peak_bytes = game_sending.map(|(peak, _)| peak),
        game_sends_dropped = game_sending.map(|(_, dropped)| dropped),
        "player experience report");
}

pub(crate) fn retire_session(state: &mut ServerState, identity: PlayerSessionIdentity) {
    state.telemetry.last_report.remove(&identity);
}

pub(crate) fn metrics(state: &ServerState, started_at: Instant) -> String {
    let mut out = String::with_capacity(12_000);
    fn scalar(out: &mut String, name: &str, help: &str, kind: &str, value: impl std::fmt::Display) {
        let _ = writeln!(
            out,
            "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}"
        );
    }
    scalar(
        &mut out,
        "zubspace_uptime_seconds",
        "Process uptime.",
        "gauge",
        started_at.elapsed().as_secs_f64(),
    );
    scalar(
        &mut out,
        "zubspace_sessions",
        "Current RTC application sessions.",
        "gauge",
        state.clients.len(),
    );
    scalar(
        &mut out,
        "zubspace_players",
        "Joined human players and bots, including observers.",
        "gauge",
        state.clients.values().filter(|c| c.joined).count() + state.bots.len(),
    );
    scalar(
        &mut out,
        "zubspace_tick_budget_seconds",
        "Configured tick work budget.",
        "gauge",
        1.0 / state.config.tick_rate_hz.max(1) as f64,
    );
    for (name, distribution) in TICK_PHASE_METRICS
        .into_iter()
        .zip(&state.telemetry.tick_phases)
    {
        distribution.prometheus(
            name,
            "Exclusive server tick phase wall time, including scheduling and lock waits.",
            &mut out,
        );
    }
    scalar(
        &mut out,
        "zubspace_tick_phase_logs_suppressed_total",
        "Over-budget phase logs suppressed by the one-per-second diagnostic bound.",
        "counter",
        state.telemetry.tick_phase_logs_suppressed,
    );
    for (name, help, total) in [
        (
            "zubspace_rtc_udp_datagrams_sent_total",
            "UDP datagrams sent on selected candidate pairs, STUN excluded.",
            &crate::rtc::RTC_DATAGRAMS.sent,
        ),
        (
            "zubspace_rtc_udp_datagrams_received_total",
            "UDP datagrams received on selected candidate pairs, STUN excluded.",
            &crate::rtc::RTC_DATAGRAMS.received,
        ),
        (
            "zubspace_rtc_udp_bytes_sent_total",
            "Bytes above UDP sent on selected candidate pairs: DataChannel payload plus DTLS and SCTP framing.",
            &crate::rtc::RTC_DATAGRAMS.bytes_sent,
        ),
        (
            "zubspace_rtc_udp_bytes_received_total",
            "Bytes above UDP received on selected candidate pairs: DataChannel payload plus DTLS and SCTP framing.",
            &crate::rtc::RTC_DATAGRAMS.bytes_received,
        ),
    ] {
        scalar(
            &mut out,
            name,
            help,
            "counter",
            total.load(std::sync::atomic::Ordering::Relaxed),
        );
    }
    scalar(
        &mut out,
        "zubspace_log_lines_dropped_total",
        "Log lines dropped because the log writer queue was full.",
        "counter",
        crate::log_queue::dropped_lines(),
    );
    let quality = crate::connection_quality::summary(state);
    for (suffix, field, help) in [
        (
            "sessions",
            "sessions",
            "Current RTC sessions with an independent quality monitor.",
        ),
        (
            "measured_sessions",
            "measured_sessions",
            "Current sessions with sufficient mature probe samples.",
        ),
        (
            "dispatch_started",
            "dispatch_started",
            "Probe dispatch attempts in current session generations; not a lifetime counter.",
        ),
        (
            "echoes_received",
            "echoes_received",
            "Validated distinct echoes in current session generations.",
        ),
        (
            "local_dispatch_cancelled",
            "local_dispatch_cancelled",
            "Local cancellations in current session generations, excluded from path loss.",
        ),
        (
            "samples",
            "samples",
            "Mature probe outcomes retained in current session windows.",
        ),
        (
            "max_session_average_rtt_ms",
            "max_session_average_rtt_ms",
            "Maximum measured session mean round-trip RTT in milliseconds, including browser processing.",
        ),
        (
            "max_session_round_trip_probe_loss",
            "max_session_round_trip_probe_loss",
            "Maximum measured session missing-echo fraction; round-trip delivery proxy, not directional UDP loss.",
        ),
        (
            "sustained_degraded_sessions",
            "sustained_degraded_sessions",
            "Current sufficiently sampled sessions meeting the sustained degradation threshold.",
        ),
        (
            "retained_holds",
            "retained_holds",
            "Reconnect-persistent quality holds awaiting a healthy return.",
        ),
        (
            "held_sessions",
            "held_sessions",
            "Current sessions whose identity has a quality hold.",
        ),
    ] {
        scalar(
            &mut out,
            &format!("zubspace_connection_quality_{suffix}"),
            help,
            "gauge",
            &quality[field],
        );
    }
    for (suffix, help) in [
        (
            "forced_total",
            "Committed connection-quality forced spectator transitions.",
        ),
        (
            "return_denied_total",
            "Ship returns rejected by the connection-quality cooldown or recovery check.",
        ),
        (
            "healthy_return_total",
            "Committed healthy returns after a connection-quality hold.",
        ),
    ] {
        scalar(
            &mut out,
            &format!("zubspace_connection_quality_{suffix}"),
            help,
            "counter",
            &quality[suffix],
        );
    }
    scalar(
        &mut out,
        "zubspace_tick_work_ewma_seconds",
        "Smoothed authoritative tick work.",
        "gauge",
        state.tick_wallclock_ewma_us / 1e6,
    );
    state.tick_work.prometheus(
        "zubspace_tick_work_seconds",
        "Authoritative tick work including state lock wait, excluding post-tick broadcasts.",
        &mut out,
    );
    state.tick_intervals.prometheus(
        "zubspace_tick_interval_seconds",
        "Wall time between authoritative tick starts.",
        &mut out,
    );
    state.tick_jitter.prometheus(
        "zubspace_tick_absolute_jitter_seconds",
        "Absolute deviation from configured tick interval.",
        &mut out,
    );
    let t = &state.telemetry;
    for (name, help, value) in [
        (
            "zubspace_client_debug_browser_errors_total",
            "Reported exception/rejection kinds; no error text or stack is collected.",
            t.client_debug.browser_errors,
        ),
        (
            "zubspace_client_debug_context_losses_total",
            "Reported WebGL context loss events.",
            t.client_debug.context_losses,
        ),
        (
            "zubspace_client_debug_context_restorations_total",
            "Reported WebGL context restoration events.",
            t.client_debug.context_restorations,
        ),
    ] {
        scalar(&mut out, name, help, "counter", value);
    }
    for (name, help, kind, value) in [
        (
            "zubspace_client_debug_reports_total",
            "Accepted opt-in client presentation reports; untrusted diagnostics.",
            "counter",
            t.client_debug.reports,
        ),
        (
            "zubspace_client_debug_frames_total",
            "Retained frames in accepted client diagnostic reports.",
            "counter",
            t.client_debug.frames,
        ),
        (
            "zubspace_client_debug_dropped_frames_total",
            "Client reported ring/size evictions; does not include reports that never arrive.",
            "counter",
            t.client_debug.dropped_frames,
        ),
        (
            "zubspace_client_debug_rejections_total",
            "Authenticated malformed, duplicate or rate-limited reports.",
            "counter",
            t.client_debug.rejected,
        ),
        (
            "zubspace_client_debug_log_failures_total",
            "Accepted diagnostic batches that failed local log output.",
            "counter",
            t.client_debug.log_failures,
        ),
    ] {
        scalar(&mut out, name, help, kind, value);
    }
    scalar(
        &mut out,
        "zubspace_client_debug_follow_camera_error_pixels_max",
        "Maximum reported camera-to-followed-render-position distance; untrusted, clamped at one million pixels.",
        "gauge",
        t.client_debug.max_follow_error_millipixels as f64 / 1000.0,
    );
    scalar(
        &mut out,
        "zubspace_tick_overruns_total",
        "Completed ticks meeting or exceeding the work budget.",
        "counter",
        t.tick_overruns,
    );
    for (name, help, value) in [
        (
            "zubspace_zone_files_sent_total",
            "Completed zone-file transfers, including redrives; failed or partial transfers are excluded.",
            t.zone_files_sent,
        ),
        (
            "zubspace_zone_files_held_total",
            "Authenticated held-file claims accepted once per file index per join; possession is challenged separately for maps.",
            t.zone_files_held,
        ),
        (
            "zubspace_zone_bytes_sent_total",
            "Logical file bytes in completed transfers, including redrives; compressed maps and raw LVZ, excluding framing.",
            t.zone_bytes_sent,
        ),
        (
            "zubspace_zone_bytes_held_total",
            "Logical transfer bytes avoided by held-file claims; compressed maps and raw LVZ, excluding framing.",
            t.zone_bytes_held,
        ),
    ] {
        scalar(&mut out, name, help, "counter", value);
    }
    scalar(
        &mut out,
        "zubspace_sessions_started_total",
        "Registered application sessions.",
        "counter",
        t.sessions_started,
    );
    scalar(
        &mut out,
        "zubspace_sessions_ended_total",
        "Retired application sessions.",
        "counter",
        t.sessions_ended,
    );
    t.snapshot_full_bytes.prometheus_scaled(
        "zubspace_snapshot_full_bytes",
        "Size of each full snapshot sent, in bytes.",
        &mut out,
        1.0,
    );
    t.snapshot_delta_bytes.prometheus_scaled(
        "zubspace_snapshot_delta_bytes",
        "Size of each delta snapshot sent, in bytes.",
        &mut out,
        1.0,
    );
    scalar(
        &mut out,
        "zubspace_snapshot_send_skipped_total",
        "Snapshots skipped because the recipient's send budget was spent.",
        "counter",
        t.snapshots_paced_out,
    );
    scalar(
        &mut out,
        "zubspace_realtime_event_send_deferred_total",
        "Realtime events held for a later tick because the recipient's send budget was spent.",
        "counter",
        t.realtime_events_paced_out,
    );
    scalar(
        &mut out,
        "zubspace_snapshots_with_holds_total",
        "Snapshots that held some listed actors at their acknowledged state to fit the send budget, instead of being skipped.",
        "counter",
        t.snapshots_with_holds,
    );
    scalar(
        &mut out,
        "zubspace_snapshot_entities_held_total",
        "Listed actors held at their acknowledged state in sent snapshots to fit the send budget.",
        "counter",
        t.snapshot_entities_held,
    );
    out.push_str("# HELP zubspace_realtime_events_sent_total Realtime 0x7d events queued for recipients, by kind; batching and the game-channel copy do not change the count.\n# TYPE zubspace_realtime_events_sent_total counter\n");
    for (kind, sent) in REALTIME_EVENT_KINDS.iter().zip(t.realtime_events_sent) {
        let _ = writeln!(
            out,
            "zubspace_realtime_events_sent_total{{kind=\"{kind}\"}} {sent}"
        );
    }
    out.push_str("# HELP zubspace_realtime_event_game_copies_total Realtime event messages also sent on the game channel, or not sent there because the recipient's link showed no loss.\n# TYPE zubspace_realtime_event_game_copies_total counter\n");
    for (decision, total) in [
        ("sent", &crate::rtc::REALTIME_EVENT_GAME_COPIES.sent),
        ("skipped", &crate::rtc::REALTIME_EVENT_GAME_COPIES.skipped),
    ] {
        let _ = writeln!(
            out,
            "zubspace_realtime_event_game_copies_total{{decision=\"{decision}\"}} {}",
            total.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
    t.snapshot_ack.prometheus("zubspace_snapshot_ack_round_trip_seconds", "Snapshot emission to received codec acknowledgement; includes network and browser scheduling, not application receipt or input-to-photon latency.", &mut out);
    t.browser_ice_rtt.prometheus(
        "zubspace_browser_ice_round_trip_seconds",
        "Browser reported selected ICE pair RTT; untrusted diagnostic samples.",
        &mut out,
    );
    t.browser_frame_mean.prometheus(
        "zubspace_browser_frame_window_mean_seconds",
        "Mean visible render frame interval per reported window; not per-frame quantiles.",
        &mut out,
    );
    t.browser_frame_max.prometheus(
        "zubspace_browser_frame_window_max_seconds",
        "Maximum visible render frame interval per reported window.",
        &mut out,
    );
    t.browser_game_buffered_peak.prometheus_scaled(
        "zubspace_browser_game_buffered_peak_bytes",
        "Largest game-channel send queue (bufferedAmount) per reported window, in bytes.",
        &mut out,
        1.0,
    );
    for (name, help, value) in [
        (
            "zubspace_browser_reports_total",
            "Accepted bounded browser diagnostic windows.",
            t.reports,
        ),
        (
            "zubspace_browser_frames_total",
            "Reported visible render intervals.",
            t.frames,
        ),
        (
            "zubspace_browser_slow_frames_total",
            "Reported visible render intervals over 50 ms.",
            t.slow_frames,
        ),
        (
            "zubspace_browser_unready_frames_total",
            "Reported frames with closed gameplay readiness; includes lifecycle transitions and spectators.",
            t.unready_frames,
        ),
        (
            "zubspace_browser_corrections_total",
            "Reported local self correction increments.",
            t.corrections,
        ),
        (
            "zubspace_browser_game_sends_dropped_total",
            "Reported game-channel sends the page dropped because its send queue was full.",
            t.browser_game_sends_dropped,
        ),
        (
            "zubspace_movement_snapbacks_total",
            "Server issued sustained movement drift corrections.",
            state.movement_snapbacks_total,
        ),
    ] {
        scalar(&mut out, name, help, "counter", value);
    }
    let now = Instant::now();
    let mut backlog = crate::state::RtcLaneBacklog::default();
    for client in state.clients.values() {
        if let crate::state::ClientPacketSender::Bounded(sink) = &client.tx {
            let sample = sink.rtc_lane_backlog(now);
            for index in 0..3 {
                backlog.packets[index] += sample.packets[index];
                backlog.bytes[index] += sample.bytes[index];
                backlog.oldest_age_us[index] =
                    backlog.oldest_age_us[index].max(sample.oldest_age_us[index]);
            }
        }
    }
    let rtc = state.rtc_overload_metrics.snapshot();
    out.push_str("# HELP zubspace_observer_auto_targets Connections with each currently offered automatic presentation target; not an application receipt.\n# TYPE zubspace_observer_auto_targets gauge\n");
    for target in [7_u8, 10, 15, 20] {
        let count = state
            .clients
            .values()
            .filter(|client| {
                client.joined
                    && client.observer_combat_timeline.profile()
                        == crate::combat_timeline::ObserverNetworkProfile::AutomaticWan
                    && client
                        .observer_combat_timeline
                        .current_directive()
                        .is_some_and(|directive| directive.target_delay_ticks == target)
            })
            .count();
        let _ = writeln!(
            out,
            "zubspace_observer_auto_targets{{target_ticks=\"{target}\"}} {count}"
        );
    }
    for (name, help, kind, value) in [
        (
            "zubspace_rtc_outbound_queue_packets",
            "Queued outbound RTC packets.",
            "gauge",
            rtc.queue_packets,
        ),
        (
            "zubspace_rtc_outbound_queue_bytes",
            "Queued outbound RTC bytes.",
            "gauge",
            rtc.queue_bytes,
        ),
        (
            "zubspace_rtc_reliable_overflows_total",
            "Reliable queue overflows.",
            "counter",
            rtc.reliable_overflows,
        ),
        (
            "zubspace_rtc_realtime_coalesced_total",
            "Replaced stale realtime packets.",
            "counter",
            rtc.realtime_coalesced,
        ),
        (
            "zubspace_rtc_protocol_violations_total",
            "RTC channel protocol violations.",
            "counter",
            rtc.protocol_violations,
        ),
    ] {
        scalar(&mut out, name, help, kind, value);
    }
    for (index, channel) in ["game", "events", "reliable"].iter().enumerate() {
        if index == 0 {
            out.push_str("# HELP zubspace_rtc_inbound_queue_packets Pending RTC ingress packets.\n# TYPE zubspace_rtc_inbound_queue_packets gauge\n# HELP zubspace_rtc_inbound_packets_total RTC ingress packets drained.\n# TYPE zubspace_rtc_inbound_packets_total counter\n");
        }
        let _ = writeln!(
            out,
            "zubspace_rtc_inbound_queue_packets{{channel=\"{channel}\"}} {}\nzubspace_rtc_inbound_packets_total{{channel=\"{channel}\"}} {}",
            rtc.inbound_queue_packets[index], rtc.inbound_packets[index]
        );
    }
    for (name, help, kind, values, scale) in [
        (
            "zubspace_rtc_inbound_bytes_total",
            "Application bytes drained from ingress including quality probes; excludes rejected oversized payloads and wire overhead.",
            "counter",
            rtc.inbound_bytes,
            1.0,
        ),
        (
            "zubspace_rtc_outbound_packets_total",
            "Application packets accepted by each DataChannel, including event copies.",
            "counter",
            rtc.outbound_packets,
            1.0,
        ),
        (
            "zubspace_rtc_outbound_bytes_total",
            "Application bytes accepted by each DataChannel, including control/file traffic and event copies; excludes SCTP/DTLS/UDP overhead.",
            "counter",
            rtc.outbound_bytes,
            1.0,
        ),
        (
            "zubspace_rtc_send_capacity_waits_total",
            "Capacity waits that actually parked, including waits cancelled at teardown.",
            "counter",
            rtc.outbound_capacity_waits,
            1.0,
        ),
        (
            "zubspace_rtc_send_capacity_wait_seconds_total",
            "Completed or cancelled parked capacity wait time.",
            "counter",
            rtc.outbound_capacity_wait_us,
            1000000.0,
        ),
        (
            "zubspace_rtc_sent_queue_age_max_seconds",
            "Maximum lane-mailbox age of a packet actually sent; not current oldest backlog age.",
            "gauge",
            rtc.outbound_queue_age_max_us,
            1000000.0,
        ),
        (
            "zubspace_rtc_outstanding_bytes_peak",
            "Highest sampled per-channel outstanding application bytes at dispatch; not current outstanding or association wire bytes.",
            "gauge",
            rtc.outbound_outstanding_peak_bytes,
            1.0,
        ),
    ] {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (index, channel) in ["game", "events", "reliable"].iter().enumerate() {
            let value = values[index] as f64 / scale;
            let _ = writeln!(out, "{name}{{channel=\"{channel}\"}} {value}");
        }
    }
    for (name, help, values, scale) in [
        (
            "zubspace_rtc_lane_queue_packets",
            "Live lane-mailbox packet count, summed across sessions; excludes central dispatcher and SCTP buffers.",
            backlog.packets.map(|value| value as f64),
            1.0,
        ),
        (
            "zubspace_rtc_lane_queue_bytes",
            "Live lane-mailbox application bytes, summed across sessions; event copies charged per destination.",
            backlog.bytes.map(|value| value as f64),
            1.0,
        ),
        (
            "zubspace_rtc_lane_oldest_age_seconds",
            "Age of oldest currently queued lane-mailbox packet across sessions; zero when empty.",
            backlog.oldest_age_us.map(|value| value as f64),
            1_000_000.0,
        ),
    ] {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
        for (index, channel) in ["game", "events", "reliable"].iter().enumerate() {
            let value = values[index] / scale;
            let _ = writeln!(out, "{name}{{channel=\"{channel}\"}} {value}");
        }
    }
    out
}

/// Optional separate operator listener. No metrics or ingestion HTTP surface is
/// added to the public signaling listener. No CORS; no user-chosen labels.
pub(crate) fn start_metrics_listener(
    state: &Arc<Mutex<ServerState>>,
    started_at: Instant,
) -> io::Result<Option<std::thread::JoinHandle<()>>> {
    let Some(address) = state.lock().expect("state").config.metrics_listen else {
        return Ok(None);
    };
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    tracing::info!(event = "metrics_listener_started", bind = %listener.local_addr()?, "Prometheus metrics listener started");
    let weak = Arc::downgrade(state);
    Ok(Some(std::thread::spawn(move || {
        while let Some(state) = weak.upgrade() {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if let Err(error) = serve_metrics(&mut stream, &state, started_at) {
                        tracing::debug!(%error, "metrics request failed");
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(error) => {
                    tracing::error!(%error, "metrics listener failed");
                    break;
                }
            }
        }
    })))
}

fn serve_metrics(
    stream: &mut TcpStream,
    state: &Arc<Mutex<ServerState>>,
    started_at: Instant,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut request = Vec::with_capacity(256);
    while !request.ends_with(b"\r\n\r\n") {
        if request.len() >= 4096 {
            return Ok(());
        }
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        request.push(byte[0]);
    }
    let (status, body) = if request.starts_with(b"GET /metrics HTTP/1.") {
        ("200 OK", metrics(&state.lock().expect("state"), started_at))
    } else {
        ("404 Not Found", String::from("not found\n"))
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{join_arena, register_connection};
    use std::sync::mpsc;

    #[test]
    fn diagnostic_gates_are_fixed_size_scope_aware_and_do_not_extend_on_suppression() {
        let mut gate = DiagnosticRateGate::default();
        let now = Instant::now();
        let interval = Duration::from_secs(10);
        assert!(gate.admit(1, now, interval));
        assert!(!gate.admit(1, now + Duration::from_secs(9), interval));
        assert!(gate.admit(1, now + interval, interval));
        assert!(
            !gate.admit(1, now, interval),
            "backwards diagnostic clocks fail closed"
        );
        assert!(
            gate.admit(2, now + interval, interval),
            "new life gets a first witness"
        );
    }

    #[test]
    fn tick_phase_metrics_keep_exclusive_populations_and_bound_overrun_logging() {
        let mut state = ServerState::default();
        let now = Instant::now();
        let phases = [100, 200, 300, 400, 500];
        let before = state.movement_snapbacks_total;
        assert!(!state.telemetry.record_tick_phases(phases, 1501.0, now));
        assert!(state.telemetry.record_tick_phases(phases, 1500.0, now));
        for _ in 0..100 {
            assert!(!state.telemetry.record_tick_phases(phases, 1500.0, now));
        }
        assert!(
            state
                .telemetry
                .record_tick_phases(phases, 1500.0, now + Duration::from_secs(1))
        );
        assert_eq!(state.movement_snapbacks_total, before);
        assert_eq!(
            state.telemetry.tick_overruns, 0,
            "existing authority pacing owns this counter"
        );
        let text = metrics(&state, now);
        for name in TICK_PHASE_METRICS {
            assert!(text.contains(&format!("{name}_count 103\n")));
        }
        assert!(text.contains("zubspace_tick_phase_logs_suppressed_total 100\n"));
        assert!(text.contains("zubspace_log_lines_dropped_total "));
        assert!(text.contains("zubspace_snapshot_full_bytes_count "));
        assert!(text.contains("zubspace_snapshot_delta_bytes_count "));
        assert!(text.contains("zubspace_snapshot_send_skipped_total 0\n"));
        assert!(text.contains("zubspace_realtime_event_send_deferred_total 0\n"));
        assert!(!text.contains("player_id="));
    }

    #[test]
    fn version_2_reports_add_the_game_send_queue_and_version_1_still_counts() {
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let id = register_connection(&shared, tx);
        join_arena(&shared, id, 0);
        let (identity, version_1) = {
            let mut state = shared.lock().unwrap();
            let c = state.clients.get_mut(&id).unwrap();
            c.last_security_valid = Some(true);
            let mut p = vec![PLAYER_REPORT_TYPE, 1];
            p.extend_from_slice(&crate::wire::server_session_id().to_le_bytes());
            p.extend_from_slice(&c.arena_stream_id.to_le_bytes());
            p.extend_from_slice(&c.life_seq.to_le_bytes());
            for value in [300_u32, 1, 5_000_000, 100_000, u32::MAX, 1, 2, 5_000_000] {
                p.extend_from_slice(&value.to_le_bytes());
            }
            (PlayerSessionIdentity::new(id, c.session_id), p)
        };
        let version_2 = |peak: u32, dropped: u32| {
            let mut p = version_1.clone();
            p[1] = 2;
            p.extend_from_slice(&peak.to_le_bytes());
            p.extend_from_slice(&dropped.to_le_bytes());
            p
        };
        let mut version_1_labelled_2 = version_1.clone();
        version_1_labelled_2[1] = 2;
        for rejected in [
            version_2(MAX_REPORTED_BUFFERED_PEAK + 1, 0),
            version_2(0, MAX_REPORTED_DROPPED_SENDS + 1),
            version_1_labelled_2,
        ] {
            receive_player_report(&shared, identity, &rejected);
        }
        assert_eq!(shared.lock().unwrap().telemetry.reports, 0);

        receive_player_report(&shared, identity, &version_2(4_096, 3));
        {
            let state = shared.lock().unwrap();
            assert_eq!(state.telemetry.reports, 1);
            let exposition = metrics(&state, Instant::now());
            assert!(exposition.contains("zubspace_browser_game_sends_dropped_total 3\n"));
            assert!(
                exposition
                    .contains("zubspace_browser_game_buffered_peak_bytes_bucket{le=\"4095\"} 0\n")
            );
            assert!(
                exposition
                    .contains("zubspace_browser_game_buffered_peak_bytes_bucket{le=\"8191\"} 1\n")
            );
        }
        // A tab still on the older build reports version 1, without the queue.
        retire_session(&mut shared.lock().unwrap(), identity);
        receive_player_report(&shared, identity, &version_1);
        let state = shared.lock().unwrap();
        assert_eq!(state.telemetry.reports, 2);
        assert_eq!(
            state.telemetry.browser_game_buffered_peak.summary()["samples"],
            1
        );
    }

    #[test]
    fn browser_reports_are_scoped_rate_limited_bounded_and_never_mutate_gameplay() {
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let id = register_connection(&shared, tx);
        join_arena(&shared, id, 0);
        let (identity, mut packet) = {
            let mut state = shared.lock().unwrap();
            let c = state.clients.get_mut(&id).unwrap();
            c.last_security_valid = Some(true);
            let mut p = vec![PLAYER_REPORT_TYPE, 1];
            p.extend_from_slice(&crate::wire::server_session_id().to_le_bytes());
            p.extend_from_slice(&c.arena_stream_id.to_le_bytes());
            p.extend_from_slice(&c.life_seq.to_le_bytes());
            for value in [300_u32, 1, 5_000_000, 100_000, u32::MAX, 1, 2, 5_000_000] {
                p.extend_from_slice(&value.to_le_bytes());
            }
            (PlayerSessionIdentity::new(id, c.session_id), p)
        };
        for offset in [0, 1, 2, 10, 18, 22, 26, 30, 34, 42, 46, 50] {
            let mut malformed = packet.clone();
            malformed[offset..(offset + 4).min(REPORT_LEN)].fill(255);
            receive_player_report(&shared, identity, &malformed);
        }
        receive_player_report(&shared, identity, &packet[..53]);
        assert_eq!(shared.lock().unwrap().telemetry.reports, 0);
        receive_player_report(&shared, identity, &packet);
        receive_player_report(&shared, identity, &packet);
        {
            let state = shared.lock().unwrap();
            assert_eq!(state.telemetry.reports, 1);
            let c = &state.clients[&id];
            assert!(c.last_position.is_none());
            assert_eq!(c.inputs_received, 0);
            let exposition = metrics(&state, Instant::now());
            assert!(exposition.contains("zubspace_browser_frames_total 300\n"));
            assert!(exposition.contains("zubspace_browser_ice_round_trip_seconds_count 0\n"));
            assert!(exposition.contains("zubspace_browser_frame_window_mean_seconds_count 1\n"));
            assert!(!exposition.contains("player_id="));
        }
        retire_session(&mut shared.lock().unwrap(), identity);
        assert!(shared.lock().unwrap().telemetry.last_report.is_empty());
        let stale = PlayerSessionIdentity::new(
            id,
            crate::authority_identity::PlayerSessionId::new(identity.session_id.get() + 1),
        );
        receive_player_report(&shared, stale, &packet);
        shared
            .lock()
            .unwrap()
            .clients
            .get_mut(&id)
            .unwrap()
            .last_security_valid = Some(false);
        receive_player_report(&shared, identity, &packet);
        shared
            .lock()
            .unwrap()
            .clients
            .get_mut(&id)
            .unwrap()
            .last_security_valid = Some(true);
        packet[18..22].copy_from_slice(&0_u32.to_le_bytes());
        receive_player_report(&shared, identity, &packet);
        assert_eq!(shared.lock().unwrap().telemetry.reports, 1);
    }

    #[test]
    fn rtc_lane_metrics_include_duplicate_bytes_and_exact_seconds_without_player_labels() {
        let state = ServerState::default();
        for index in 0..3 {
            state.rtc_overload_metrics.record_lane_send(
                index,
                100 + index,
                Duration::from_micros(2_500),
            );
            state.rtc_overload_metrics.record_lane_capacity(
                index,
                true,
                Duration::from_micros(1_500),
                262_144,
            );
        }
        let text = metrics(&state, Instant::now());
        assert!(text.contains("zubspace_rtc_outbound_bytes_total{channel=\"events\"} 101\n"));
        assert!(text.contains("zubspace_rtc_send_capacity_waits_total{channel=\"reliable\"} 1\n"));
        assert!(text.contains(
            "zubspace_rtc_send_capacity_wait_seconds_total{channel=\"reliable\"} 0.0015\n"
        ));
        assert!(
            text.contains("zubspace_rtc_sent_queue_age_max_seconds{channel=\"game\"} 0.0025\n")
        );
        assert!(text.contains("zubspace_rtc_outstanding_bytes_peak{channel=\"game\"} 262144\n"));
        assert!(!text.contains("player_id="));
    }

    #[test]
    fn exact_prometheus_histogram_population_and_units() {
        let mut histogram = TimingHistogram::default();
        for value in [0, 15, 16, 31, 32, 1_000_000, 20_000_000] {
            histogram.record(value);
        }
        let mut text = String::new();
        histogram.prometheus("test_seconds", "test", &mut text);
        assert!(text.contains("test_seconds_bucket{le=\"0.000015\"} 2\n"));
        assert!(text.contains("test_seconds_bucket{le=\"0.000031\"} 4\n"));
        assert!(text.contains("test_seconds_bucket{le=\"+Inf\"} 7\n"));
        assert!(text.contains("test_seconds_count 7\n"));
        assert!(text.contains("test_seconds_sum 21.000094\n"));
    }

    #[test]
    fn snapshot_emission_records_sizes_in_bytes_and_counts_paced_sends() {
        let mut telemetry = Telemetry::default();
        let mut emission = SnapshotEmission::default();
        emission.sent(true, 1200);
        emission.sent(false, 300);
        emission.sent(false, 500);
        emission.snapshot_paced_out();
        emission.realtime_event_paced_out();
        emission.realtime_event_paced_out();
        emission.realtime_event_sent(1);
        emission.realtime_event_sent(4);
        emission.realtime_event_sent(4);
        emission.realtime_event_sent(0);
        emission.realtime_event_sent(9);
        telemetry.record_snapshot_emission(emission);
        assert_eq!(telemetry.snapshots_paced_out, 1);
        assert_eq!(telemetry.realtime_events_paced_out, 2);
        assert_eq!(telemetry.realtime_events_sent, [1, 0, 0, 2, 0]);

        let mut text = String::new();
        telemetry
            .snapshot_delta_bytes
            .prometheus_scaled("delta_bytes", "test", &mut text, 1.0);
        assert!(
            text.contains("delta_bytes_bucket{le=\"255\"} 0\n"),
            "{text}"
        );
        assert!(
            text.contains("delta_bytes_bucket{le=\"511\"} 2\n"),
            "{text}"
        );
        assert!(text.contains("delta_bytes_count 2\n"), "{text}");
        assert!(text.contains("delta_bytes_sum 800\n"), "{text}");
        let mut text = String::new();
        telemetry
            .snapshot_full_bytes
            .prometheus_scaled("full_bytes", "test", &mut text, 1.0);
        assert!(
            text.contains("full_bytes_bucket{le=\"2047\"} 1\n"),
            "{text}"
        );
        assert!(text.contains("full_bytes_sum 1200\n"), "{text}");
    }
}
