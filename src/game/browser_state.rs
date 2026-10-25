use super::network::{
    AuthorityAdmissionSeed, LifecycleAction, authoritative_lifecycle_action,
    build_authority_admission_plan, prize_state_is_admissible_across_lifecycle_cutover,
    self_runtime_snapshot_is_admissible,
};
use super::*;

#[cfg(target_arch = "wasm32")]
// Server application-reliable retries become due every three seconds. Keep
// the lifecycle barrier bounded, but allow several complete retry windows so
// a lost ACK, a lost ordered prefix, and a lost application receipt cannot
// exhaust the deadline before their independent retries have run.
pub(in crate::game) const SHIP_CHANGE_ACK_TIMEOUT_SECONDS: f32 = 15.0;

#[cfg(test)]
pub(in crate::game) fn preserve_lifecycle_request_intent(
    pending: &mut Option<PendingLifecycleRequest>,
    can_queue: bool,
    intent: PendingLifecycleRequest,
) -> bool {
    if !can_queue || pending.is_some() {
        return false;
    }
    *pending = Some(intent);
    true
}

#[cfg(test)]
pub(in crate::game) fn queue_frequency_change_for_network_tail(
    pending: &mut Option<PendingLifecycleRequest>,
    can_queue: bool,
    frequency: u16,
) -> bool {
    // Frequency actions are sampled before the current reference-game movement
    // step.  Preserve only the intent here.  The normal network tail sends it
    // after movement, so its reserved sequence N and exact f32 baseline are
    // the post-N canonical state rather than the pre-N state mislabeled N.
    preserve_lifecycle_request_intent(
        pending,
        can_queue,
        PendingLifecycleRequest::Frequency(frequency),
    )
}

/// The server's application-reliable lifecycle terminal shares its ordered
/// S2C stream with the arena map/LVZ bootstrap. Preserve an early ship or
/// frequency intent, but do not emit its C2S transaction until the
/// authenticated complete-file publication has drained. Otherwise the
/// server can commit the new life while its terminal sits behind thousands of
/// already queued file fragments and the client must eventually fail closed.
#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn lifecycle_request_emission_ready(zone_files_ready: bool) -> bool {
    zone_files_ready
}

/// Correction transactions and lifecycle application receipts must run before
/// the current gameplay step. The correction barrier prevents an action from
/// being consumed on the old movement lineage; the lifecycle receipt rebases
/// before sampling and suppresses that frame so no extra sequence is advanced.
#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn network_event_applies_before_gameplay(event: &ClientProtocolEvent) -> bool {
    matches!(
        event,
        ClientProtocolEvent::MovementCorrected(_)
            | ClientProtocolEvent::MovementCorrectionAccepted(_)
            | ClientProtocolEvent::ShipChangeRejected(_)
            | ClientProtocolEvent::ConnectionQualityDirective(_)
            | ClientProtocolEvent::LifecycleBaselineAccepted(_)
            | ClientProtocolEvent::RepelApplicationAccepted(_)
            | ClientProtocolEvent::PrizeEffectAccepted(_)
            | ClientProtocolEvent::PreGameplayAuthorityReset
            | ClientProtocolEvent::PreGameplayPrizeState { .. }
            | ClientProtocolEvent::PrizePickupAck(ScopedPrizePickupAck {
                ack: PrizePickupAck {
                    engine_shutdown_application_required: true,
                    ..
                },
                ..
            })
    )
}

/// Extract only recipient-private PrizeState from a snapshot for pre-step
/// reconciliation. The original snapshot remains one post-step event with its
/// entities and every non-prize event intact. Removing the private state from
/// that copy makes double application structurally impossible while retaining
/// the exact relative ordering among pickup ACKs, private state, and terminals.
#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn partition_browser_network_actions_at_segment(
    mut actions: ClientProtocolActions,
    server_session_id: Option<u64>,
    arena_stream_id: Option<u64>,
    self_player_id: Option<u16>,
    authoritative_life_seq: Option<u32>,
    authoritative_alive: Option<bool>,
    joined_arena: bool,
    arena_epoch: Option<u32>,
    settings_ready: bool,
    security_ready: bool,
    map_ready: bool,
    authority_segment: u64,
    movement_correction_pending: bool,
    respawn_baseline_pending: bool,
    lifecycle_cutover_pending: bool,
) -> (ClientProtocolActions, ClientProtocolActions) {
    let initial_scope = AuthorityScopeIdentity {
        server_session_id,
        arena_stream_id,
        player_id: self_player_id,
        life_seq: authoritative_life_seq,
        alive: authoritative_alive,
    };
    let seed = AuthorityAdmissionSeed {
        scope: initial_scope,
        readiness: AuthorityReadiness {
            joined: joined_arena,
            arena_epoch,
            settings: settings_ready,
            security: security_ready,
            map: map_ready,
        },
        segment: authority_segment,
    };
    let authority_admission = if actions.authority_admission.len() == actions.events.len() {
        std::mem::take(&mut actions.authority_admission)
    } else {
        build_authority_admission_plan(&actions, seed)
    };
    let has_hard_replacement = authority_admission
        .iter()
        .any(|admission| matches!(admission.boundary, AuthorityBoundaryAdmission::Hard { .. }));
    let replacement_segment =
        authority_admission
            .iter()
            .rev()
            .find_map(|admission| match admission.boundary {
                AuthorityBoundaryAdmission::Hard { segment, .. } => Some(segment),
                _ => None,
            });
    let mut pre_gameplay = ClientProtocolActions::default();
    let mut deferred = ClientProtocolActions::default();
    let mut replacement_context = ClientProtocolActions::default();
    let mut replacement_private = ClientProtocolActions::default();
    let mut replacement_snapshot_tail = false;
    pre_gameplay.outbound_packets = std::mem::take(&mut actions.outbound_packets);
    if let Some(admission) = authority_admission
        .iter()
        .rev()
        .find(|admission| matches!(admission.boundary, AuthorityBoundaryAdmission::Hard { .. }))
    {
        pre_gameplay
            .events
            .push(ClientProtocolEvent::PreGameplayAuthorityReset);
        pre_gameplay.authority_admission.push(*admission);
    }
    for (event, mut admission) in actions.events.into_iter().zip(authority_admission) {
        match event {
            _event if admission.readiness_fact == AuthorityReadinessAdmission::Reject => {
                // A readiness carrier belongs to an authority generation, not
                // merely a reusable numeric session/arena tuple. Never move
                // an old carrier across the hard boundary or let it reach
                // Game's security/map/settings state.
                replacement_snapshot_tail = false;
            }
            ClientProtocolEvent::SnapshotReceived(mut frame) => {
                let private = frame.events.iter().find_map(|event| match event {
                    SnapshotEvent::PrizeState(state) => Some(*state),
                    _ => None,
                });
                if let Some(state) = private {
                    let staged = ClientProtocolEvent::PreGameplayPrizeState {
                        snapshot_id: frame.header.snapshot_id,
                        snapshot_tick: frame.header.tick,
                        state,
                    };
                    if has_hard_replacement
                        && matches!(
                            admission.private_fact,
                            PrivateAuthorityAdmission::Replacement(_)
                        )
                        && admission.readiness_after.complete()
                    {
                        replacement_snapshot_tail = true;
                        frame
                            .events
                            .retain(|event| !matches!(event, SnapshotEvent::PrizeState(_)));
                        replacement_context
                            .events
                            .push(ClientProtocolEvent::SnapshotReceived(frame));
                        replacement_context.authority_admission.push(admission);
                        admission.boundary = AuthorityBoundaryAdmission::None;
                        if let PrivateAuthorityAdmission::Replacement(scope) =
                            admission.private_fact
                        {
                            admission.action = AuthorityActionAdmission::Bound(scope);
                        }
                        replacement_private.events.push(staged);
                        replacement_private.authority_admission.push(admission);
                        continue;
                    }
                    frame
                        .events
                        .retain(|event| !matches!(event, SnapshotEvent::PrizeState(_)));
                    let readiness_complete = admission.readiness_after.complete();
                    match admission.private_fact {
                        PrivateAuthorityAdmission::Current(scope)
                            if scope.is_complete_live()
                                && readiness_complete
                                && self_runtime_snapshot_is_admissible(
                                    authoritative_alive,
                                    respawn_baseline_pending,
                                )
                                && prize_state_is_admissible_across_lifecycle_cutover(
                                    lifecycle_cutover_pending,
                                    authoritative_life_seq,
                                    state.life_seq,
                                ) =>
                        {
                            let mut staged_admission = admission;
                            staged_admission.boundary = AuthorityBoundaryAdmission::None;
                            staged_admission.action = AuthorityActionAdmission::Bound(scope);
                            pre_gameplay.events.push(staged);
                            pre_gameplay.authority_admission.push(staged_admission);
                        }
                        PrivateAuthorityAdmission::Replacement(scope)
                            if scope.is_complete_live() && readiness_complete =>
                        {
                            let mut staged_admission = admission;
                            staged_admission.boundary = AuthorityBoundaryAdmission::None;
                            staged_admission.action = AuthorityActionAdmission::Bound(scope);
                            replacement_private.events.push(staged);
                            replacement_private
                                .authority_admission
                                .push(staged_admission);
                        }
                        _ => {}
                    }
                }
                let replacement_scope = matches!(
                    admission.private_fact,
                    PrivateAuthorityAdmission::Replacement(_)
                );
                let replacement_action_scope = match admission.action {
                    AuthorityActionAdmission::Bound(scope) => Some(scope),
                    _ => None,
                };
                let target = if replacement_scope
                    && (admission.boundary == AuthorityBoundaryAdmission::Soft
                        || private.is_some()
                        || replacement_action_scope.is_none_or(|scope| !scope.is_complete_live()))
                {
                    &mut replacement_context
                } else if replacement_scope {
                    &mut replacement_private
                } else {
                    &mut deferred
                };
                target
                    .events
                    .push(ClientProtocolEvent::SnapshotReceived(frame));
                target.authority_admission.push(admission);
            }
            event @ (ClientProtocolEvent::PlayerPosition(_)
            | ClientProtocolEvent::SnapshotPlayerPosition { .. })
                if replacement_snapshot_tail =>
            {
                replacement_private.events.push(event);
                replacement_private.authority_admission.push(admission);
            }
            event
                if has_hard_replacement
                    && admission.readiness_fact == AuthorityReadinessAdmission::Replacement
                    && matches!(event, ClientProtocolEvent::SecurityChallengeReceived { .. }) =>
            {
                // SECURITY is cross-channel. Preserve its immutable final
                // segment admission while staging it after reliable
                // replacement boundaries/settings and before any admitted
                // replacement-private baseline.
                replacement_context.events.push(event);
                replacement_context.authority_admission.push(admission);
            }
            event
                if network_event_applies_before_gameplay(&event)
                    && !(movement_correction_pending
                        && network_event_waits_for_movement_correction_terminal(&event))
                    && matches!(
                        admission.private_fact,
                        PrivateAuthorityAdmission::Irrelevant
                            | PrivateAuthorityAdmission::Current(_)
                    ) =>
            {
                pre_gameplay.events.push(event);
                pre_gameplay.authority_admission.push(admission);
            }
            event
                if matches!(
                    admission.private_fact,
                    PrivateAuthorityAdmission::Replacement(_)
                ) && admission.readiness_after.complete() =>
            {
                if let PrivateAuthorityAdmission::Replacement(scope) = admission.private_fact {
                    admission.action = AuthorityActionAdmission::Bound(scope);
                }
                replacement_private.events.push(event);
                replacement_private.authority_admission.push(admission);
            }
            event
                if !has_hard_replacement
                    && matches!(
                        admission.action,
                        AuthorityActionAdmission::Bound(scope) if scope != initial_scope
                    ) =>
            {
                // A same-segment lifecycle snapshot is staged ahead of its
                // recipient-private facts. Preserve every later callback
                // already stamped against that exact new life in the same
                // context group; otherwise an ordinary reliable roster fact
                // falls in front of the snapshot and is rejected against the
                // predecessor Game life.
                replacement_context.events.push(event);
                replacement_context.authority_admission.push(admission);
            }
            event
                if has_hard_replacement
                    && replacement_segment == Some(admission.segment)
                    && matches!(admission.action, AuthorityActionAdmission::Bound(_)) =>
            {
                let target = match admission.action {
                    AuthorityActionAdmission::Bound(scope) if scope.is_complete_live() => {
                        &mut replacement_private
                    }
                    _ => &mut replacement_context,
                };
                target.events.push(event);
                target.authority_admission.push(admission);
            }
            event => {
                replacement_snapshot_tail = false;
                deferred.events.push(event);
                deferred.authority_admission.push(admission);
            }
        }
    }
    deferred.append(replacement_context);
    deferred.append(replacement_private);
    (pre_gameplay, deferred)
}

#[cfg(test)]
pub(in crate::game) fn partition_browser_network_actions(
    actions: ClientProtocolActions,
    server_session_id: Option<u64>,
    arena_stream_id: Option<u64>,
    self_player_id: Option<u16>,
    authoritative_life_seq: Option<u32>,
    authoritative_alive: Option<bool>,
    joined_arena: bool,
    arena_epoch: Option<u32>,
    settings_ready: bool,
    security_ready: bool,
    map_ready: bool,
    respawn_baseline_pending: bool,
    lifecycle_cutover_pending: bool,
) -> (ClientProtocolActions, ClientProtocolActions) {
    partition_browser_network_actions_at_segment(
        actions,
        server_session_id,
        arena_stream_id,
        self_player_id,
        authoritative_life_seq,
        authoritative_alive,
        joined_arena,
        arena_epoch,
        settings_ready,
        security_ready,
        map_ready,
        0,
        false,
        respawn_baseline_pending,
        lifecycle_cutover_pending,
    )
}

/// Same-life movement boundaries that arrive after a correction application
/// marker must wait for its exact terminal. Applying them locally first would
/// let an older terminal overwrite a newer Warp/door/Repel lineage.
#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn network_event_waits_for_movement_correction_terminal(
    event: &ClientProtocolEvent,
) -> bool {
    match event {
        ClientProtocolEvent::PrizePickupAck(scoped) => {
            scoped.ack.prize_id == PrizeKind::Warp.protocol_id()
        }
        ClientProtocolEvent::SetCoordinates(_) => true,
        ClientProtocolEvent::SnapshotReceived(frame) => frame.events.iter().any(|event| {
            matches!(
                event,
                SnapshotEvent::LifecycleState(lifecycle) if lifecycle.repel_delivery.is_some()
            )
        }),
        _ => false,
    }
}

/// An exact newer lifecycle boundary owns more authority than a same-life
/// movement correction. Let it reach the sequencer so death/ship replacement
/// can atomically cancel the obsolete correction and discard its held input.
/// Same-life Warp/Repel/coordinate changes still wait for the correction's
/// terminal, and malformed or stale lifecycle values are not promoted merely
/// because an admission was marked soft.
#[cfg(any(test, target_arch = "wasm32"))]
fn network_event_supersedes_movement_correction(
    event: &ClientProtocolEvent,
    admission: AuthorityEventAdmission,
) -> bool {
    if admission.boundary != AuthorityBoundaryAdmission::Soft {
        return false;
    }
    let AuthorityActionAdmission::Bound(observed) = admission.action else {
        return false;
    };
    if observed.alive != Some(true) {
        return false;
    }
    let candidate = match event {
        ClientProtocolEvent::PlayerDeath(death) if observed.player_id == Some(death.killed_id) => {
            Some(SnapshotLifecycleState {
                life_seq: death.life_seq,
                alive: false,
                respawn_at_tick: Some(death.respawn_at_tick),
                repel_until_tick: None,
                repel_delivery: None,
            })
        }
        ClientProtocolEvent::SnapshotReceived(frame)
            if observed.arena_stream_id == Some(frame.header.arena_stream_id) =>
        {
            frame.events.iter().find_map(|event| match event {
                SnapshotEvent::LifecycleState(lifecycle) => Some(*lifecycle),
                _ => None,
            })
        }
        ClientProtocolEvent::ShipFrequencyChanged(change)
            if observed.player_id == Some(change.player_id) =>
        {
            change.life_seq.map(|life_seq| SnapshotLifecycleState {
                life_seq,
                alive: change.ship < 8,
                respawn_at_tick: None,
                repel_until_tick: None,
                repel_delivery: None,
            })
        }
        _ => None,
    };
    candidate.is_some_and(|candidate| {
        authoritative_lifecycle_action(observed.life_seq, observed.alive, candidate)
            != LifecycleAction::Ignore
            && (observed.life_seq != Some(candidate.life_seq)
                || observed.alive != Some(candidate.alive))
    })
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn partition_network_actions_behind_movement_correction_at_segment(
    mut actions: ClientProtocolActions,
    barrier_active: bool,
    live_segment: u64,
) -> (ClientProtocolActions, ClientProtocolActions) {
    let mut ready = ClientProtocolActions {
        outbound_packets: std::mem::take(&mut actions.outbound_packets),
        events: Vec::new(),
        authority_admission: Vec::new(),
    };
    let mut held = ClientProtocolActions::default();
    let admission = if actions.authority_admission.len() == actions.events.len() {
        std::mem::take(&mut actions.authority_admission)
    } else {
        vec![
            AuthorityEventAdmission {
                segment: live_segment,
                action: AuthorityActionAdmission::Bound(AuthorityScopeIdentity::default()),
                ..AuthorityEventAdmission::default()
            };
            actions.events.len()
        ]
    };
    let final_replacement_segment =
        admission
            .iter()
            .fold(None, |latest, admission| match admission.boundary {
                AuthorityBoundaryAdmission::Hard { segment, .. } => Some(segment),
                _ => latest,
            });
    let admitted_segment = final_replacement_segment.unwrap_or(live_segment);
    for (event, admission) in actions.events.into_iter().zip(admission) {
        let hard_boundary = matches!(admission.boundary, AuthorityBoundaryAdmission::Hard { .. });
        let bound = matches!(admission.action, AuthorityActionAdmission::Bound(_));
        if admission.action == AuthorityActionAdmission::Reject
            || (!hard_boundary && bound && admission.segment != admitted_segment)
        {
            // A hard replacement atomically retires every previously held
            // gameplay fact before a correction terminal can release it.
            continue;
        }
        let supersedes_correction =
            barrier_active && network_event_supersedes_movement_correction(&event, admission);
        if barrier_active && bound && !hard_boundary && !supersedes_correction {
            held.events.push(event);
            held.authority_admission.push(admission);
        } else {
            ready.events.push(event);
            ready.authority_admission.push(admission);
        }
    }
    (ready, held)
}

#[cfg(test)]
pub(in crate::game) fn partition_network_actions_behind_movement_correction(
    actions: ClientProtocolActions,
    barrier_active: bool,
) -> (ClientProtocolActions, ClientProtocolActions) {
    partition_network_actions_behind_movement_correction_at_segment(actions, barrier_active, 0)
}

pub(in crate::game) fn movement_correction_blocks_local_discontinuity(
    application: Option<MovementCorrectionApplied>,
) -> bool {
    application.is_some()
}

#[cfg(target_arch = "wasm32")]
pub(in crate::game) struct BrowserNetworkClient {
    pub(in crate::game) client: NetworkClient<BrowserRtcTransport>,
    pub(in crate::game) login_name: String,
    pub(in crate::game) password: String,
    pub(in crate::game) player_identity: String,
    pub(in crate::game) login_sent: bool,
    pub(in crate::game) identity_login_retry: IdentityLoginRetry,
    pub(in crate::game) registration_requested: bool,
    pub(in crate::game) registration_sent: bool,
    /// The application disconnect was accepted by the browser transport.
    /// Keep the client object for diagnostics/rendering, but never poll or
    /// emit gameplay again while the page finishes its bounded RTC teardown.
    pub(in crate::game) disconnect_sent: bool,
    pub(in crate::game) position_timer: f32,
    pub(in crate::game) time_sync_timer: f32,
    pub(in crate::game) server_clock: BrowserServerClock,
    pub(in crate::game) stats: BrowserNetworkStats,
    /// Bounded transport-recovery latch. One rejoin attempt per terminal episode, cleared once the
    /// transport reaches open again, so a later failure is a new episode but a failing retry is not
    /// a reconnect loop.
    pub(in crate::game) rejoin_attempted: bool,
    /// Count of transport rejoins this page has started, published in `uiMetrics` so a recovery is
    /// attributable from the client's own numbers rather than inferred from peer counts.
    pub(in crate::game) rejoin_count: u32,
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BrowserTimeSyncRequest {
    nonce: u32,
    authority_segment: u64,
    server_session_id: Option<u64>,
    arena_stream_id: Option<u64>,
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::game) struct BrowserServerClock {
    latest_server_tick: Option<u32>,
    latest_authenticated_server_tick: Option<u32>,
    local_monotonic_micros: Option<u64>,
    local_tick_remainder_micros: u64,
    server_session_id: Option<u64>,
    arena_stream_id: Option<u64>,
    authority_segment: u64,
    input_sequence_base: u16,
    position_sequence_base: u16,
    weapon_sequence_base: u16,
    reliable_sequence_base: u16,
    tick_rate_hz: u16,
    snapshot_rate_hz: u16,
    sync_nonce: u32,
    pending_sync_requests: VecDeque<BrowserTimeSyncRequest>,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl BrowserServerClock {
    const MASK: u32 = 0x7fff_ffff;
    const MAX_PENDING_SYNC_REQUESTS: usize = 64;
    const TICK_MICROS: u64 = crate::combat_timeline::COMBAT_TIMELINE_TICK_MILLIS as u64 * 1_000;

    /// Advance the browser's estimate of the process-monotonic server clock
    /// between authenticated tick observations. Snapshot delivery is
    /// intentionally bursty (the server simulates at 60 Hz while Continuum
    /// time is 100 Hz); treating only each received snapshot as time made an
    /// ordinary arrival look like a multi-tick renderer hitch. This estimate
    /// cannot author a receipt render tick or combat result: the server
    /// independently derives both, and the applied-snapshot watermark still
    /// fails closed once presentation outruns retained evidence.
    pub(in crate::game) fn advance_local_monotonic_micros(&mut self, now_micros: u64) {
        let Some(previous_micros) = self.local_monotonic_micros.replace(now_micros) else {
            self.local_tick_remainder_micros = 0;
            return;
        };
        if now_micros < previous_micros || self.latest_server_tick.is_none() {
            self.local_tick_remainder_micros = 0;
            return;
        }
        let budget = self
            .local_tick_remainder_micros
            .saturating_add(now_micros - previous_micros);
        let elapsed_ticks = budget / Self::TICK_MICROS;
        self.local_tick_remainder_micros = budget % Self::TICK_MICROS;
        if elapsed_ticks != 0 {
            self.latest_server_tick = self
                .latest_server_tick
                .map(|tick| crate::continuum::continuum_tick_add(tick, elapsed_ticks as u32));
        }
    }

    pub(in crate::game) fn observe_server_tick(&mut self, server_tick: u32) {
        let server_tick = server_tick & Self::MASK;
        if self
            .latest_authenticated_server_tick
            .is_none_or(|latest| tick_after(server_tick, latest))
        {
            self.latest_authenticated_server_tick = Some(server_tick);
        }
        if self
            .latest_server_tick
            .is_none_or(|latest| tick_after(server_tick, latest))
        {
            self.latest_server_tick = Some(server_tick);
        }
    }

    pub(in crate::game) fn observe_authority_segment(&mut self, authority_segment: u64) {
        if self.authority_segment != authority_segment {
            self.authority_segment = authority_segment;
            self.pending_sync_requests.clear();
        }
    }

    /// Install a bootstrap atomically. A replacement generation clears the
    /// high tick before observing its lower bootstrap tick; ordinary forward
    /// same-generation bootstraps retain wrap-safe monotonic observation.
    pub(in crate::game) fn observe_bootstrap(
        &mut self,
        bootstrap: ServerClockBootstrap,
        authority_segment: u64,
    ) -> bool {
        let replacement = self.authority_segment != authority_segment
            || self.server_session_id != Some(bootstrap.server_session_id)
            || self.arena_stream_id != Some(bootstrap.arena_stream_id);
        if replacement {
            self.latest_server_tick = None;
            self.latest_authenticated_server_tick = None;
            self.local_tick_remainder_micros = 0;
            self.pending_sync_requests.clear();
        }
        self.authority_segment = authority_segment;
        self.server_session_id = Some(bootstrap.server_session_id);
        self.arena_stream_id = Some(bootstrap.arena_stream_id);
        self.input_sequence_base = bootstrap.input_sequence_base;
        self.position_sequence_base = bootstrap.position_sequence_base;
        self.weapon_sequence_base = bootstrap.weapon_sequence_base;
        self.reliable_sequence_base = bootstrap.reliable_sequence_base;
        self.tick_rate_hz = bootstrap.tick_rate_hz;
        self.snapshot_rate_hz = bootstrap.snapshot_rate_hz;
        if replacement {
            self.latest_server_tick = Some(bootstrap.server_tick & Self::MASK);
            self.latest_authenticated_server_tick = self.latest_server_tick;
            self.local_tick_remainder_micros = 0;
        } else {
            self.observe_server_tick(bootstrap.server_tick);
        }
        replacement
    }

    pub(in crate::game) fn weapon_scope(&self) -> Option<(u64, u64)> {
        Some((self.server_session_id?, self.arena_stream_id?))
    }

    pub(in crate::game) fn weapon_render_partial_seconds(&self) -> f32 {
        self.local_tick_remainder_micros as f32 / 1_000_000.0
    }

    pub(in crate::game) fn current_server_tick(&self) -> Option<u32> {
        self.latest_server_tick
    }

    /// Latest tick carried by an authenticated server packet. Unlike
    /// `current_server_tick`, this watermark never advances from the browser's
    /// local monotonic clock and can therefore own freshness/fail-closed gates.
    pub(in crate::game) fn latest_authenticated_server_tick(&self) -> Option<u32> {
        self.latest_authenticated_server_tick
    }

    pub(in crate::game) fn next_sync_nonce(&mut self, authority_segment: u64) -> u32 {
        self.observe_authority_segment(authority_segment);
        self.sync_nonce = self.sync_nonce.wrapping_add(1) & Self::MASK;
        self.pending_sync_requests
            .push_back(BrowserTimeSyncRequest {
                nonce: self.sync_nonce,
                authority_segment,
                server_session_id: self.server_session_id,
                arena_stream_id: self.arena_stream_id,
            });
        while self.pending_sync_requests.len() > Self::MAX_PENDING_SYNC_REQUESTS {
            self.pending_sync_requests.pop_front();
        }
        self.sync_nonce
    }

    pub(in crate::game) fn observe_time_sync(
        &mut self,
        scoped: ScopedTimeSyncResponse,
        authority_segment: u64,
    ) -> bool {
        let Some(index) = self
            .pending_sync_requests
            .iter()
            .position(|pending| pending.nonce == scoped.response.received_timestamp)
        else {
            return false;
        };
        let pending = self
            .pending_sync_requests
            .remove(index)
            .expect("located time-sync request exists");
        if pending.authority_segment != authority_segment
            || self.authority_segment != authority_segment
            || pending.server_session_id != scoped.server_session_id
            || pending.arena_stream_id != scoped.arena_stream_id
            || self.server_session_id != scoped.server_session_id
            || self.arena_stream_id != scoped.arena_stream_id
            || scoped.response.server_session_id != scoped.server_session_id
            || scoped.response.arena_stream_id != scoped.arena_stream_id
        {
            return false;
        }
        self.observe_server_tick(scoped.response.local_timestamp);
        true
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn tick_after(candidate: u32, previous: u32) -> bool {
    crate::continuum::continuum_tick_diff(candidate, previous) > 0
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn advance_network_position_timer(
    timer: &mut f32,
    dt: f32,
    interval: f32,
) -> bool {
    *timer -= dt.max(0.0);
    if *timer > 0.0 {
        return false;
    }

    *timer = (*timer + interval).clamp(0.0, interval);
    true
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn network_position_emission_due(
    position_timer_due: bool,
    final_catch_up_step: bool,
    pending_life_reset_position: bool,
    suppress_life_tail_position: bool,
    same_life_boundary_applied: bool,
    weapon_boundary_pending: bool,
) -> bool {
    pending_life_reset_position
        || same_life_boundary_applied
        // Weapons mutate local velocity after this logical movement step.
        // Publish that exact tail immediately instead of retaining the
        // weapon until the ordinary 10 Hz position cadence, when a later
        // displacement would no longer match the weapon's recoil boundary.
        || (weapon_boundary_pending && final_catch_up_step)
        || (!suppress_life_tail_position && position_timer_due && final_catch_up_step)
}

/// A current-protocol position already carries the same bounded rolling input
/// history that a standalone `0x62` packet would publish. Send the standalone
/// copy only on logical steps without a position so catch-up batches and timer
/// gaps remain recoverable without doubling steady-state game-lane ingress.
#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) fn standalone_input_emission_due(position_emission_due: bool) -> bool {
    !position_emission_due
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::game) struct BrowserNetworkStats {
    sample_elapsed: f32,
    last_packets_sent: u32,
    last_packets_received: u32,
    last_positions_sent: u32,
    last_remote_positions_received: u32,
    last_dropped_realtime_packets: u32,
    pub(in crate::game) packets_sent_per_second: f32,
    pub(in crate::game) packets_received_per_second: f32,
    pub(in crate::game) positions_sent_per_second: f32,
    pub(in crate::game) remote_positions_received_per_second: f32,
    pub(in crate::game) dropped_realtime_packets_per_second: f32,
    pub(in crate::game) positions_sent: u32,
    pub(in crate::game) dropped_realtime_packets: u32,
    pub(in crate::game) rtt_ticks: Option<i32>,
    pub(in crate::game) jitter_ticks: Option<i32>,
}

#[cfg(any(test, target_arch = "wasm32"))]
#[allow(dead_code)]
impl BrowserNetworkStats {
    pub(in crate::game) fn record_position_sent(&mut self) {
        self.positions_sent = self.positions_sent.wrapping_add(1);
    }

    pub(in crate::game) fn record_time_sync(&mut self, rtt_ticks: i32) {
        let rtt_ticks = rtt_ticks.max(0);
        self.jitter_ticks = self.rtt_ticks.map(|previous| (rtt_ticks - previous).abs());
        self.rtt_ticks = Some(rtt_ticks);
    }

    pub(in crate::game) fn update_rates(
        &mut self,
        dt: f32,
        packets_sent: u32,
        packets_received: u32,
        remote_positions_received: u32,
        dropped_realtime_packets: u32,
    ) {
        self.dropped_realtime_packets = dropped_realtime_packets;
        self.sample_elapsed += dt.max(0.0);
        if self.sample_elapsed < 1.0 {
            return;
        }

        let elapsed = self.sample_elapsed.max(f32::EPSILON);
        self.packets_sent_per_second =
            packets_sent.wrapping_sub(self.last_packets_sent) as f32 / elapsed;
        self.packets_received_per_second =
            packets_received.wrapping_sub(self.last_packets_received) as f32 / elapsed;
        self.positions_sent_per_second =
            self.positions_sent.wrapping_sub(self.last_positions_sent) as f32 / elapsed;
        self.remote_positions_received_per_second =
            remote_positions_received.wrapping_sub(self.last_remote_positions_received) as f32
                / elapsed;
        self.dropped_realtime_packets_per_second =
            dropped_realtime_packets.wrapping_sub(self.last_dropped_realtime_packets) as f32
                / elapsed;
        self.last_packets_sent = packets_sent;
        self.last_packets_received = packets_received;
        self.last_positions_sent = self.positions_sent;
        self.last_remote_positions_received = remote_positions_received;
        self.last_dropped_realtime_packets = dropped_realtime_packets;
        self.sample_elapsed = 0.0;
    }
}

#[cfg(test)]
mod position_cadence_tests {
    use super::{advance_network_position_timer, network_position_emission_due};
    use crate::constants::NETWORK_RTC_POSITION_INTERVAL;

    /// Replay the exact batching `src/main.rs` performs: a rendered frame accumulates wall time, drains an integer number
    /// of 100 Hz logical steps, and only the batch's final step may emit a position. Returns how many positions a client
    /// that never presses a key would emit in `seconds` at a given render cadence. Every disjunct of
    /// `network_position_emission_due` that is not the timer needs a boundary event, which a neutral client never
    /// produces, so the boundary arguments stay false.
    fn neutral_client_emissions(
        render_fps: f32,
        step_hz: f32,
        initial_timer: f32,
        seconds: f32,
    ) -> u32 {
        let step_seconds = 1.0 / step_hz;
        let mut timer = initial_timer;
        let mut carry = 0.0f32;
        let mut emissions = 0u32;
        for _ in 0..(render_fps * seconds).round() as u32 {
            carry += 1.0 / render_fps;
            let steps = (carry / step_seconds).floor() as u32;
            carry -= steps as f32 * step_seconds;
            for step in 0..steps {
                let timer_due = advance_network_position_timer(
                    &mut timer,
                    step_seconds,
                    NETWORK_RTC_POSITION_INTERVAL,
                );
                let final_step = step + 1 == steps;
                if network_position_emission_due(timer_due, final_step, false, false, false, false)
                {
                    emissions += 1;
                }
            }
        }
        emissions
    }

    #[test]
    fn a_neutral_client_emits_positions_on_every_plausible_render_cadence() {
        // The real defect this was written for: one live client emitted ~59 positions/s while another, in gameplay and
        // sending thousands of other packets, emitted zero. This scan asks whether batching phase alone can starve a
        // client - i.e. whether the timer can come due only on non-final steps, whose due-flag is discarded.
        let _initial_timer = NETWORK_RTC_POSITION_INTERVAL;
        let mut starved = Vec::new();
        for render_fps in [
            30.0f32, 50.0, 59.94, 60.0, 72.0, 75.0, 90.0, 100.0, 120.0, 144.0, 165.0, 240.0,
        ] {
            for step_hz in [60.0f32, 100.0, 120.0] {
                for initial in [NETWORK_RTC_POSITION_INTERVAL, 0.0] {
                    let emitted = neutral_client_emissions(render_fps, step_hz, initial, 10.0);
                    if emitted == 0 {
                        starved.push((render_fps, step_hz, initial));
                    }
                }
            }
        }
        assert!(
            starved.is_empty(),
            "render cadences that emitted zero positions for a neutral client over 10 s: {starved:?} \
             (interval={}s, one step of 100 Hz = {}s)",
            NETWORK_RTC_POSITION_INTERVAL,
            1.0 / 100.0
        );
    }

    #[test]
    fn a_due_flag_on_a_non_final_step_is_not_carried_to_the_final_step() {
        // Pins the lossy part of the design explicitly: the timer resets when it fires, and emission only consults the
        // flag produced by the final step of the batch. A mid-batch fire is therefore dropped, not deferred.
        let mut timer = 0.005;
        assert!(
            advance_network_position_timer(&mut timer, 0.01, NETWORK_RTC_POSITION_INTERVAL),
            "a step that crosses zero must report due"
        );
        assert!(
            !network_position_emission_due(true, false, false, false, false, false),
            "a due flag on a non-final step must not emit"
        );
        assert!(
            !advance_network_position_timer(&mut timer, 0.01, NETWORK_RTC_POSITION_INTERVAL),
            "after firing the timer resets, so the flag is not still set on the next step"
        );
    }
}

/// A client-supplied identity collision never authorizes eviction or changing
/// identity. Retry the same login on the existing reliable connection while
/// the server's ordinary stale-session cleanup runs.
#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Default)]
pub(in crate::game) struct IdentityLoginRetry {
    first_rejection_us: Option<u64>,
    next_retry_us: u64,
    exhausted: bool,
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
pub(in crate::game) enum IdentityLoginRetryAction {
    Wait,
    Send,
    ReturnToMenu,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl IdentityLoginRetry {
    pub(in crate::game) fn rejected(&mut self, now_us: u64) {
        if self.first_rejection_us.is_none() && !self.exhausted {
            self.first_rejection_us = Some(now_us);
            self.next_retry_us = now_us.saturating_add(2_000_000);
        }
    }
    pub(in crate::game) fn accepted(&mut self) {
        *self = Self::default();
    }
    pub(in crate::game) fn poll(&mut self, now_us: u64) -> IdentityLoginRetryAction {
        let Some(first) = self.first_rejection_us else {
            return IdentityLoginRetryAction::Wait;
        };
        if self.exhausted {
            return IdentityLoginRetryAction::Wait;
        }
        if now_us.saturating_sub(first) >= 45_000_000 {
            self.exhausted = true;
            return IdentityLoginRetryAction::ReturnToMenu;
        }
        if now_us < self.next_retry_us {
            return IdentityLoginRetryAction::Wait;
        }
        self.next_retry_us = now_us.saturating_add(2_000_000);
        IdentityLoginRetryAction::Send
    }
}

#[cfg(test)]
mod identity_login_retry_tests {
    use super::*;
    #[test]
    fn collision_retries_are_monotonic_bounded_and_duplicates_never_extend_the_deadline() {
        let mut retry = IdentityLoginRetry::default();
        assert_eq!(retry.poll(1), IdentityLoginRetryAction::Wait);
        retry.rejected(10_000_000);
        assert_eq!(retry.poll(9_000_000), IdentityLoginRetryAction::Wait);
        assert_eq!(retry.poll(11_999_999), IdentityLoginRetryAction::Wait);
        assert_eq!(retry.poll(12_000_000), IdentityLoginRetryAction::Send);
        for second in (14..55).step_by(2) {
            retry.rejected(second * 1_000_000);
            assert_eq!(
                retry.poll(second * 1_000_000),
                IdentityLoginRetryAction::Send
            );
        }
        assert_eq!(
            retry.poll(55_000_000),
            IdentityLoginRetryAction::ReturnToMenu
        );
        retry.rejected(56_000_000);
        assert_eq!(retry.poll(60_000_000), IdentityLoginRetryAction::Wait);
    }
    #[test]
    fn acceptance_cancels_retries_and_a_stall_does_not_catch_up_a_burst() {
        let mut retry = IdentityLoginRetry::default();
        retry.rejected(0);
        assert_eq!(retry.poll(25_000_000), IdentityLoginRetryAction::Send);
        assert_eq!(retry.poll(25_000_000), IdentityLoginRetryAction::Wait);
        retry.accepted();
        assert_eq!(retry.poll(100_000_000), IdentityLoginRetryAction::Wait);
        retry.rejected(101_000_000);
        assert_eq!(retry.poll(103_000_000), IdentityLoginRetryAction::Send);
    }
}
