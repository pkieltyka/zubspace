//! Server-nonce delivery evidence selects a bounded per-connection target.
//! No client-reported ping, trace, input timestamp or loss claim enters here.
use super::*;
use crate::combat_timeline::{ObserverNetworkProfile, ObserverTimelineDirectiveV1};

const INCREASE_STABLE: Duration = Duration::from_millis(500);
const DECREASE_STABLE: Duration = Duration::from_secs(10);
const OFFER_LEAD_TICKS: u32 = 50;

#[derive(Clone, Copy, Debug)]
struct Evidence {
    echoes: u64,
    received_samples: usize,
    p95_rtt_ms: f64,
    matured_round_trip_loss: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AdaptivePresentationPolicy {
    last_echoes: Option<u64>,
    candidate: Option<(u8, Instant)>,
    initial_policy_confirmed: bool,
}

impl AdaptivePresentationPolicy {
    fn observe(&mut self, evidence: Evidence, current_target: u8, now: Instant) -> Option<u8> {
        if self.last_echoes == Some(evidence.echoes) {
            return None;
        }
        self.last_echoes = Some(evidence.echoes);
        if evidence.received_samples < 2 {
            self.candidate = None;
            return None;
        }
        let desired_ms = evidence.p95_rtt_ms * 0.5 + 30.0;
        let mut target = if desired_ms <= 70.0 {
            7
        } else if desired_ms <= 100.0 {
            10
        } else if desired_ms <= 150.0 {
            15
        } else {
            20
        };
        // Only mature successes/losses share the loss denominator. An echo
        // still waiting for its timeout must not masquerade as packet loss.
        if evidence.matured_round_trip_loss >= 0.06 {
            target = target.max(15);
        } else if evidence.matured_round_trip_loss >= 0.02 {
            target = target.max(10);
        }
        if target == current_target {
            self.candidate = None;
            self.initial_policy_confirmed = true;
            return None;
        }
        match self.candidate {
            Some((candidate, since)) if candidate == target => {
                // The conservative 200 ms cold start may settle promptly on
                // fresh trusted evidence. Once any initial policy is confirmed,
                // normal reductions retain the full recovery hysteresis.
                let stable = if target > current_target
                    || (!self.initial_policy_confirmed && current_target == 20)
                {
                    INCREASE_STABLE
                } else {
                    DECREASE_STABLE
                };
                (now.saturating_duration_since(since) >= stable).then_some(target)
            }
            _ => {
                self.candidate = Some((target, now));
                None
            }
        }
    }
}

fn measured_evidence(quality: &SessionConnectionQuality) -> Option<Evidence> {
    let mut rtts = quality
        .monitor
        .samples
        .iter()
        .chain(quality.monitor.pending.iter())
        .filter_map(|probe| probe.rtt)
        .collect::<Vec<_>>();
    if rtts.is_empty() {
        return None;
    }
    rtts.sort_unstable();
    let rank = (rtts.len() * 95).div_ceil(100).saturating_sub(1);
    Some(Evidence {
        echoes: quality.echoes,
        received_samples: rtts.len(),
        p95_rtt_ms: rtts[rank].as_secs_f64() * 1_000.0,
        matured_round_trip_loss: quality
            .reading
            .map_or(0.0, |reading| reading.round_trip_loss),
    })
}

fn apply_schedule(
    client: &mut crate::state::ClientHandle,
    directive: ObserverTimelineDirectiveV1,
    spectator: bool,
) -> Result<(), crate::combat_timeline::ObserverScheduleRejection> {
    use crate::combat_timeline::ObserverScheduleRejection;
    // Schedule changes belong to the observer's session/arena clock, including
    // flight from a retiring life. Stage every context together: never publish
    // a new view while an older bomb silently remains on a different clock.
    let mut live = client.observer_combat_timeline.clone();
    let mut retired = client.retired_projectile_observers.clone();
    let observer = live.observer().ok_or(ObserverScheduleRejection::NoScope)?;
    let issue = |timeline: &mut crate::combat_timeline::ObserverCombatTimeline| {
        if spectator {
            timeline.issue_observer_delay_schedule(directive)
        } else {
            timeline.issue_delay_schedule(directive)
        }
    };
    issue(&mut live)?;
    for (life, timeline) in &mut retired {
        if timeline.arena_stream_id() != Some(client.arena_stream_id)
            || !timeline
                .observer()
                .is_some_and(|owner| owner.session == observer.session && owner.life == *life)
        {
            return Err(ObserverScheduleRejection::NoScope);
        }
        issue(timeline)?;
    }
    client.observer_combat_timeline = live;
    client.retired_projectile_observers = retired;
    Ok(())
}

/// Called inside the authoritative tick transaction. The tiny quality lock
/// never awaits RTC work. Three fixed policy choices above the LAN floor do
/// not change local prediction, input time credit, or a shot's rewind ceiling.
pub(crate) fn adapt_in_state(
    state: &mut crate::state::ServerState,
    tick: u32,
    now: Instant,
    host_healthy: bool,
) {
    let mut offers = Vec::new();
    for (&id, client) in &mut state.clients {
        if !client.joined
            || !client.snapshot_ready
            || client.observer_combat_timeline.profile() != ObserverNetworkProfile::AutomaticWan
        {
            continue;
        }
        if !host_healthy {
            client.adaptive_presentation.candidate = None;
            continue;
        }
        if !client.alive && client.player.ship < 8 {
            continue;
        }
        let Some(shared) = client.connection_quality.as_ref() else {
            continue;
        };
        let evidence = {
            let quality = shared.lock().expect("session quality");
            if quality.arena_stream_id != client.arena_stream_id
                || client.adaptive_presentation.last_echoes == Some(quality.echoes)
            {
                continue;
            }
            measured_evidence(&quality)
        };
        let Some(evidence) = evidence else {
            continue;
        };
        let Some(previous) = client.observer_combat_timeline.current_directive() else {
            continue;
        };
        let Some(target) =
            client
                .adaptive_presentation
                .observe(evidence, previous.target_delay_ticks, now)
        else {
            continue;
        };
        if crate::continuum::continuum_tick_diff(tick, previous.issued_server_tick)
            < crate::combat_timeline::OBSERVER_TIMELINE_MIN_GENERATION_TICKS as i32
            || client.observer_combat_timeline.delay_ticks_at(tick)
                != Some(previous.target_delay_ticks)
        {
            continue;
        }
        let spectator = client.player.ship >= 8;
        let boundary = if spectator {
            // The admitted ship request/initial bootstrap names exactly the
            // next input at which the spectator's sequencer stopped.
            client
                .spectator_next_input_sequence
                .or(client.life_position_sequence_base)
        } else {
            client
                .last_input_seq
                .map(|sequence| sequence.wrapping_add(1))
        };
        let Some(boundary) = boundary else {
            continue;
        };
        offers.push((id, target, spectator, boundary, previous, evidence));
    }
    for (id, target, spectator, boundary, previous, evidence) in offers {
        let generation = state.allocate_observer_timeline_generation();
        let client = state.clients.get_mut(&id).expect("tick owns client roster");
        let directive = ObserverTimelineDirectiveV1 {
            generation,
            issued_server_tick: tick,
            effective_server_tick: crate::continuum::continuum_tick_add(tick, OFFER_LEAD_TICKS),
            effective_from_input_sequence: boundary,
            from_delay_ticks: previous.target_delay_ticks,
            target_delay_ticks: target,
            ..previous
        };
        let result = apply_schedule(client, directive, spectator);
        if result.is_ok() {
            client.adaptive_presentation.candidate = None;
            client.adaptive_presentation.initial_policy_confirmed = true;
            tracing::info!(
                player_id = id,
                session_id = client.session_id.get(),
                arena_stream_id = client.arena_stream_id,
                generation,
                previous_target_ticks = previous.target_delay_ticks,
                target_ticks = target,
                ceiling_ticks = directive.ceiling_delay_ticks,
                effective_server_tick = directive.effective_server_tick,
                effective_input_sequence = boundary,
                spectator,
                p95_rtt_ms = evidence.p95_rtt_ms,
                received_samples = evidence.received_samples,
                matured_round_trip_loss = evidence.matured_round_trip_loss,
                "server adaptive presentation target issued"
            );
        } else {
            tracing::debug!(
                player_id = id,
                ?result,
                "server adaptive presentation offer deferred"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat_timeline::{CombatViewReceiptV1, ObserverPresentationFrameV1};
    use crate::state::ServerState;
    use std::sync::{Arc, Mutex, mpsc};

    fn evidence(echoes: u64, rtt: f64) -> Evidence {
        Evidence {
            echoes,
            received_samples: 2,
            p95_rtt_ms: rtt,
            matured_round_trip_loss: 0.0,
        }
    }

    #[test]
    fn conservative_bootstrap_requires_fresh_evidence_then_keeps_recovery_hysteresis() {
        let now = Instant::now();
        let mut fast = AdaptivePresentationPolicy::default();
        assert_eq!(fast.observe(evidence(1, 20.0), 20, now), None);
        assert_eq!(
            fast.observe(evidence(1, 20.0), 20, now + Duration::from_secs(20)),
            None
        );
        assert_eq!(
            fast.observe(evidence(2, 20.0), 20, now + INCREASE_STABLE),
            Some(7)
        );
        // A failed schedule transaction must not falsely mark bootstrap complete.
        assert!(!fast.initial_policy_confirmed);

        let mut slow = AdaptivePresentationPolicy::default();
        assert_eq!(slow.observe(evidence(1, 400.0), 20, now), None);
        assert!(
            slow.initial_policy_confirmed,
            "fresh evidence confirmed the conservative target"
        );
        assert_eq!(
            slow.observe(evidence(2, 20.0), 20, now + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            slow.observe(evidence(3, 20.0), 20, now + Duration::from_secs(2)),
            None
        );
        assert_eq!(
            slow.observe(evidence(4, 20.0), 20, now + Duration::from_secs(11)),
            Some(7)
        );
        let mut unmeasured = AdaptivePresentationPolicy::default();
        let too_few = Evidence {
            received_samples: 1,
            ..evidence(1, 20.0)
        };
        assert_eq!(unmeasured.observe(too_few, 20, now), None);
        assert!(!unmeasured.initial_policy_confirmed);
    }

    #[test]
    fn hysteresis_requires_fresh_server_echoes_and_bounds_loss_targets() {
        let now = Instant::now();
        let mut policy = AdaptivePresentationPolicy::default();
        assert_eq!(policy.observe(evidence(1, 200.0), 7, now), None);
        assert_eq!(
            policy.observe(evidence(1, 200.0), 7, now + Duration::from_secs(20)),
            None,
            "elapsed wall time alone cannot renew stale evidence"
        );
        assert_eq!(
            policy.observe(evidence(2, 200.0), 7, now + INCREASE_STABLE),
            Some(15)
        );
        assert_eq!(
            policy.observe(evidence(3, 20.0), 15, now + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            policy.observe(evidence(4, 20.0), 15, now + Duration::from_secs(10)),
            None
        );
        assert_eq!(
            policy.observe(evidence(5, 20.0), 15, now + Duration::from_secs(11)),
            Some(7)
        );
        for (loss, expected) in [(0.02, 10), (0.06, 15)] {
            let mut policy = AdaptivePresentationPolicy::default();
            let e = Evidence {
                matured_round_trip_loss: loss,
                ..evidence(1, 20.0)
            };
            assert_eq!(policy.observe(e, 7, now), None);
            assert_eq!(
                policy.observe(Evidence { echoes: 2, ..e }, 7, now + INCREASE_STABLE),
                Some(expected)
            );
        }
        let mut policy = AdaptivePresentationPolicy::default();
        assert_eq!(policy.observe(evidence(1, 2_500.0), 7, now), None);
        assert_eq!(
            policy.observe(evidence(2, 2_500.0), 7, now + INCREASE_STABLE),
            Some(20)
        );
    }

    #[test]
    fn retired_context_failure_cannot_partially_commit_a_new_clock() {
        use crate::authority_identity::{
            LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
        };
        use crate::combat_timeline::{ObserverCombatTimeline, ObserverScheduleRejection};
        for wrong_session in [false, true] {
            let shared = Arc::new(Mutex::new(ServerState::default()));
            let id = peer(&shared, 0, Instant::now());
            let mut state = shared.lock().unwrap();
            let current = state.active_player_runtime_identity(id).unwrap();
            let client = state.clients.get_mut(&id).unwrap();
            let initial = client.observer_combat_timeline.current_directive().unwrap();
            let old = PlayerRuntimeIdentity::new(
                PlayerSessionIdentity::new(
                    id,
                    PlayerSessionId::new(
                        current.session.session_id.get() + u64::from(wrong_session),
                    ),
                ),
                LifeIdentity::new(id, current.life.life_seq.saturating_sub(1).max(1)),
            );
            let mut context = ObserverCombatTimeline::default();
            context
                .begin_scope(
                    old,
                    client.arena_stream_id,
                    if wrong_session {
                        ObserverNetworkProfile::AutomaticWan
                    } else {
                        ObserverNetworkProfile::CleanLan
                    },
                    initial.generation,
                    1_000,
                    initial.effective_from_input_sequence,
                )
                .unwrap();
            client
                .retired_projectile_observers
                .insert(old.life, context);
            let live_before = client.observer_combat_timeline.clone();
            let retired_before = client.retired_projectile_observers.clone();
            let offer = ObserverTimelineDirectiveV1 {
                generation: initial.generation.wrapping_add(1).max(1),
                issued_server_tick: 1_100,
                effective_server_tick: 1_150,
                effective_from_input_sequence: initial
                    .effective_from_input_sequence
                    .wrapping_add(1),
                target_delay_ticks: 15,
                ..initial
            };
            assert_eq!(
                apply_schedule(client, offer, false),
                Err(if wrong_session {
                    ObserverScheduleRejection::NoScope
                } else {
                    ObserverScheduleRejection::ProfileChanged
                })
            );
            assert_eq!(client.observer_combat_timeline, live_before);
            assert_eq!(client.retired_projectile_observers, retired_before);
        }
    }

    fn peer(state: &Arc<Mutex<ServerState>>, ship: u8, now: Instant) -> u16 {
        let (tx, _rx) = mpsc::channel();
        let id = crate::arena::register_connection(state, tx);
        crate::arena::join_arena(state, id, ship);
        let mut locked = state.lock().unwrap();
        let generation = locked.allocate_observer_timeline_generation();
        let runtime = locked.active_player_runtime_identity(id).unwrap();
        let profile = locked.config.observer_network_profile;
        let client = locked.clients.get_mut(&id).unwrap();
        let sequence = 100 + id * 128;
        client
            .observer_combat_timeline
            .begin_scope(
                runtime,
                client.arena_stream_id,
                profile,
                generation,
                1_000,
                sequence,
            )
            .unwrap();
        client
            .observer_presentation_history
            .begin_scope(runtime, client.arena_stream_id);
        // Production join/ship admission stores the exact bootstrap/request
        // sequence. The older native join adapter omits that assignment.
        client.life_position_sequence_base = Some(sequence);
        client.snapshot_ready = true;
        let mut quality = SessionConnectionQuality::new(now);
        quality.reset_scope(client.arena_stream_id, now);
        client.connection_quality = Some(Arc::new(Mutex::new(quality)));
        id
    }

    fn echo(state: &mut ServerState, id: u16, step: u32, now: Instant, rtt: Duration) {
        let client = &state.clients[&id];
        let mut quality = client.connection_quality.as_ref().unwrap().lock().unwrap();
        let sent = now + Duration::from_millis(u64::from(step) * 500);
        let probe = quality.reserve_probe(u64::from(step) + 1, sent).unwrap();
        assert!(quality.begin_dispatch(probe, sent));
        let mut forged = probe;
        forged.nonce ^= 0x8000_0000;
        assert!(!quality.receive_echo(forged, sent + rtt));
        assert!(quality.receive_echo(probe, sent + rtt));
        quality.poll(sent + Duration::from_millis(250), true);
    }

    fn player_input(state: &mut ServerState, id: u16, step: u32, tick: u32) {
        let runtime = state.active_player_runtime_identity(id).unwrap();
        let client = state.clients.get_mut(&id).unwrap();
        let sequence = client
            .life_position_sequence_base
            .unwrap()
            .wrapping_add(step as u16);
        let directive = client.observer_combat_timeline.current_directive().unwrap();
        let snapshot_id = step as u16 + 1;
        client
            .observer_presentation_history
            .observe_emitted(ObserverPresentationFrameV1 {
                observer: runtime,
                arena_stream_id: client.arena_stream_id,
                snapshot_id,
                server_tick: tick,
                actors: Vec::new(),
            });
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: client.life_seq,
            input_sequence: sequence,
            timeline_generation: directive.generation,
            latest_game_applied_snapshot_id: snapshot_id,
            flags: 0,
        };
        client
            .observer_combat_timeline
            .verify_receipt(
                runtime,
                tick,
                receipt,
                &client.observer_presentation_history,
            )
            .unwrap();
        client.last_input_seq = Some(sequence);
    }

    #[test]
    fn same_server_selects_independent_targets_and_recovers_from_measured_rtt() {
        let now = Instant::now();
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let fast = peer(&shared, 0, now);
        let slow = peer(&shared, 0, now);
        let mut state = shared.lock().unwrap();
        let initial_energy = state.clients[&slow].server_energy;
        // A retained context represents flight that survives this soft life.
        // Registry lifetime itself is covered by the server combat tests.
        let old_runtime = state.active_player_runtime_identity(slow).unwrap();
        let old_binding = {
            let client = state.clients.get_mut(&slow).unwrap();
            let sequence = client.life_position_sequence_base.unwrap();
            let generation = client
                .observer_combat_timeline
                .current_directive()
                .unwrap()
                .generation;
            client
                .observer_presentation_history
                .observe_emitted(ObserverPresentationFrameV1 {
                    observer: old_runtime,
                    arena_stream_id: client.arena_stream_id,
                    snapshot_id: u16::MAX,
                    server_tick: 1_000,
                    actors: Vec::new(),
                });
            client
                .observer_combat_timeline
                .verify_receipt(
                    old_runtime,
                    1_000,
                    CombatViewReceiptV1 {
                        observer_life_seq: client.life_seq,
                        input_sequence: sequence,
                        timeline_generation: generation,
                        latest_game_applied_snapshot_id: u16::MAX,
                        flags: 0,
                    },
                    &client.observer_presentation_history,
                )
                .unwrap();
            let binding = client
                .observer_combat_timeline
                .bind_projectile(old_runtime, sequence, 1_000)
                .unwrap();
            client.retired_projectile_observers.insert(
                old_runtime.life,
                client
                    .observer_combat_timeline
                    .retained_projectile_context(),
            );
            binding
        };
        state.advance_client_life_at_tick(slow, 1_000).unwrap();
        // The successor's authoritative baseline continues at its own next
        // sequence, as the respawn/lifecycle assignment does in production.
        state
            .clients
            .get_mut(&slow)
            .unwrap()
            .life_position_sequence_base = Some(old_binding.owner_input_sequence.wrapping_add(1));
        let mut transitions = Vec::new();
        let mut previous = 20;
        for step in 0..=120 {
            let at = now + Duration::from_millis(u64::from(step) * 500 + 250);
            let tick = 1_025 + step * 50;
            echo(&mut state, fast, step, now, Duration::from_millis(20));
            echo(
                &mut state,
                slow,
                step,
                now,
                // Settle the conservative bootstrap, then degrade/recover.
                Duration::from_millis(if (14..27).contains(&step) { 200 } else { 20 }),
            );
            player_input(&mut state, fast, step, tick);
            player_input(&mut state, slow, step, tick);
            adapt_in_state(&mut state, tick, at, true);
            assert_eq!(
                state.clients[&fast]
                    .observer_combat_timeline
                    .current_directive()
                    .unwrap()
                    .target_delay_ticks,
                if step < 2 { 20 } else { 7 }
            );
            let directive = state.clients[&slow]
                .observer_combat_timeline
                .current_directive()
                .unwrap();
            assert_eq!(directive.ceiling_delay_ticks, 20);
            let source = &state.clients[&slow];
            let retired = &source.retired_projectile_observers[&old_runtime.life];
            assert_eq!(retired.observer(), Some(old_runtime));
            let query = retired
                .projectile_presentation_query(
                    old_binding,
                    tick,
                    &source.observer_presentation_history,
                )
                .unwrap();
            assert_eq!(
                query.render_tick,
                source
                    .observer_combat_timeline
                    .render_tick_at(tick)
                    .unwrap(),
                "surviving old-life flight must use this observer's current server clock"
            );
            assert_eq!(
                query.accepted_timeline_generation,
                old_binding.accepted_timeline_generation
            );
            assert_eq!(old_binding.maximum_rewind_ticks, 20);
            if directive.target_delay_ticks != previous {
                transitions.push(directive.target_delay_ticks);
                previous = directive.target_delay_ticks;
            }
            if step == 20 {
                assert_eq!(previous, 15);
                let text = crate::observability::metrics(&state, now);
                assert!(text.contains("zubspace_observer_auto_targets{target_ticks=\"7\"} 1\n"));
                assert!(text.contains("zubspace_observer_auto_targets{target_ticks=\"15\"} 1\n"));
            }
        }
        assert_eq!(transitions, vec![7, 15, 7]);
        assert_eq!(state.clients[&slow].server_energy, initial_energy);
        assert!(
            state.clients[&slow].last_position.is_none(),
            "policy offers cannot invent canonical movement"
        );
    }

    #[test]
    fn stopped_spectator_clock_receives_both_degradation_and_recovery_offers() {
        let now = Instant::now();
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let id = peer(&shared, 0, now);
        let (boundary, view) = {
            let mut state = shared.lock().unwrap();
            player_input(&mut state, id, 0, 1_000);
            let boundary = state.clients[&id].last_input_seq.unwrap().wrapping_add(1);
            let runtime = state.active_player_runtime_identity(id).unwrap();
            assert!(matches!(
                crate::arena::force_quality_spectator_in_state(&mut state, runtime, 1_000, now),
                crate::arena::RosterChangeOutcome::Applied(_)
            ));
            let viewer = state.clients.get_mut(&id).unwrap();
            assert!(
                viewer.life_position_sequence_base.is_none(),
                "forced observer has no user ship-request baseline"
            );
            viewer.last_security_valid = Some(true);
            let view = crate::spectator_view::SpectatorViewRequest {
                target: u16::MAX,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: viewer.arena_stream_id,
                life_seq: viewer.life_seq,
                sequence: 1,
                x: 8_192.0,
                y: 8_192.0,
                next_input_sequence: Some(boundary),
            };
            (boundary, view)
        };
        crate::player_events::update_spectate(&shared, id, &view.encode());
        let mut state = shared.lock().unwrap();
        assert_eq!(
            state.clients[&id].spectator_next_input_sequence,
            Some(boundary)
        );
        let mut client = crate::combat_timeline::ClientObserverCombatTimeline::default();
        let mut targets = Vec::new();
        let original = state.clients[&id]
            .observer_combat_timeline
            .current_directive()
            .unwrap();
        for step in 0..=120 {
            let at = now + Duration::from_millis(u64::from(step) * 500 + 250);
            let tick = 1_025 + step * 50;
            echo(
                &mut state,
                id,
                step,
                now,
                Duration::from_millis(if (14..27).contains(&step) { 200 } else { 20 }),
            );
            adapt_in_state(&mut state, tick, at, true);
            let directive = state.clients[&id]
                .observer_combat_timeline
                .current_directive()
                .unwrap();
            if directive.generation == original.generation {
                assert_eq!(
                    directive, original,
                    "forcing spectator preserves the existing observer-wide clock"
                );
            } else {
                assert_eq!(
                    directive.effective_from_input_sequence, boundary,
                    "new offers name the exact stopped spectator input"
                );
            }
            client.observe_directive(directive, boundary);
            assert_eq!(client.active_directive(), Some(directive));
            if targets.last().copied() != Some(directive.target_delay_ticks) {
                targets.push(directive.target_delay_ticks);
            }
            assert!(state.clients[&id].last_input_seq.is_none());
        }
        assert_eq!(targets, vec![20, 7, 15, 7]);
        assert!(!state.clients[&id].alive);
        assert!(state.clients[&id].last_position.is_none());
    }

    #[test]
    fn host_pressure_discards_candidates_and_explicit_fixed_profiles_stay_fixed() {
        let now = Instant::now();
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let auto = peer(&shared, 0, now);
        shared.lock().unwrap().config.observer_network_profile = ObserverNetworkProfile::CleanLan;
        let fixed = peer(&shared, 0, now);
        let mut state = shared.lock().unwrap();
        for step in 0..=5 {
            let at = now + Duration::from_millis(u64::from(step) * 500 + 250);
            let tick = 1_025 + step * 50;
            for id in [auto, fixed] {
                echo(&mut state, id, step, now, Duration::from_millis(200));
                player_input(&mut state, id, step, tick);
            }
            if step == 2 {
                state.clients[&auto]
                    .connection_quality
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .poll(at, false);
            }
            adapt_in_state(&mut state, tick, at, step != 2);
            assert_eq!(
                state.clients[&auto]
                    .observer_combat_timeline
                    .current_directive()
                    .unwrap()
                    .target_delay_ticks,
                if step == 5 { 15 } else { 20 }
            );
            let fixed_directive = state.clients[&fixed]
                .observer_combat_timeline
                .current_directive()
                .unwrap();
            assert_eq!(
                (
                    fixed_directive.target_delay_ticks,
                    fixed_directive.ceiling_delay_ticks
                ),
                (7, 9)
            );
        }
    }
}
