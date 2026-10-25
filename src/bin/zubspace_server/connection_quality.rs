//! Server-owned challenge/echo measurements. Never feed client-reported
//! diagnostics, redundant input gaps, or reliable retransmissions into this policy.
//! A missing echo measures round-trip probe delivery loss, not directional UDP loss.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[path = "connection_quality/holds.rs"]
pub(crate) mod holds;

#[path = "connection_quality/presentation.rs"]
mod presentation;
pub(crate) use presentation::{
    AdaptivePresentationPolicy, adapt_in_state as adapt_presentation_in_state,
};

const PROBE_INTERVAL: Duration = Duration::from_millis(500);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const WINDOW: Duration = Duration::from_secs(30);
const MIN_SAMPLES: usize = 20;
const MAX_PENDING: usize = 8;
const MAX_SAMPLES: usize = 64;
const SUSTAINED: Duration = Duration::from_secs(10);
const COOLDOWN: Duration = Duration::from_secs(60);
const SERVER_STALL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionQualityPolicy {
    pub(crate) max_rtt_ms: u16,
    pub(crate) max_loss_per_mille: u16,
    pub(crate) recovery_rtt_ms: u16,
    pub(crate) recovery_loss_per_mille: u16,
}

impl Default for ConnectionQualityPolicy {
    fn default() -> Self {
        Self {
            max_rtt_ms: 600,
            max_loss_per_mille: 150,
            recovery_rtt_ms: 400,
            recovery_loss_per_mille: 100,
        }
    }
}

impl ConnectionQualityPolicy {
    pub(crate) fn validate(self) -> Result<(), String> {
        if !(100..=2500).contains(&self.max_rtt_ms)
            || self.recovery_rtt_ms == 0
            || self.recovery_rtt_ms >= self.max_rtt_ms
        {
            return Err("connection quality RTT limit must be 100..=2500ms and recovery RTT must be positive and strictly lower".into());
        }
        if !(2..=1000).contains(&self.max_loss_per_mille)
            || self.recovery_loss_per_mille == 0
            || self.recovery_loss_per_mille >= self.max_loss_per_mille
        {
            return Err("connection quality loss limit must be 2..=1000 per mille and recovery loss must be positive and strictly lower".into());
        }
        Ok(())
    }
    pub(crate) fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let parsed: u16 = value
            .parse()
            .map_err(|_| format!("{key} must be an unsigned 16-bit integer"))?;
        match key {
            "quality_max_rtt_ms" => self.max_rtt_ms = parsed,
            "quality_max_loss_per_mille" => self.max_loss_per_mille = parsed,
            "quality_recovery_rtt_ms" => self.recovery_rtt_ms = parsed,
            "quality_recovery_loss_per_mille" => self.recovery_loss_per_mille = parsed,
            _ => return Err(format!("unknown connection quality setting {key}")),
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Probe {
    nonce: u64,
    sequence: Option<u32>,
    sent_at: Instant,
    rtt: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QualityReading {
    pub(crate) samples: usize,
    pub(crate) average_rtt_ms: Option<f64>,
    pub(crate) round_trip_loss: f64,
    pub(crate) should_spectate: bool,
    /// Fresh evidence from this exact session/arena, independent of whether
    /// an older session left a reconnect-persistent hold in ServerState.
    pub(crate) sustained_healthy: bool,
    pub(crate) may_play: bool,
    pub(crate) cooldown_remaining: Duration,
}

impl QualityReading {
    /// A full sample window (at least 10 s of probes) with every probe
    /// answered: no loss seen in the last 30 s.
    pub(crate) fn shows_clean_link(&self) -> bool {
        self.samples >= MIN_SAMPLES && self.round_trip_loss <= 0.0
    }
}

/// All storage is bounded independently of session duration or echo volume.
/// The caller must reset the monitor at an arena/session generation boundary.
#[derive(Clone, Debug)]
pub(crate) struct ConnectionQuality {
    policy: ConnectionQualityPolicy,
    pending: VecDeque<Probe>,
    samples: VecDeque<Probe>,
    last_sent: Option<Instant>,
    last_poll: Instant,
    bad_since: Option<Instant>,
    healthy_since: Option<Instant>,
    cooldown_until: Option<Instant>,
}

impl ConnectionQuality {
    pub(crate) fn new(now: Instant) -> Self {
        Self::with_policy(now, ConnectionQualityPolicy::default())
    }

    fn with_policy(now: Instant, policy: ConnectionQualityPolicy) -> Self {
        Self {
            policy,
            pending: VecDeque::new(),
            samples: VecDeque::new(),
            last_sent: None,
            last_poll: now,
            bad_since: None,
            healthy_since: None,
            cooldown_until: None,
        }
    }

    pub(crate) fn probe_due(&self, now: Instant) -> bool {
        self.pending.len() < MAX_PENDING
            && self
                .last_sent
                .is_none_or(|at| now.saturating_duration_since(at) >= PROBE_INTERVAL)
    }

    /// Start immediately before RTC dispatch, after queue acceptance and wait.
    /// Queue rejection is host pressure and must never count as player loss.
    pub(crate) fn record_sent(&mut self, nonce: u64, now: Instant) -> bool {
        if nonce == 0
            || !self.probe_due(now)
            || self
                .pending
                .iter()
                .chain(self.samples.iter())
                .any(|p| p.nonce == nonce)
        {
            return false;
        }
        self.last_sent = Some(now);
        self.pending.push_back(Probe {
            nonce,
            sequence: None,
            sent_at: now,
            rtt: None,
        });
        true
    }

    /// A local dispatch failure is not path loss.
    fn cancel_sent(&mut self, nonce: u64) {
        self.pending.retain(|probe| probe.nonce != nonce);
    }

    fn discard_window(&mut self, now: Instant) {
        self.pending.clear();
        self.samples.clear();
        self.last_sent = None;
        self.bad_since = None;
        self.healthy_since = None;
        self.last_poll = now;
    }

    /// Transport authentication and arena-generation validation precede this call.
    pub(crate) fn record_echo(&mut self, nonce: u64, now: Instant) -> bool {
        let Some(probe) = self.pending.iter_mut().find(|p| p.nonce == nonce) else {
            return false;
        };
        let Some(rtt) = now.checked_duration_since(probe.sent_at) else {
            return false;
        };
        if probe.rtt.is_some() || rtt >= PROBE_TIMEOUT {
            return false;
        }
        probe.rtt = Some(rtt);
        true
    }

    pub(crate) fn poll(&mut self, now: Instant) -> QualityReading {
        // A paused server cannot distinguish delayed processing from a bad path.
        // Discard the entire affected measurement window, preserving an existing
        // cooldown. New evidence must establish recovery after the interruption.
        if now.saturating_duration_since(self.last_poll) >= SERVER_STALL {
            self.pending.clear();
            self.samples.clear();
            self.bad_since = None;
            self.healthy_since = None;
            self.last_sent = None;
        }
        self.last_poll = now;
        // Mature successes and losses at the same age: fast echoes otherwise
        // bias the current window toward success while lost probes are pending.
        while self
            .pending
            .front()
            .is_some_and(|p| now.saturating_duration_since(p.sent_at) >= PROBE_TIMEOUT)
        {
            self.samples.push_back(self.pending.pop_front().unwrap());
        }
        while self
            .samples
            .front()
            .is_some_and(|p| now.saturating_duration_since(p.sent_at) > WINDOW)
            || self.samples.len() > MAX_SAMPLES
        {
            self.samples.pop_front();
        }
        let (received, total_ms) = self
            .samples
            .iter()
            .filter_map(|p| p.rtt)
            .fold((0usize, 0.0f64), |(count, sum), rtt| {
                (count + 1, sum + rtt.as_secs_f64() * 1000.0)
            });
        let average_rtt_ms = (received > 0).then(|| total_ms / received as f64);
        let loss = if self.samples.is_empty() {
            0.0
        } else {
            (self.samples.len() - received) as f64 / self.samples.len() as f64
        };
        let enough = self.samples.len() >= MIN_SAMPLES;
        // ASSS Lag defaults: PingToSpec=600ms; loss limits=15%. Our loss is
        // explicitly a round-trip proxy and not the legacy directional metric.
        let lost_per_mille_numerator = (self.samples.len() - received) * 1000;
        let bad = enough
            && (lost_per_mille_numerator
                >= self.samples.len() * usize::from(self.policy.max_loss_per_mille)
                || average_rtt_ms.is_some_and(|rtt| rtt >= f64::from(self.policy.max_rtt_ms)));
        let healthy = enough
            && lost_per_mille_numerator
                < self.samples.len() * usize::from(self.policy.recovery_loss_per_mille)
            && average_rtt_ms.is_some_and(|rtt| rtt < f64::from(self.policy.recovery_rtt_ms));
        if bad {
            self.bad_since.get_or_insert(now);
        } else {
            self.bad_since = None;
        }
        if healthy {
            self.healthy_since.get_or_insert(now);
        } else {
            self.healthy_since = None;
        }
        let cooldown_remaining = self
            .cooldown_until
            .map_or(Duration::ZERO, |until| until.saturating_duration_since(now));
        let recovery_required = self.cooldown_until.is_some();
        let sustained_healthy = self
            .healthy_since
            .is_some_and(|at| now.saturating_duration_since(at) >= SUSTAINED);
        QualityReading {
            samples: self.samples.len(),
            average_rtt_ms,
            round_trip_loss: loss,
            should_spectate: self.cooldown_until.is_none()
                && self
                    .bad_since
                    .is_some_and(|at| now.saturating_duration_since(at) >= SUSTAINED),
            sustained_healthy,
            may_play: cooldown_remaining.is_zero() && (!recovery_required || sustained_healthy),
            cooldown_remaining,
        }
    }

    /// Set once at the committed spectator transition, never on retry/echo.
    pub(crate) fn begin_cooldown(&mut self, now: Instant) {
        if self.cooldown_until.is_none() {
            self.cooldown_until = Some(now + COOLDOWN);
            self.healthy_since = None;
        }
    }

    /// Call only when committing a healthy return to play. A future degraded
    /// period must be able to trigger a new hold after this one was served.
    pub(crate) fn finish_cooldown(&mut self, now: Instant) -> bool {
        if self.cooldown_until.is_none() || !self.poll(now).may_play {
            return false;
        }
        self.cooldown_until = None;
        self.bad_since = None;
        true
    }
}

use crate::connection_quality_protocol::ConnectionQualityProbe;

pub(crate) fn hold_for_client(
    state: &crate::state::ServerState,
    player_id: u16,
) -> Option<holds::QualityHold> {
    let identity = state.clients.get(&player_id)?.player_identity.as_deref()?;
    state.quality_holds.get(identity)
}

pub(crate) fn hold_packet(
    client: &crate::state::ClientHandle,
    hold: holds::QualityHold,
    now: Instant,
    rejected_request: Option<(u8, u16)>,
) -> Vec<u8> {
    use crate::connection_quality_protocol::{
        ConnectionQualityDirective, ConnectionQualityDirectiveKind,
    };
    ConnectionQualityDirective {
        kind: ConnectionQualityDirectiveKind::Hold,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: client.arena_stream_id,
        player_id: client.player.id,
        predecessor_life_seq: client.life_seq,
        life_seq: client.life_seq,
        hold_id: hold.id,
        remaining_ms: hold.remaining(now).as_millis().min(COOLDOWN.as_millis()) as u32,
        rejected_request,
    }
    .encode()
}

/// Re-poll at request time: a previous tick's healthy verdict is insufficient
/// after a host pause. Reconnect scope replacement also discards that evidence.
pub(crate) fn sustained_health_now(
    state: &crate::state::ServerState,
    player_id: u16,
    now: Instant,
) -> bool {
    let Some(client) = state.clients.get(&player_id) else {
        return false;
    };
    if !client.joined || !client.snapshot_ready {
        return false;
    }
    let host_healthy = state.tick_wallclock_ewma_us
        < (4_000_000.0 / f64::from(state.config.tick_rate_hz.max(1))).max(50_000.0);
    client.connection_quality.as_ref().is_some_and(|quality| {
        quality
            .lock()
            .expect("session quality lock poisoned")
            .poll(now, host_healthy)
            .is_some_and(|reading| reading.sustained_healthy)
    })
}

/// Shared only by this session's RTC callback, writer and simulation. Its small
/// lock is independent of ServerState and is never held across an await.
#[derive(Debug)]
pub(crate) struct SessionConnectionQuality {
    arena_stream_id: u64,
    transport_available: bool,
    sequence: u32,
    queued: Option<ConnectionQualityProbe>,
    queued_at: Option<Instant>,
    monitor: ConnectionQuality,
    pub(crate) reading: Option<QualityReading>,
    last_log: Option<Instant>,
    last_hold_status: Option<(u64, u32, Instant)>,
    pub(crate) sent: u64,
    pub(crate) echoes: u64,
    pub(crate) dispatch_cancelled: u64,
    pub(crate) invalid_echoes: u64,
    /// Highest logged window's average RTT, and the running total and count
    /// behind their mean, for the session summary.
    pub(crate) window_rtt_ms_peak: f64,
    window_rtt_ms_total: f64,
    rtt_windows: u32,
}

impl SessionConnectionQuality {
    pub(crate) fn window_rtt_ms_mean(&self) -> Option<f64> {
        (self.rtt_windows > 0).then(|| self.window_rtt_ms_total / f64::from(self.rtt_windows))
    }

    fn record_window_rtt(&mut self, average_rtt_ms: Option<f64>) {
        if let Some(rtt) = average_rtt_ms {
            self.window_rtt_ms_peak = self.window_rtt_ms_peak.max(rtt);
            self.window_rtt_ms_total += rtt;
            self.rtt_windows += 1;
        }
    }

    pub(crate) fn new(now: Instant) -> Self {
        Self {
            arena_stream_id: 0,
            transport_available: true,
            sequence: 0,
            queued: None,
            queued_at: None,
            monitor: ConnectionQuality::new(now),
            reading: None,
            last_log: None,
            last_hold_status: None,
            sent: 0,
            echoes: 0,
            dispatch_cancelled: 0,
            invalid_echoes: 0,
            window_rtt_ms_peak: 0.0,
            window_rtt_ms_total: 0.0,
            rtt_windows: 0,
        }
    }

    pub(crate) fn configure(&mut self, policy: ConnectionQualityPolicy) {
        self.monitor.policy = policy;
    }

    pub(crate) fn reset_scope(&mut self, arena_stream_id: u64, now: Instant) {
        let policy = self.monitor.policy;
        let transport_available = self.transport_available;
        *self = Self::new(now);
        self.monitor.policy = policy;
        self.transport_available = transport_available;
        self.arena_stream_id = arena_stream_id;
    }

    /// A known-disconnected RTC peer is not an active gameplay path. Sampling
    /// it until ICE's failure timeout would manufacture a reconnect-persistent
    /// penalty after an ordinary browser exit. Keep committed cooldowns; only
    /// discard measurements, pending dispatch and stale recovery evidence.
    pub(crate) fn set_transport_available(&mut self, available: bool, now: Instant) {
        if self.transport_available != available {
            self.transport_available = available;
            self.monitor.discard_window(now);
            self.cancel_queued();
            self.reading = None;
        }
    }

    pub(crate) fn poll(&mut self, now: Instant, host_healthy: bool) -> Option<QualityReading> {
        if self.arena_stream_id == 0 || !self.transport_available {
            return None;
        }
        if !host_healthy {
            self.monitor.discard_window(now);
            self.cancel_queued();
        }
        if self
            .queued_at
            .is_some_and(|at| now.saturating_duration_since(at) >= SERVER_STALL)
        {
            // A writer that cannot dispatch in one second is host pressure.
            self.cancel_queued();
            self.monitor.discard_window(now);
        }
        let reading = self.monitor.poll(now);
        self.reading = Some(reading);
        Some(reading)
    }

    pub(crate) fn reserve_probe(
        &mut self,
        nonce: u64,
        now: Instant,
    ) -> Option<ConnectionQualityProbe> {
        if self.arena_stream_id == 0
            || !self.transport_available
            || nonce == 0
            || self.queued.is_some()
            || !self.monitor.probe_due(now)
        {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1);
        let probe = ConnectionQualityProbe {
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: self.arena_stream_id,
            sequence: self.sequence,
            nonce,
        };
        self.queued = Some(probe);
        self.queued_at = Some(now);
        Some(probe)
    }

    pub(crate) fn cancel_queued(&mut self) {
        if self.queued.take().is_some() {
            self.dispatch_cancelled = self.dispatch_cancelled.saturating_add(1);
        }
        self.queued_at = None;
    }

    /// Runs immediately before the actual DataChannel send. A stale queued
    /// probe cannot start a sample after arena replacement or host cancellation.
    pub(crate) fn begin_dispatch(&mut self, probe: ConnectionQualityProbe, now: Instant) -> bool {
        if !self.transport_available || self.queued != Some(probe) {
            return false;
        }
        if self
            .queued_at
            .is_some_and(|at| now.saturating_duration_since(at) >= SERVER_STALL)
        {
            self.cancel_queued();
            self.monitor.discard_window(now);
            return false;
        }
        self.queued = None;
        self.queued_at = None;
        if !self.monitor.record_sent(probe.nonce, now) {
            return false;
        }
        self.monitor.pending.back_mut().unwrap().sequence = Some(probe.sequence);
        self.sent = self.sent.saturating_add(1);
        true
    }

    pub(crate) fn dispatch_failed(&mut self, probe: ConnectionQualityProbe) {
        self.monitor.cancel_sent(probe.nonce);
        self.dispatch_cancelled = self.dispatch_cancelled.saturating_add(1);
    }

    /// On the authenticated RTC game channel, before the application queue.
    pub(crate) fn receive_echo(&mut self, probe: ConnectionQualityProbe, now: Instant) -> bool {
        if !self.transport_available
            || probe.server_session_id != crate::wire::server_session_id()
            || probe.arena_stream_id != self.arena_stream_id
            || !self.monitor.pending.iter().any(|pending| {
                pending.nonce == probe.nonce && pending.sequence == Some(probe.sequence)
            })
            || !self.monitor.record_echo(probe.nonce, now)
        {
            self.invalid_echoes = self.invalid_echoes.saturating_add(1);
            return false;
        }
        // Nonce is unpredictable; the exact scope/sequence is consumed once.
        self.echoes = self.echoes.saturating_add(1);
        true
    }

    pub(crate) fn log_due(&mut self, now: Instant) -> bool {
        if self
            .last_log
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(10))
        {
            return false;
        }
        self.last_log = Some(now);
        true
    }
}

/// Called under the simulation's ServerState lock; never waits on RTC work.
pub(crate) fn poll_client(
    client: &mut crate::state::ClientHandle,
    now: Instant,
    host_healthy: bool,
) {
    if !client.joined || !client.snapshot_ready {
        return;
    }
    let Some(shared) = client.connection_quality.as_ref() else {
        return;
    };
    let mut quality = shared.lock().expect("session quality lock poisoned");
    if let Some(reading) = quality.poll(now, host_healthy) {
        if quality.log_due(now) {
            tracing::debug!(
                player_id = client.player.id,
                session_id = client.session_id.get(),
                arena_stream_id = client.arena_stream_id,
                last_probe_sequence = quality.sequence,
                unanswered_probe_sequences = ?quality.monitor.samples.iter()
                    .filter(|probe| probe.rtt.is_none()).filter_map(|probe| probe.sequence)
                    .collect::<Vec<_>>(),
                answered_probe_sequences = ?quality.monitor.samples.iter()
                    .filter(|probe| probe.rtt.is_some()).filter_map(|probe| probe.sequence)
                    .collect::<Vec<_>>(),
                invalid_echoes = quality.invalid_echoes,
                "server connection quality probe audit"
            );
            tracing::info!(
                player_id = client.player.id,
                session_id = client.session_id.get(),
                arena_stream_id = client.arena_stream_id,
                samples = reading.samples,
                average_rtt_ms = reading.average_rtt_ms,
                round_trip_probe_loss = reading.round_trip_loss,
                sustained_degradation = reading.should_spectate,
                probes_sent = quality.sent,
                decision_enforced = true,
                echoes_received = quality.echoes,
                invalid_echoes = quality.invalid_echoes,
                local_dispatch_cancelled = quality.dispatch_cancelled,
                host_healthy,
                "server connection quality window"
            );
            quality.record_window_rtt(reading.average_rtt_ms);
        }
        if host_healthy && quality.queued.is_none() && quality.monitor.probe_due(now) {
            let mut bytes = [0u8; 8];
            // Measurement challenges must be unpredictable. Entropy failure
            // skips a sample; a clock-derived fallback would permit forged RTT.
            if getrandom::fill(&mut bytes).is_ok() {
                if let Some(probe) = quality.reserve_probe(u64::from_le_bytes(bytes), now) {
                    if client.tx.send(probe.encode()).is_err() {
                        quality.cancel_queued();
                    }
                }
            }
        }
    }
}

/// Runs under the tick's exclusive arena transaction and ServerState lock.
/// No callback or client diagnostic may directly enter this authority path.
pub(crate) fn enforce_in_state(state: &mut crate::state::ServerState, tick: u32, now: Instant) {
    use crate::connection_quality_protocol::{
        ConnectionQualityDirective, ConnectionQualityDirectiveKind,
    };
    let candidates = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            if !client.joined || !client.snapshot_ready || client.player.ship >= 8 {
                return None;
            }
            let identity = client.player_identity.clone()?;
            let reading = client
                .connection_quality
                .as_ref()?
                .lock()
                .expect("session quality lock poisoned")
                .reading?;
            reading.should_spectate.then(|| {
                (
                    id,
                    identity,
                    state.active_player_runtime_identity(id),
                    reading,
                )
            })
        })
        .collect::<Vec<_>>();
    for (id, identity, runtime, reading) in candidates {
        let Some(runtime) = runtime else {
            continue;
        };
        let prepared = match state.quality_holds.prepare(&identity, now) {
            Ok(hold) => hold,
            Err(error) => {
                tracing::error!(
                    player_id = id,
                    ?error,
                    decision = "hold_allocation_failed",
                    "connection quality decision"
                );
                continue;
            }
        };
        let crate::arena::RosterChangeOutcome::Applied(roster) =
            crate::arena::force_quality_spectator_in_state(state, runtime, tick, now)
        else {
            continue;
        };
        // The preflight and commit share the same lock: no other allocation
        // can interleave, and deferred roster transitions create no hold.
        let hold = state
            .quality_holds
            .begin(&identity, now)
            .expect("preflighted quality hold");
        assert_eq!(hold, prepared);
        state.quality_holds.forced = state.quality_holds.forced.saturating_add(1);
        let client = state.clients.get_mut(&id).expect("forced client exists");
        let fact = ConnectionQualityDirective {
            kind: ConnectionQualityDirectiveKind::Force,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: client.arena_stream_id,
            player_id: id,
            predecessor_life_seq: runtime.life.life_seq,
            life_seq: client.life_seq,
            hold_id: hold.id,
            remaining_ms: hold.remaining(now).as_millis().min(COOLDOWN.as_millis()) as u32,
            rejected_request: None,
        };
        crate::reliable::send_reliable_payload(client, &fact.encode());
        tracing::warn!(
            player_id = id,
            session_id = client.session_id.get(),
            arena_stream_id = client.arena_stream_id,
            life_seq = client.life_seq,
            hold_id = hold.id,
            remaining_ms = fact.remaining_ms,
            samples = reading.samples,
            average_rtt_ms = reading.average_rtt_ms,
            round_trip_probe_loss = reading.round_trip_loss,
            decision = "forced_spectator",
            "connection quality decision"
        );
        for recipient in state.clients.values_mut().filter(|client| client.joined) {
            crate::reliable::send_reliable_payload(recipient, &roster);
        }
    }
    for client in state
        .clients
        .values_mut()
        .filter(|client| client.joined && client.snapshot_ready && client.player.ship == 8)
    {
        let Some(hold) = client
            .player_identity
            .as_deref()
            .and_then(|identity| state.quality_holds.get(identity))
        else {
            continue;
        };
        let Some(shared) = client.connection_quality.as_ref() else {
            continue;
        };
        let due = {
            let mut quality = shared.lock().expect("session quality lock poisoned");
            if quality.last_hold_status.is_some_and(|(id, life, at)| {
                id == hold.id
                    && life == client.life_seq
                    && now.saturating_duration_since(at) < Duration::from_secs(1)
            }) {
                false
            } else {
                quality.last_hold_status = Some((hold.id, client.life_seq, now));
                true
            }
        };
        if due {
            let packet = hold_packet(client, hold, now, None);
            crate::reliable::send_reliable_payload(client, &packet);
        }
    }
}

/// Aggregate current-session gauges, deliberately without player/session labels.
pub(crate) fn summary(state: &crate::state::ServerState) -> serde_json::Value {
    let (
        mut sessions,
        mut measured,
        mut sent,
        mut echoes,
        mut cancelled,
        mut samples,
        mut degraded,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0usize, 0u64);
    let (mut max_rtt_ms, mut max_loss) = (0.0f64, 0.0f64);
    for shared in state
        .clients
        .values()
        .filter_map(|client| client.connection_quality.as_ref())
    {
        let quality = shared.lock().expect("session quality lock poisoned");
        sessions += 1;
        sent = sent.saturating_add(quality.sent);
        echoes = echoes.saturating_add(quality.echoes);
        cancelled = cancelled.saturating_add(quality.dispatch_cancelled);
        if let Some(reading) = quality.reading {
            samples += reading.samples;
            if reading.samples >= MIN_SAMPLES {
                measured += 1;
                max_rtt_ms = max_rtt_ms.max(reading.average_rtt_ms.unwrap_or(0.0));
                max_loss = max_loss.max(reading.round_trip_loss);
                degraded += u64::from(reading.should_spectate);
            }
        }
    }
    serde_json::json!({ "scope": "current-session-windows", "sessions": sessions,
        "measured_sessions": measured, "dispatch_started": sent, "echoes_received": echoes,
        "local_dispatch_cancelled": cancelled, "samples": samples,
        "max_session_average_rtt_ms": max_rtt_ms, "max_session_round_trip_probe_loss": max_loss,
        "sustained_degraded_sessions": degraded, "enforcement_enabled": true,
        "retained_holds": state.quality_holds.len(),
        "held_sessions": state.clients.keys().filter(|&&id| hold_for_client(state, id).is_some()).count(),
        "forced_total": state.quality_holds.forced, "return_denied_total": state.quality_holds.denied,
        "healthy_return_total": state.quality_holds.returned,
        "thresholds": { "max_rtt_ms": state.config.connection_quality.max_rtt_ms,
            "max_loss_per_mille": state.config.connection_quality.max_loss_per_mille,
            "recovery_rtt_ms": state.config.connection_quality.recovery_rtt_ms,
            "recovery_loss_per_mille": state.config.connection_quality.recovery_loss_per_mille } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_runtime_forces_only_sustained_bad_peer_and_publishes_scoped_fact_first() {
        use crate::connection_quality_protocol::{
            ConnectionQualityDirective, ConnectionQualityDirectiveKind,
        };
        use std::sync::{Arc, Mutex, mpsc};
        let state = Arc::new(Mutex::new(crate::state::ServerState::default()));
        let (tx_bad, rx_bad) = mpsc::channel();
        let (tx_good, rx_good) = mpsc::channel();
        let bad = crate::arena::register_connection(&state, tx_bad);
        let good = crate::arena::register_connection(&state, tx_good);
        for (id, identity) in [(bad, "bad"), (good, "good")] {
            assert!(crate::identity::set_player_identity_and_name(
                &state,
                id,
                identity.into(),
                identity.into()
            ));
            crate::arena::join_arena(&state, id, 0);
        }
        rx_bad.try_iter().for_each(drop);
        rx_good.try_iter().for_each(drop);
        let base = Instant::now();
        let now = base + Duration::from_secs(30);
        let old_life = {
            let mut guard = state.lock().unwrap();
            for (id, rtt) in [(bad, 700), (good, 40)] {
                let client = guard.clients.get_mut(&id).unwrap();
                let mut quality = SessionConnectionQuality::new(base);
                quality.reset_scope(client.arena_stream_id, base);
                for ms in (0..=30_000).step_by(500) {
                    step(&mut quality.monitor, base, ms, Some(rtt));
                }
                quality.poll(now, true);
                client.connection_quality = Some(Arc::new(Mutex::new(quality)));
                client.snapshot_ready = true;
            }
            let client = guard.clients.get_mut(&bad).unwrap();
            client.server_energy = 1;
            client.life_seq
        };
        {
            let mut guard = state.lock().unwrap();
            enforce_in_state(&mut guard, 100, now);
            assert_eq!(guard.clients[&bad].player.ship, 8);
            assert_eq!(guard.clients[&bad].life_seq, old_life + 1);
            assert_eq!(guard.clients[&good].player.ship, 0);
            assert_eq!(guard.quality_holds.forced, 1);
            let hold = guard.quality_holds.get("bad").unwrap();
            assert_eq!(hold.remaining(now), COOLDOWN);
            enforce_in_state(&mut guard, 101, now + Duration::from_millis(500));
            assert_eq!(guard.quality_holds.get("bad"), Some(hold));
            assert_eq!(guard.clients[&bad].life_seq, old_life + 1);
            assert_eq!(guard.quality_holds.forced, 1);
            assert_eq!(summary(&guard)["held_sessions"], 1);
            let metrics = crate::observability::metrics(&guard, base);
            assert!(metrics.contains("zubspace_connection_quality_forced_total 1\n"));
        }
        let payloads = rx_bad
            .try_iter()
            .filter_map(|packet| {
                crate::reliable::reliable_payload(&packet).map(|payload| payload.to_vec())
            })
            .collect::<Vec<_>>();
        assert_eq!(payloads.len(), 3, "{payloads:?}");
        let force = ConnectionQualityDirective::parse(&payloads[0]).unwrap();
        assert_eq!(force.kind, ConnectionQualityDirectiveKind::Force);
        assert_eq!(force.predecessor_life_seq, old_life);
        assert_eq!(force.life_seq, old_life + 1);
        assert_eq!(force.player_id, bad);
        assert_eq!(force.remaining_ms, 60_000);
        assert_eq!(payloads[1][0], crate::roster::SHIP_FREQUENCY_CHANGE);
        assert_eq!(
            ConnectionQualityDirective::parse(&payloads[2])
                .unwrap()
                .kind,
            ConnectionQualityDirectiveKind::Hold
        );
        let good_payloads = rx_good
            .try_iter()
            .filter_map(|packet| {
                crate::reliable::reliable_payload(&packet).map(|payload| payload.to_vec())
            })
            .collect::<Vec<_>>();
        assert_eq!(good_payloads, [payloads[1].clone()]);
    }

    #[test]
    fn quality_healthy_return_commits_clear_before_ship_ack_and_releases_registry() {
        use crate::connection_quality_protocol::{
            ConnectionQualityDirective, ConnectionQualityDirectiveKind,
        };
        use std::sync::{Arc, Mutex, mpsc};
        let state = Arc::new(Mutex::new(crate::state::ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player = crate::arena::register_connection(&state, tx);
        assert!(crate::identity::set_player_identity_and_name(
            &state,
            player,
            "returning".into(),
            "returning".into()
        ));
        let now = Instant::now();
        let hold = state
            .lock()
            .unwrap()
            .quality_holds
            .begin("returning", now - COOLDOWN)
            .unwrap();
        crate::arena::join_arena(&state, player, 0);
        rx.try_iter().for_each(drop);
        let old_life = {
            let mut guard = state.lock().unwrap();
            let client = guard.clients.get_mut(&player).unwrap();
            assert_eq!(client.player.ship, 8);
            let base = now - Duration::from_secs(25);
            let mut quality = SessionConnectionQuality::new(base);
            quality.reset_scope(client.arena_stream_id, base);
            for ms in (0..=25_000).step_by(500) {
                step(&mut quality.monitor, base, ms, Some(40));
            }
            client.connection_quality = Some(Arc::new(Mutex::new(quality)));
            client.snapshot_ready = true;
            client.life_seq
        };
        crate::arena::update_ship(&state, player, &[0x18, 0, 0, 0]);
        let payloads = rx
            .try_iter()
            .filter_map(|packet| {
                crate::reliable::reliable_payload(&packet).map(|payload| payload.to_vec())
            })
            .collect::<Vec<_>>();
        assert_eq!(payloads.len(), 2, "{payloads:?}");
        let clear = ConnectionQualityDirective::parse(&payloads[0]).unwrap();
        assert_eq!(clear.kind, ConnectionQualityDirectiveKind::Clear);
        assert_eq!(clear.hold_id, hold.id);
        assert_eq!(clear.life_seq, old_life);
        assert_eq!(payloads[1][0], crate::roster::SHIP_FREQUENCY_CHANGE);
        let guard = state.lock().unwrap();
        assert!(guard.quality_holds.get("returning").is_none());
        assert_eq!(guard.clients[&player].player.ship, 0);
        assert!(guard.clients[&player].alive);
        assert_eq!(guard.clients[&player].life_seq, old_life + 1);
    }

    fn step(
        monitor: &mut ConnectionQuality,
        base: Instant,
        ms: u64,
        rtt: Option<u64>,
    ) -> QualityReading {
        let now = base + Duration::from_millis(ms);
        let reading = monitor.poll(now);
        if monitor.probe_due(now) {
            let nonce = ms + 1;
            assert!(monitor.record_sent(nonce, now));
            if let Some(rtt) = rtt {
                assert!(monitor.record_echo(nonce, now + Duration::from_millis(rtt)));
            }
        }
        reading
    }

    #[test]
    fn quality_reconnect_requires_fresh_recovery_evidence_and_host_stall_revokes_it() {
        let base = Instant::now();
        let mut session = SessionConnectionQuality::new(base);
        session.reset_scope(1, base);
        for ms in (0..=25_000).step_by(500) {
            step(&mut session.monitor, base, ms, Some(40));
        }
        assert!(
            session
                .poll(base + Duration::from_secs(25), true)
                .unwrap()
                .sustained_healthy
        );
        // An arena switch/reconnect cannot borrow the preceding path's verdict.
        let rejoined = base + Duration::from_secs(26);
        session.reset_scope(2, rejoined);
        let initial = session.poll(rejoined, true).unwrap();
        assert!(!initial.sustained_healthy);
        assert!(initial.may_play); // no session-local hold; registry decides admission
        for ms in (0..=25_000).step_by(500) {
            let reading = step(&mut session.monitor, rejoined, ms, Some(40));
            assert_eq!(reading.sustained_healthy, ms >= 22_500);
        }
        assert!(
            !session
                .poll(rejoined + Duration::from_secs(25), false)
                .unwrap()
                .sustained_healthy
        );
        assert_eq!(session.reading.unwrap().samples, 0);
    }

    #[test]
    fn scoped_probes_exclude_queue_wait_and_cancel_host_failures() {
        let base = Instant::now();
        let mut session = SessionConnectionQuality::new(base);
        assert!(session.reserve_probe(1, base).is_none());
        session.reset_scope(42, base);
        let probe = session.reserve_probe(7, base).unwrap();
        assert!(session.reserve_probe(8, base).is_none());
        session.poll(base + Duration::from_millis(500), true);
        assert!(session.begin_dispatch(probe, base + Duration::from_millis(800)));
        let mut forged = probe;
        forged.sequence = forged.sequence.wrapping_add(1);
        assert!(!session.receive_echo(forged, base + Duration::from_millis(900)));
        assert!(session.receive_echo(probe, base + Duration::from_millis(950)));
        assert!(!session.receive_echo(probe, base + Duration::from_millis(960)));
        for ms in (1000..=4000).step_by(500) {
            session.poll(base + Duration::from_millis(ms), true);
        }
        let reading = session.reading.unwrap();
        assert_eq!(reading.samples, 1);
        assert_eq!(reading.average_rtt_ms, Some(150.0)); // excludes 800ms queue wait
        let second = session
            .reserve_probe(8, base + Duration::from_secs(4))
            .unwrap();
        assert!(session.begin_dispatch(second, base + Duration::from_secs(4)));
        session.dispatch_failed(second);
        for ms in (4500..=7500).step_by(500) {
            session.poll(base + Duration::from_millis(ms), true);
        }
        assert_eq!(session.reading.unwrap().samples, 1);
        assert_eq!(session.reading.unwrap().round_trip_loss, 0.0);
        let queued = session
            .reserve_probe(9, base + Duration::from_millis(7500))
            .unwrap();
        assert!(!session.begin_dispatch(queued, base + Duration::from_secs(9)));
        assert!(session.monitor.pending.is_empty());
        assert!(session.monitor.samples.is_empty());
    }

    #[test]
    fn offline_probe_pause_preserves_committed_cooldown_and_requires_fresh_evidence() {
        let base = Instant::now();
        let mut session = SessionConnectionQuality::new(base);
        session.reset_scope(7, base);
        session.monitor.begin_cooldown(base);
        let probe = session.reserve_probe(1, base).unwrap();
        assert!(session.begin_dispatch(probe, base));
        session.set_transport_available(false, base + Duration::from_secs(1));
        for second in 1..30 {
            let now = base + Duration::from_secs(second);
            assert!(session.poll(now, true).is_none());
            assert!(session.reserve_probe(second + 1, now).is_none());
        }
        assert!(session.monitor.pending.is_empty());
        assert!(session.monitor.samples.is_empty());
        let resumed = base + Duration::from_secs(30);
        session.set_transport_available(true, resumed);
        assert!(!session.receive_echo(probe, resumed));
        let reading = session.poll(resumed, true).unwrap();
        assert_eq!(reading.samples, 0);
        assert_eq!(reading.cooldown_remaining, Duration::from_secs(30));
        assert!(!reading.may_play);
        assert!(!reading.sustained_healthy);

        // Ordinary connected packet loss still triggers the unchanged policy.
        let mut connected = SessionConnectionQuality::new(base);
        connected.reset_scope(8, base);
        for ms in (0..=30_000).step_by(500) {
            let now = base + Duration::from_millis(ms);
            connected.poll(now, true);
            let probe = connected.reserve_probe(ms + 1, now).unwrap();
            assert!(connected.begin_dispatch(probe, now));
        }
        assert!(connected.reading.unwrap().should_spectate);
    }

    #[test]
    fn scope_replacement_and_host_overload_retire_old_queued_and_inflight_probes() {
        let base = Instant::now();
        let mut session = SessionConnectionQuality::new(base);
        session.reset_scope(1, base);
        let old = session.reserve_probe(5, base).unwrap();
        session.reset_scope(2, base);
        assert!(!session.begin_dispatch(old, base));
        let current = session.reserve_probe(6, base).unwrap();
        assert!(session.begin_dispatch(current, base));
        assert!(!session.receive_echo(
            ConnectionQualityProbe {
                arena_stream_id: 1,
                ..current
            },
            base
        ));
        session.poll(base + Duration::from_millis(50), false);
        assert!(!session.receive_echo(current, base + Duration::from_millis(100)));
        assert_eq!(session.reading.unwrap().samples, 0);
    }

    #[test]
    fn exact_fifteen_percent_loss_boundary_requires_sustained_evidence() {
        let base = Instant::now();
        for missing in [2, 3] {
            let mut monitor = ConnectionQuality::new(base);
            for i in 0..20 {
                monitor.samples.push_back(Probe {
                    nonce: i + 1,
                    sequence: None,
                    sent_at: base,
                    rtt: (i >= missing).then_some(Duration::from_millis(40)),
                });
            }
            let mut verdict = monitor.poll(base);
            assert!(!verdict.should_spectate);
            for ms in (500..=10_000).step_by(500) {
                verdict = monitor.poll(base + Duration::from_millis(ms));
            }
            assert_eq!(verdict.should_spectate, missing == 3);
            assert_eq!(verdict.round_trip_loss, missing as f64 / 20.0);
        }
    }

    #[test]
    fn configured_policy_changes_judgment_and_survives_scope_replacement() {
        let base = Instant::now();
        let policy = ConnectionQualityPolicy {
            max_rtt_ms: 500,
            recovery_rtt_ms: 350,
            ..ConnectionQualityPolicy::default()
        };
        policy.validate().unwrap();
        let mut configured = ConnectionQuality::with_policy(base, policy);
        let mut default = ConnectionQuality::new(base);
        let (mut configured_bad, mut default_bad) = (false, false);
        for ms in (0..30_000).step_by(500) {
            configured_bad |= step(&mut configured, base, ms, Some(550)).should_spectate;
            default_bad |= step(&mut default, base, ms, Some(550)).should_spectate;
        }
        assert!(configured_bad);
        assert!(!default_bad);
        let mut session = SessionConnectionQuality::new(base);
        session.configure(policy);
        session.reset_scope(5, base);
        assert_eq!(session.monitor.policy, policy);
        session.reset_scope(6, base);
        assert_eq!(session.monitor.policy, policy);
    }

    #[test]
    fn healthy_path_and_short_spike_do_not_force_spectator() {
        let base = Instant::now();
        let mut monitor = ConnectionQuality::new(base);
        for ms in (0..60_000).step_by(500) {
            let rtt = if (20_000..21_000).contains(&ms) {
                None
            } else {
                Some(40)
            };
            let reading = step(&mut monitor, base, ms, rtt);
            assert!(!reading.should_spectate);
            assert!(reading.may_play);
            assert!(monitor.pending.len() <= MAX_PENDING);
            assert!(monitor.samples.len() <= MAX_SAMPLES);
        }
    }

    #[test]
    fn sustained_delay_or_loss_requires_full_cooldown_and_healthy_recovery() {
        for rtt in [Some(700), None] {
            let base = Instant::now();
            let mut monitor = ConnectionQuality::new(base);
            let mut forced_at = None;
            for ms in (0..165_000).step_by(500) {
                let reading = step(
                    &mut monitor,
                    base,
                    ms,
                    if ms < 30_000 { rtt } else { Some(40) },
                );
                if reading.should_spectate {
                    assert!(ms >= 22_000);
                    assert!(forced_at.is_none());
                    monitor.begin_cooldown(base + Duration::from_millis(ms));
                    forced_at = Some(ms);
                }
                if let Some(forced) = forced_at {
                    if ms > forced && ms < forced + COOLDOWN.as_millis() as u64 {
                        assert!(!reading.may_play);
                    }
                    if ms >= forced + COOLDOWN.as_millis() as u64 {
                        assert!(reading.may_play);
                    }
                }
            }
            assert!(forced_at.is_some());
        }
    }

    #[test]
    fn echoes_cannot_forge_duplicate_late_or_unknown_samples() {
        let base = Instant::now();
        let mut monitor = ConnectionQuality::new(base);
        assert!(!monitor.record_sent(0, base));
        assert!(monitor.record_sent(5, base));
        assert!(!monitor.record_echo(8, base));
        assert!(!monitor.record_echo(5, base - Duration::from_millis(1)));
        assert!(monitor.record_echo(5, base + Duration::from_millis(30)));
        assert!(!monitor.record_echo(5, base + Duration::from_millis(50)));
        assert!(monitor.record_sent(6, base + PROBE_INTERVAL));
        assert!(!monitor.record_echo(6, base + PROBE_INTERVAL + PROBE_TIMEOUT));
        // No early successes: both outcomes enter the window only on maturity.
        assert_eq!(monitor.poll(base + Duration::from_millis(600)).samples, 0);
    }

    #[test]
    fn server_stall_cancels_measurements_but_does_not_erase_cooldown() {
        let base = Instant::now();
        let mut monitor = ConnectionQuality::new(base);
        for ms in (0..25_000).step_by(500) {
            step(&mut monitor, base, ms, None);
        }
        monitor.begin_cooldown(base + Duration::from_secs(25));
        monitor.begin_cooldown(base + Duration::from_secs(26));
        let reading = monitor.poll(base + Duration::from_secs(40));
        assert_eq!(reading.samples, 0);
        assert_eq!(reading.cooldown_remaining, Duration::from_secs(45));
        assert!(!reading.should_spectate);
        assert!(!reading.may_play);
        // Expiry alone cannot admit an unmeasured or still-unhealthy path.
        assert!(!monitor.poll(base + Duration::from_secs(86)).may_play);
    }

    #[test]
    fn healthy_return_rearms_policy_for_subsequent_degradation() {
        let base = Instant::now();
        let mut monitor = ConnectionQuality::new(base);
        monitor.begin_cooldown(base);
        assert!(!monitor.finish_cooldown(base));
        for ms in (0..=60_000).step_by(500) {
            step(&mut monitor, base, ms, Some(40));
        }
        assert!(monitor.finish_cooldown(base + COOLDOWN));
        assert!(!monitor.finish_cooldown(base + COOLDOWN));
        let mut forced_again = false;
        for ms in (60_500..120_000).step_by(500) {
            forced_again |= step(&mut monitor, base, ms, None).should_spectate;
        }
        assert!(forced_again);
    }
}
