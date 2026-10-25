use crate::arena_settings::arena_settings_packet_for_zone;
use crate::authority::{StatusInventory, WeaponInventory};
use crate::authority_identity::{PlayerRuntimeIdentity, PlayerSessionIdentity};
use crate::clock_bootstrap::ServerClockBootstrap;
use crate::hidden_prizes::current_hidden_prize_packets_for_state_at_tick;
use crate::objectives::{
    ObjectiveLifecycleReason, clear_player_turret_links, current_brick_packets_for_state_at_tick,
    current_flag_packets_for_state, current_powerball_packets_for_state,
    current_turret_packets_for_state, objective_state_replay_packets,
};
use crate::player_events::toggle_send_damage;
use crate::reliable::{
    BootstrapReliableBarrier, ServerReliableReceiver, ServerReliableSender,
    broadcast_reliable_except, complete_bootstrap_delivery, prepare_reliable_payloads,
    prepare_reliable_payloads_for_session_subject_and_recipient,
    send_prepared_reliable_batch_to_session_identity, send_to_session_identity,
};
use crate::roster::{
    arena_directory_listing_for_state, has_banner, player_banner_change, player_entering,
    player_id_assignment, player_leaving,
};
use crate::state::{
    ClientHandle, ClientPacketSender, PLAYER_BANNER_BYTES, PlayerSummary, ServerState,
};
use crate::wire::{current_tick, server_session_id};
use crate::zone::map_information;
#[cfg(test)]
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::set_coordinates_packet;
use super::zones::select_arena_zone_for_login_in_state;

#[cfg(test)]
pub(crate) fn register_connection(state: &Arc<Mutex<ServerState>>, tx: Sender<Vec<u8>>) -> u16 {
    register_connection_with_sender(state, tx.into()).player_id
}

#[cfg(test)]
pub(crate) fn register_connection_with_sender(
    state: &Arc<Mutex<ServerState>>,
    tx: ClientPacketSender,
) -> PlayerSessionIdentity {
    register_connection_with_sender_at_tick(state, tx, current_tick())
}

#[cfg(test)]
fn register_connection_with_sender_at_tick(
    state: &Arc<Mutex<ServerState>>,
    tx: ClientPacketSender,
    tick: u32,
) -> PlayerSessionIdentity {
    let mut state = state.lock().expect("server state should not be poisoned");
    register_connection_with_sender_in_state_at_tick(&mut state, tx, tick)
}

pub(crate) fn register_connection_with_sender_in_state_at_tick(
    state: &mut ServerState,
    tx: ClientPacketSender,
    tick: u32,
) -> PlayerSessionIdentity {
    let id = state.allocate_client_id();
    let session_id = state.allocate_player_session_id();
    let identity = PlayerSessionIdentity::new(id, session_id);
    state.telemetry.sessions_started = state.telemetry.sessions_started.saturating_add(1);
    let simulated_tx_loss = state.simulated_tx_loss.clone();
    let simulated_tx_loss_after_bootstrap = state.config.simulated_tx_loss_after_bootstrap;
    let maxed_spawn_loadout = state.config.maxed_spawn_loadout;
    let send_budget_kib_per_second = state.config.send_budget_kib_per_second;
    state.clients.insert(
        id,
        ClientHandle {
            protocol_revision: 0,
            snapshot_holds: Default::default(),
            maxed_spawn_loadout,
            spawn_idle: Default::default(),
            session_id,
            arena_stream_id: 0,
            inputs_received: 0,
            positions_received: 0,
            ack_ids_logged: 0,
            next_realtime_event_id: 0,
            qa_trace_bundle_id: None,
            threat_presentation: Default::default(),
            ingress_gate: Arc::new(Mutex::new(())),
            tx,
            simulated_tx_loss,
            simulated_tx_loss_after_bootstrap,
            player: PlayerSummary {
                id,
                name: format!("pilot{id}"),
                ship: 0,
                frequency: id,
                assists: 0,
                kill_points: 0,
                flag_points: 0,
                wins: 0,
                losses: 0,
                flags: 0,
                banner: [0; PLAYER_BANNER_BYTES],
            },
            player_identity: None,
            arena_admission_reserved: false,
            joined: false,
            snapshot_ready: false,
            connection_quality: None,
            bootstrap_reliable_barrier: None,
            outgoing_reliable: ServerReliableSender::default(),
            incoming_reliable: ServerReliableReceiver::default(),
            map_transfer: None,
            last_security_report: None,
            last_security_valid: None,
            door_state: None,
            door_seed_cache: None,
            server_bounty: 0,
            server_energy: 0,
            server_max_energy: 0,
            server_energy_remainder: 0.0,
            last_energy_recharge_tick: None,
            last_continuous_energy_drain_tick: None,
            server_super_until_tick: None,
            server_shield_until_tick: None,
            server_engine_shutdown_start_sequence: None,
            server_engine_shutdown_until_tick: None,
            pending_prize_effect_applications: std::collections::VecDeque::new(),
            prize_effect_application_terminals: std::collections::VecDeque::new(),
            server_emp_start_sequence: None,
            server_emp_until_tick: None,
            server_prize_effect_rng_seed: crate::authority_prizes::SECURITY_PRIZE_SEED,
            server_prize_state_revision: 1,
            last_damage_prize_loss_seq: None,
            last_damage_prize_loss_id: None,
            server_rocket_until_tick: None,
            server_rocket_input_asserted: false,
            server_rocket_authorizations: std::collections::VecDeque::new(),
            server_status: StatusInventory::cleared(),
            server_weapons: WeaponInventory::cleared(),
            last_reported_energy: 0,
            last_position: None,
            aoi_observer_position: None,
            spectator_view_sequence: None,
            spectator_next_input_sequence: None,
            objective_last_safe: false,
            visibility_cell: None,
            last_position_server_tick: None,
            movement_time_budget: Default::default(),
            last_position_relay_at: None,
            position_relay_tokens: 3.0,
            position_relay_burst_count: 0,
            last_weapon_fire_tick: None,
            last_gun_fire_tick: None,
            last_bomb_fire_tick: None,
            objective_weapon_lock_until_tick: None,
            last_accepted_weapon_data: None,
            last_accepted_weapon_tick: None,
            accepted_weapon_witnesses: Default::default(),
            objective_ball_fire_witnesses: Default::default(),
            last_special_weapon_request: None,
            last_brick_drop_tick: None,
            chat_rate_debt_until: None,
            last_ship_change_at: None,
            ship_change_burst_count: 0,
            last_frequency_change_at: None,
            frequency_change_burst_count: 0,
            last_attach_request_at: None,
            attach_request_burst_count: 0,
            alive: false,
            respawn_at_tick: None,
            life_seq: 0,
            life_started_tick: Some(tick),
            last_input_seq: None,
            last_input_record: None,
            recent_input_records: std::collections::VecDeque::with_capacity(
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
            ),
            recent_input_rocket_assertions: std::collections::VecDeque::with_capacity(
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
            ),
            last_position_sequence: None,
            life_position_sequence_base: None,
            pending_frequency_baseline: None,
            pending_warp_prize_relocations: std::collections::VecDeque::new(),
            consumed_warp_prize_relocations: std::collections::VecDeque::new(),
            latest_weapon_seq: None,
            weapon_seq_mask: 0,
            latest_portal_action_id: None,
            portal_action_mask: 0,
            portal_placement_terminals: std::collections::VecDeque::new(),
            portal_warp_terminals: std::collections::VecDeque::new(),
            active_portal: None,
            server_warp_settle_until_tick: None,
            server_warp_bomb_lock_until_tick: None,
            canonical_position_witnesses: std::collections::VecDeque::with_capacity(
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
            ),
            canonical_movement_witness: None,
            canonical_mismatch_log_gate: Default::default(),
            server_sim_state: None,
            server_sim_anchor_tick: None,
            server_sim_input_bits: None,
            movement_drift_x: 0.0,
            movement_drift_y: 0.0,
            pending_movement_correction: None,
            retired_movement_corrections: std::collections::VecDeque::new(),
            pending_reset_application_terminals: std::collections::VecDeque::new(),
            next_movement_correction_generation: 0,
            movement_correction_test_forced: false,
            aoi_tier_memory: std::collections::HashMap::new(),
            server_thrust_acceleration: 0.0,
            server_top_speed: 0.0,
            server_rotation_per_second: 0.0,
            server_recharge_per_second: 0.0,
            server_repel_until_tick: None,
            server_repel_sequence_window: None,
            server_repel_observer_until_tick: None,
            pending_repel_observations: std::collections::VecDeque::new(),
            retired_repel_applications: std::collections::VecDeque::new(),
            position_history: std::collections::VecDeque::with_capacity(
                crate::state::MAX_POSITION_HISTORY + 1,
            ),
            actor_motion_history: crate::combat_timeline::ActorMotionHistory::new(),
            observer_presentation_history: crate::combat_timeline::ObserverPresentationHistory::new(
            ),
            observer_combat_timeline: Default::default(),
            adaptive_presentation: Default::default(),
            retired_projectile_observers: Default::default(),
            snapshot_seq: 0,
            #[cfg(debug_assertions)]
            qa_prize_baseline_delay_start_snapshot: None,
            last_acked_snapshot_id: None,
            projectile_refresh_pending: Default::default(),
            projectile_motion_frozen_published: Default::default(),
            last_projectile_refresh: None,
            snapshot_history: std::collections::VecDeque::new(),
            pending_snapshot_events: crate::snapshot::PendingSnapshotEvents::default(),
            pending_realtime_events: crate::snapshot::PendingRealtimeEvents::default(),
            pending_snapshot_weapon: 0,
            recent_damagers: Vec::new(),
            measured_rtt: None,
            lag_compensation_ticks: crate::state::DEFAULT_LAG_COMPENSATION_TICKS,
            send_pacing: crate::state::SendPacing::with_budget_kib(send_budget_kib_per_second),
        },
    );
    identity
}

#[cfg(test)]
pub(crate) fn join_arena(state: &Arc<Mutex<ServerState>>, player_id: u16, ship: u8) {
    join_arena_at_tick(state, player_id, ship, current_tick());
}

/// Select the requested bundled zone and publish the player's joined state in
/// one ServerState transaction. A second client cannot change the single
/// arena between those two facts.
pub(crate) fn join_arena_for_login(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    ship: u8,
    arena_login_payload: &[u8],
) -> Result<(), &'static str> {
    let session = state
        .lock()
        .expect("server state should not be poisoned")
        .active_player_session_identity(player_id);
    let mut result = join_arena_at_tick_inner(
        state,
        player_id,
        ship,
        current_tick(),
        Some(arena_login_payload),
        || {},
        || {},
        || {},
        || {},
    );
    let failed_bootstrap = if result.is_ok() {
        let guard = state.lock().expect("server state should not be poisoned");
        if guard.active_player_session_identity(player_id) != session {
            result = Err("ZUBSPACE_JOIN_FAILED");
            None
        } else {
            guard.clients.get(&player_id).and_then(|client| {
                (client.joined
                    && !client.snapshot_ready
                    && client
                        .bootstrap_reliable_barrier
                        .as_ref()
                        .is_none_or(|barrier| !barrier.initial_delivery_complete()))
                .then(|| guard.active_player_runtime_identity(player_id))
                .flatten()
            })
        }
    } else {
        None
    };
    if let Some(runtime) = failed_bootstrap {
        // The ingress gate is already held by the runtime caller. Use the
        // ordered, exact-runtime leave path rather than recursively taking
        // that gate through transport disconnect. Pending ACKs alone are not
        // failure: a successful first sink pass keeps its reservation.
        let _ = leave_arena_for_runtime(state, Some(runtime), player_id, current_tick());
        result = Err("ZUBSPACE_JOIN_FAILED");
    }
    if result.is_err() {
        let mut state = state.lock().expect("server state should not be poisoned");
        if session.is_some() && state.active_player_session_identity(player_id) == session {
            if let Some(client) = state.clients.get_mut(&player_id) {
                if !client.joined {
                    client.arena_admission_reserved = false;
                }
            }
        }
    }
    result
}

#[cfg(test)]
pub(crate) fn join_arena_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    ship: u8,
    tick: u32,
) {
    join_arena_at_tick_inner(
        state,
        player_id,
        ship,
        tick,
        None,
        || {},
        || {},
        || {},
        || {},
    )
    .expect("joining the current arena cannot fail zone selection");
}

#[cfg(test)]
pub(crate) fn join_arena_at_tick_with_objective_capture_hook(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    ship: u8,
    tick: u32,
    after_objective_capture: impl FnOnce(),
) {
    join_arena_at_tick_inner(
        state,
        player_id,
        ship,
        tick,
        None,
        || {},
        after_objective_capture,
        || {},
        || {},
    )
    .expect("joining the current arena cannot fail zone selection");
}

#[cfg(test)]
pub(crate) fn join_arena_at_tick_with_delivery_hooks(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    ship: u8,
    tick: u32,
    before_bootstrap_enqueue: impl FnOnce(),
    after_bootstrap_enqueue: impl FnOnce(),
) {
    join_arena_at_tick_inner(
        state,
        player_id,
        ship,
        tick,
        None,
        || {},
        || {},
        before_bootstrap_enqueue,
        after_bootstrap_enqueue,
    )
    .expect("joining the current arena cannot fail zone selection");
}

#[cfg(test)]
pub(crate) fn join_arena_for_login_at_tick_with_zone_selection_hook(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    ship: u8,
    arena_login_payload: &[u8],
    tick: u32,
    after_zone_selection: impl FnOnce(),
) -> Result<(), &'static str> {
    join_arena_at_tick_inner(
        state,
        player_id,
        ship,
        tick,
        Some(arena_login_payload),
        after_zone_selection,
        || {},
        || {},
        || {},
    )
}

fn join_arena_at_tick_inner(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    ship: u8,
    tick: u32,
    arena_login_payload: Option<&[u8]>,
    after_zone_selection: impl FnOnce(),
    after_objective_capture: impl FnOnce(),
    before_bootstrap_enqueue: impl FnOnce(),
    after_bootstrap_enqueue: impl FnOnce(),
) -> Result<(), &'static str> {
    let reliable_sent_at = Instant::now();
    let (
        joining_identity,
        self_summary,
        existing_player_entries,
        existing_positions,
        initial_reliable_batch,
    ) = {
        let mut state = state.lock().expect("server state should not be poisoned");
        if let Some(payload) = arena_login_payload {
            select_arena_zone_for_login_in_state(&mut state, payload)?;
        }
        let name = state
            .clients
            .get(&player_id)
            .ok_or("ZUBSPACE_ID_INVALID")?
            .player
            .name
            .clone();
        crate::identity::reserve_arena_slot_in_state(&mut state, player_id, &name)
            .map_err(|error| error.code())?;
        // Test hook runs while the state lock remains held, immediately after
        // requested-zone selection and before joined-state publication.
        after_zone_selection();
        // A fresh arena/transport stream cannot bypass an older identity hold.
        // Always enter as an observer; returning requires a new healthy window
        // plus an explicit ship request after the original deadline.
        let quality_hold = crate::connection_quality::hold_for_client(&state, player_id);
        let ship = if quality_hold.is_some() { 8 } else { ship };
        let arena_stream_id = state
            .allocate_arena_stream_id()
            .ok_or("arena stream identity space exhausted")?;
        let observer_timeline_generation = state.allocate_observer_timeline_generation();
        let observer_network_profile = state.config.observer_network_profile;
        let sequence_base = join_sequence_base(player_id, tick);
        let joining_identity = state
            .active_player_session_identity(player_id)
            .expect("joining client should be registered");
        let qa_trace_context = (cfg!(debug_assertions)
            || state.config.qa_trace_upload
            || state.config.client_debug_trace)
            .then(|| state.qa_traces.register(joining_identity, arena_stream_id));
        let existing_player_entries: Vec<(PlayerSessionIdentity, PlayerSummary)> = state
            .clients
            .iter()
            .filter_map(|(&id, client)| {
                (id != player_id && client.joined).then_some((
                    PlayerSessionIdentity::new(id, client.session_id),
                    client.player.clone(),
                ))
            })
            .collect();
        let existing_players: Vec<PlayerSummary> = existing_player_entries
            .iter()
            .map(|(_, player)| player.clone())
            .chain(state.bots.iter().map(|bot| bot.player.clone()))
            .collect();
        let existing_positions: Vec<Vec<u8>> = state
            .clients
            .iter()
            .filter_map(|(&id, client)| {
                (id != player_id && client.joined)
                    .then(|| client.last_position.clone())
                    .flatten()
            })
            .chain(
                state
                    .bots
                    .iter()
                    .filter(|bot| bot.alive())
                    .map(|bot| bot.large_position_packet()),
            )
            .map(|mut position| {
                // This is a new observer's initial current-state projection,
                // not a replay of historical movement. Seed its position clock
                // at the same request tick as bootstrap/successor snapshots,
                // including actors idle longer than the legacy half-range and
                // bots created before their first simulation step. Do not
                // mutate the source actor's stored position or motion history.
                position[2..4].copy_from_slice(&(tick as u16).to_le_bytes());
                position
            })
            .collect();
        // A new arena stream is a hard authority boundary. Public combat
        // observations may cross only soft life cutovers on the same stream,
        // never a leave/rejoin or zone replacement.
        if let Some(client) = state.clients.get_mut(&player_id) {
            client.pending_snapshot_events = crate::snapshot::PendingSnapshotEvents::default();
            client.pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
            client.threat_presentation.clear();
            client.projectile_refresh_pending.clear();
            client.projectile_motion_frozen_published.clear();
            client.last_projectile_refresh = None;
        }
        state
            .advance_client_life_at_tick(player_id, tick)
            .expect("joining client should be registered");
        state.set_client_spawn_aoi_observer(player_id);
        let client = state
            .clients
            .get_mut(&player_id)
            .expect("joining client should be registered");
        client.retired_movement_corrections.clear();
        client.adaptive_presentation = Default::default();
        client.player.ship = ship;
        client.arena_stream_id = arena_stream_id;
        if let Some(quality) = &client.connection_quality {
            quality
                .lock()
                .expect("session quality lock poisoned")
                .reset_scope(arena_stream_id, std::time::Instant::now());
        }
        // `advance_client_life_at_tick` ran before this fresh stream was
        // allocated, so replace its registration-time segment explicitly.
        // No actor motion or presentation sample may retain arena stream 0.
        client.begin_actor_motion_segment();
        let observer_runtime = PlayerRuntimeIdentity::new(
            joining_identity,
            crate::authority_identity::LifeIdentity::new(player_id, client.life_seq),
        );
        client
            .observer_presentation_history
            .begin_scope(observer_runtime, arena_stream_id);
        client
            .observer_combat_timeline
            .begin_scope(
                observer_runtime,
                arena_stream_id,
                observer_network_profile,
                observer_timeline_generation,
                tick,
                sequence_base,
            )
            .expect("fresh join creates a valid observer timeline scope");
        client.next_realtime_event_id = 0;
        client.qa_trace_bundle_id = qa_trace_context.map(|context| context.bundle_id);
        // Do not expose this player to other joiners until their own
        // assignment/bootstrap packets have been queued below. Otherwise
        // simultaneous joins can deliver PlayerEntered before
        // PlayerIdAssigned; the client clears roster state on assignment and
        // loses that early remote player permanently.
        client.joined = false;
        client.snapshot_ready = false;
        client.bootstrap_reliable_barrier = None;
        client.alive = ship < 8;
        client.respawn_at_tick = None;
        // A fresh arena stream cannot inherit the last safe-zone witness from
        // the preceding join. Same-arena deaths deliberately preserve it in
        // `clear_client_position`, but join/session boundaries do not.
        client.objective_last_safe = false;
        // Map/WASM bootstrap can take materially longer in Firefox than the
        // reliable roster bootstrap. Until the first canonical position is
        // admitted, retain a read-only spawn anchor so snapshots can present
        // nearby bots instead of emitting an empty AOI. This never populates
        // `last_position`, simulation state, or any self-authority field;
        // `set_client_position` replaces it with the first accepted position.
        client.reset_server_runtime_for_ship_at_tick(tick);
        // The clock bootstrap and server authority must name the same first
        // exact lifecycle record. Current-protocol clients cannot seed an
        // arbitrary hidden movement state after join; their one neutral
        // spawn report at this base establishes the server-known f32 anchor.
        #[cfg(not(test))]
        {
            client.life_position_sequence_base = Some(sequence_base);
        }
        let self_summary = client.player.clone();

        // Capture every objective fact under this same request-entry tick and
        // state transaction. In particular, hidden-prize TTLs are derived
        // here rather than after the assignment/map send phases.
        let mut objective_replay = current_flag_packets_for_state(&state);
        objective_replay.extend(current_turret_packets_for_state(&state));
        objective_replay.extend(current_brick_packets_for_state_at_tick(&mut state, tick));
        objective_replay.extend(current_powerball_packets_for_state(&state));
        objective_replay.extend(objective_state_replay_packets(&state, tick));
        objective_replay.extend(current_hidden_prize_packets_for_state_at_tick(
            &mut state, tick,
        ));
        after_objective_capture();

        // Queue the complete reliable bootstrap before exposing the client as
        // joined. Later objective mutations then reserve strictly newer ids,
        // while the actual packet sink remains outside this global lock.
        let client = state
            .clients
            .get_mut(&player_id)
            .expect("joining client should remain registered");
        client.joined = true;
        client.snapshot_ready = false;
        client.alive = ship < 8;
        client.respawn_at_tick = None;

        let map = state.zone_map();
        let mut initial_payloads = Vec::new();
        // The bootstrap is the hard authority-generation boundary. Publish it
        // before the replacement PlayerId and JoinGame facts so a client that
        // correctly clears predecessor identity at the boundary cannot be
        // stranded waiting for one-shot facts the server never retries.
        initial_payloads.push(join_clock_bootstrap_packet_at_tick(
            player_id,
            arena_stream_id,
            tick,
            state.config.tick_rate_hz,
        ));
        initial_payloads.push(player_id_assignment(player_id));
        initial_payloads.push(vec![0x02]);
        initial_payloads.push(player_entering(&self_summary));
        if let Some(hold) = quality_hold {
            initial_payloads.push(crate::connection_quality::hold_packet(
                &state.clients[&player_id],
                hold,
                reliable_sent_at,
                None,
            ));
        }
        for player in &existing_players {
            initial_payloads.push(player_entering(player));
            if has_banner(&player.banner) {
                initial_payloads.push(player_banner_change(player.id, &player.banner));
            }
        }
        initial_payloads.push(
            arena_settings_packet_for_zone(&state.config.default_zone)
                .expect("server state must name a bundled zone")
                .to_vec(),
        );
        // Server-authoritative projectile damage is mandatory; clients should
        // not send legacy client-timestamped damage reports.
        initial_payloads.push(toggle_send_damage(false));
        initial_payloads.push(map_information(map));
        if self_summary.ship < 8 {
            initial_payloads.push(set_coordinates_packet(map));
        }
        initial_payloads.push(arena_directory_listing_for_state(&state));
        initial_payloads.extend(objective_replay);
        // Diagnostics metadata must not interpose itself between classic
        // lifecycle/roster facts. It remains inside the same complete
        // bootstrap barrier, so snapshots still cannot overtake it.
        if let Some(context) = qa_trace_context {
            initial_payloads.push(context.encode());
        }

        let client = state
            .clients
            .get_mut(&player_id)
            .expect("joining client should remain registered");
        client.map_transfer = Some(crate::state::MapTransferState {
            held_files: Default::default(),
            counted_held_files: Default::default(),
            sent_files: Default::default(),
            held_bytes: 0,
            sent_bytes: 0,
            completed_transfers: 0,
            started_at: std::time::Instant::now(),
            summary_logged: false,
            zone_id: map.zone_id.as_ref(),
            next_index: 0,
            pending_index: None,
            deferred_successor_index: None,
            active: None,
            file_count: (1 + map.lvz_files.len()) as u16,
            map_sha256: map.sha256,
            redrive_count: 0,
        });
        let initial_reliable_batch =
            prepare_reliable_payloads(client, &initial_payloads, reliable_sent_at);
        client.bootstrap_reliable_barrier = initial_reliable_batch
            .bootstrap_ids()
            .map(|ids| BootstrapReliableBarrier::new(joining_identity.session_id, ids));
        (
            joining_identity,
            self_summary,
            existing_player_entries,
            existing_positions,
            initial_reliable_batch,
        )
    };

    before_bootstrap_enqueue();
    let bootstrap_enqueued = send_prepared_reliable_batch_to_session_identity(
        state,
        joining_identity,
        initial_reliable_batch,
    );
    after_bootstrap_enqueue();
    if !complete_bootstrap_delivery(state, joining_identity, bootstrap_enqueued) {
        return Ok(());
    }

    for position in existing_positions {
        let _ = send_to_session_identity(state, joining_identity, position);
    }

    let mut self_fanout = vec![player_entering(&self_summary)];
    if has_banner(&self_summary.banner) {
        self_fanout.push(player_banner_change(player_id, &self_summary.banner));
    }
    for (existing_identity, _) in &existing_player_entries {
        if let Some(batch) = prepare_reliable_payloads_for_session_subject_and_recipient(
            state,
            joining_identity,
            *existing_identity,
            &self_fanout,
            reliable_sent_at,
        ) {
            let _ = batch.send();
        }
    }
    Ok(())
}

fn join_clock_bootstrap_packet_at_tick(
    player_id: u16,
    arena_stream_id: u64,
    server_tick: u32,
    tick_rate_hz: u16,
) -> Vec<u8> {
    let sequence_base = join_sequence_base(player_id, server_tick);
    ServerClockBootstrap {
        server_session_id: server_session_id(),
        arena_stream_id,
        server_tick,
        input_sequence_base: sequence_base,
        position_sequence_base: sequence_base,
        weapon_sequence_base: sequence_base,
        reliable_sequence_base: 0,
        tick_rate_hz,
        snapshot_rate_hz: crate::snapshot::SNAPSHOT_HZ,
    }
    .encode()
}

fn join_sequence_base(player_id: u16, server_tick: u32) -> u16 {
    (server_tick as u16)
        .wrapping_mul(31)
        .wrapping_add(player_id)
}

#[cfg(test)]
pub(crate) fn leave_arena(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let _ = leave_arena_for_runtime(state, None, player_id, crate::wire::current_tick());
}

pub(crate) fn leave_arena_for_runtime(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<crate::authority_identity::PlayerRuntimeIdentity>,
    player_id: u16,
    tick: u32,
) -> crate::state::ReliableLifecycleMutationOutcome {
    use crate::state::ReliableLifecycleMutationOutcome;

    let (was_joined, objective_packets) = {
        let mut state = state.lock().expect("server state should not be poisoned");
        if expected_identity.is_some()
            && state.active_player_runtime_identity(player_id) != expected_identity
        {
            return ReliableLifecycleMutationOutcome::TerminalNoop;
        }
        let was_joined = state
            .clients
            .get(&player_id)
            .is_some_and(|client| client.joined);
        let objective_packets = if was_joined {
            let Ok(packets) = crate::objectives::player_lifecycle_packets(
                &mut state,
                player_id,
                None,
                ObjectiveLifecycleReason::SessionOrArena,
                tick,
            ) else {
                tracing::error!(
                    player_id,
                    "arena leave retained: objective cleanup could not commit"
                );
                return ReliableLifecycleMutationOutcome::Deferred;
            };
            packets
        } else {
            Vec::new()
        };
        if was_joined {
            publish_departing_weapon_cutoff(&mut state, player_id, tick);
            let leaving = state
                .active_player_session_identity(player_id)
                .expect("a joined departing player has a session");
            state
                .active_decoys
                .retain(|decoy| decoy.owner.session != leaving);
        }
        crate::identity::persist_stats_for_departure(&mut state, player_id);
        if let Some(client) = state.clients.get_mut(&player_id) {
            client.joined = false;
            client.arena_admission_reserved = false;
            client.snapshot_ready = false;
            client.bootstrap_reliable_barrier = None;
            client.map_transfer = None;
            client.alive = false;
            client.respawn_at_tick = None;
            client.player.flags = 0;
            client.objective_last_safe = false;
            client.pending_snapshot_events = crate::snapshot::PendingSnapshotEvents::default();
            client.pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
            state.clear_client_position(player_id);
            state.remove_projectiles_for_player(player_id);
        }
        (was_joined, objective_packets)
    };
    if was_joined {
        clear_player_turret_links(state, player_id);
        for packet in objective_packets {
            broadcast_reliable_except(state, player_id, packet);
        }
        broadcast_reliable_except(state, player_id, player_leaving(player_id));
    }
    if was_joined {
        ReliableLifecycleMutationOutcome::Applied
    } else {
        ReliableLifecycleMutationOutcome::TerminalNoop
    }
}

#[cfg(test)]
pub(crate) fn disconnect_player(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let identity = state
        .lock()
        .expect("server state should not be poisoned")
        .active_player_session_identity(player_id);
    if let Some(identity) = identity {
        disconnect_player_session(state, identity);
    }
}

pub(crate) fn disconnect_player_session(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
) {
    disconnect_player_session_after_removal(state, identity, || {});
}

pub(crate) fn disconnect_player_session_after_removal(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    after_removal: impl FnOnce(),
) {
    let arena_gate = state
        .lock()
        .expect("server state should not be poisoned")
        .arena_transaction_gate
        .clone();
    let _arena_transaction = arena_gate
        .read()
        .expect("arena transaction gate should not be poisoned");
    let ingress_gate = state
        .lock()
        .expect("server state should not be poisoned")
        .player_session_ingress_gate(identity.player_id);
    let Some(ingress_gate) = ingress_gate else {
        return;
    };
    let _ingress = ingress_gate
        .lock()
        .expect("client ingress gate should not be poisoned");
    let (was_joined, objective_packets) = {
        let mut state = state.lock().expect("server state should not be poisoned");
        // The player id may have been reused after this transport copied its
        // session identity but before its delayed disconnect callback reached
        // the arena. Validate the never-reused session token while holding
        // both the per-id ingress gate and the state lock, before even reading
        // replacement state or staging objective lifecycle mutations.
        if !state.player_session_matches(identity) {
            return;
        }
        let was_joined = state
            .clients
            .get(&identity.player_id)
            .is_some_and(|client| client.joined);
        let objective_packets = if was_joined {
            let Ok(packets) = crate::objectives::player_lifecycle_packets(
                &mut state,
                identity.player_id,
                None,
                ObjectiveLifecycleReason::Disconnect,
                crate::wire::current_tick(),
            ) else {
                tracing::error!(
                    player_id = identity.player_id,
                    "disconnect retained: objective cleanup could not commit"
                );
                return;
            };
            packets
        } else {
            Vec::new()
        };
        if was_joined {
            publish_departing_weapon_cutoff(&mut state, identity.player_id, current_tick());
        }
        crate::identity::persist_stats_for_departure(&mut state, identity.player_id);
        let Some(client) = state.remove_client_for_session(identity) else {
            return;
        };
        state.reserve_disconnecting_player_id(identity);
        debug_assert_eq!(client.joined, was_joined);
        (was_joined, objective_packets)
    };
    after_removal();
    let player_id = identity.player_id;
    if was_joined {
        clear_player_turret_links(state, player_id);
        for packet in objective_packets {
            broadcast_reliable_except(state, player_id, packet);
        }
        broadcast_reliable_except(state, player_id, player_leaving(player_id));
    }
    let released = state
        .lock()
        .expect("server state should not be poisoned")
        .release_disconnecting_player_id(identity);
    debug_assert!(released, "disconnect cleanup should own its id reservation");
}

/// A departing source cannot own a pending realtime cancellation: normal
/// pruning retires that source immediately. Retain its exact launch cutoff
/// in each observer's bounded ordered reliable queue before PlayerLeft, using
/// the existing terminal-on-overflow policy instead of an unbounded orphan map.
fn publish_departing_weapon_cutoff(state: &mut ServerState, player_id: u16, tick: u32) {
    use crate::realtime_event::{
        AuthenticatedRealtimeEvent, RealtimeEventPayload, RealtimeOwnerWeaponCancellation,
    };
    let Some(owner) = state.active_player_runtime_identity(player_id) else {
        return;
    };
    let through = state.next_projectile_group_id;
    if through == 0 {
        return;
    }
    // Groups are allocated globally and never reused. The current upper bound
    // covers this source's earlier lives; a later same-session rejoin launches
    // above it, and another session can never match the cancellation identity.
    let cancellation = RealtimeOwnerWeaponCancellation {
        source_player_id: player_id,
        source_session_id: owner.session.session_id.get(),
        through_shot_group_id: through,
        owner_life_seq: owner.life.life_seq,
        owner_input_sequence: None,
    };
    let mut recipient_attempts = 0;
    for (&id, client) in &mut state.clients {
        if id == player_id || !client.joined {
            continue;
        }
        let event = AuthenticatedRealtimeEvent {
            server_session_id: server_session_id(),
            arena_stream_id: client.arena_stream_id,
            event_id: client.allocate_realtime_event_id(),
            server_tick: tick,
            payload: RealtimeEventPayload::OwnerWeaponsCancelled(cancellation),
        };
        crate::reliable::send_reliable_payload(client, &event.encode());
        recipient_attempts += 1;
    }
    state.qa_traces.record_for_all(serde_json::json!({
        "type":"server_departure_weapon_cancellation", "serverTick":tick,
        "sourcePlayerId":player_id, "sourceSessionId":owner.session.session_id.get().to_string(),
        "ownerLifeSeq":owner.life.life_seq, "throughShotGroupId":through.to_string(),
        "recipientAttempts":recipient_attempts, "transport":"ordered_reliable",
    }));
    tracing::info!(
        player_id,
        session_id = owner.session.session_id.get(),
        owner_life_seq = owner.life.life_seq,
        through_shot_group_id = through,
        recipient_attempts,
        server_tick = tick,
        "departing owner weapon cutoff prepared"
    );
}
