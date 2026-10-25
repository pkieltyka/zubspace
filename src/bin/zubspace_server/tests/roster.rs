use super::*;

#[derive(Debug, PartialEq)]
struct FrequencyAuthorityState {
    player: PlayerSummary,
    alive: bool,
    respawn_at_tick: Option<u32>,
    life_seq: u32,
    life_started_tick: Option<u32>,
    door_seed_cache: Option<crate::doors::DoorSeedCache>,
    last_input_seq: Option<u16>,
    last_input_record: Option<crate::state::ClientInputObservation>,
    recent_input_records: std::collections::VecDeque<crate::state::SequencedInputObservation>,
    recent_input_rocket_assertions: std::collections::VecDeque<(u16, bool)>,
    last_position: Option<Vec<u8>>,
    visibility_cell: Option<crate::visibility::VisibilityCell>,
    visibility_index: crate::visibility::VisibilityIndex,
    last_position_server_tick: Option<u32>,
    last_position_sequence: Option<u16>,
    position_history: std::collections::VecDeque<crate::state::RecentPositionSample>,
    actor_motion_history: crate::combat_timeline::ActorMotionHistory,
    server_sim_state: Option<crate::state::ServerSimMovement>,
    server_sim_anchor_tick: Option<u32>,
    server_sim_input_bits: Option<u16>,
    movement_drift: (f32, f32),
    last_position_relay_at: Option<Instant>,
    position_relay_tokens: f32,
    position_relay_burst_count: u8,
    last_frequency_change_at: Option<Instant>,
    frequency_change_burst_count: u8,
    server_energy: u16,
    server_energy_remainder: f32,
    server_status: crate::authority::StatusInventory,
    server_weapons: crate::authority::WeaponInventory,
    server_rocket_until_tick: Option<u32>,
    server_rocket_input_asserted: bool,
    server_rocket_authorizations:
        std::collections::VecDeque<crate::state::ServerRocketAuthorization>,
    pending_warp_prize_relocations:
        std::collections::VecDeque<crate::state::PendingWarpPrizeRelocation>,
    life_position_sequence_base: Option<u16>,
    pending_frequency_baseline: Option<crate::continuum_position::ClientPositionSnapshot>,
    pending_position_inputs: usize,
    active_projectiles: usize,
}

fn frequency_authority_state(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
) -> FrequencyAuthorityState {
    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    FrequencyAuthorityState {
        player: client.player.clone(),
        alive: client.alive,
        respawn_at_tick: client.respawn_at_tick,
        life_seq: client.life_seq,
        life_started_tick: client.life_started_tick,
        door_seed_cache: client.door_seed_cache,
        last_input_seq: client.last_input_seq,
        last_input_record: client.last_input_record,
        recent_input_records: client.recent_input_records.clone(),
        recent_input_rocket_assertions: client.recent_input_rocket_assertions.clone(),
        last_position: client.last_position.clone(),
        visibility_cell: client.visibility_cell,
        visibility_index: state.visibility_index.clone(),
        last_position_server_tick: client.last_position_server_tick,
        last_position_sequence: client.last_position_sequence,
        position_history: client.position_history.clone(),
        actor_motion_history: client.actor_motion_history.clone(),
        server_sim_state: client.server_sim_state,
        server_sim_anchor_tick: client.server_sim_anchor_tick,
        server_sim_input_bits: client.server_sim_input_bits,
        movement_drift: (client.movement_drift_x, client.movement_drift_y),
        last_position_relay_at: client.last_position_relay_at,
        position_relay_tokens: client.position_relay_tokens,
        position_relay_burst_count: client.position_relay_burst_count,
        last_frequency_change_at: client.last_frequency_change_at,
        frequency_change_burst_count: client.frequency_change_burst_count,
        server_energy: client.server_energy,
        server_energy_remainder: client.server_energy_remainder,
        server_status: client.server_status,
        server_weapons: client.server_weapons,
        server_rocket_until_tick: client.server_rocket_until_tick,
        server_rocket_input_asserted: client.server_rocket_input_asserted,
        server_rocket_authorizations: client.server_rocket_authorizations.clone(),
        pending_warp_prize_relocations: client.pending_warp_prize_relocations.clone(),
        life_position_sequence_base: client.life_position_sequence_base,
        pending_frequency_baseline: client.pending_frequency_baseline,
        pending_position_inputs: state
            .pending_position_inputs
            .queued_count_for_player(player_id),
        active_projectiles: state.active_projectiles.len(),
    }
}

fn read_frequency_roster(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let cutover = read_reliable_payload_from_channel(rx);
    assert_eq!(
        cutover.first().copied(),
        Some(crate::roster::FREQUENCY_CUTOVER_ACK)
    );
    read_reliable_payload_from_channel(rx)
}

fn current_ship_frequency_change_payload(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
) -> Vec<u8> {
    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    crate::roster::ship_frequency_change(&client.player, client.life_seq)
}

#[test]
fn authoritative_ship_change_requires_sequence_boundary_before_lifecycle_mutation() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let before = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        (
            client.player.clone(),
            client.life_seq,
            client.life_started_tick,
            client.server_energy,
            client.last_position.clone(),
            client.life_position_sequence_base,
        )
    };
    let count_before = crate::security::peek_security_reject_count_for_test(
        "ship_change",
        "missing_position_sequence_base",
    );

    update_ship(&state, player_id, &[SHIP_CHANGE_REQUEST, 1]);

    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    assert_eq!(
        (
            client.player.clone(),
            client.life_seq,
            client.life_started_tick,
            client.server_energy,
            client.last_position.clone(),
            client.life_position_sequence_base,
        ),
        before
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "ship_change",
            "missing_position_sequence_base"
        ),
        count_before + 1
    );
}

#[test]
fn roster_changes_thread_one_request_tick_through_runtime_and_life_at_wrap() {
    for request_tick in [456_789, 0x7fff_fffe] {
        let state = Arc::new(Mutex::new(team_mode_state()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena_at_tick(
            &state,
            player_id,
            0,
            crate::continuum::continuum_tick_sub(request_tick, 100),
        );
        drain_available(&rx);

        update_ship_at_tick(&state, player_id, &ship_change_request(1), request_tick);
        assert_eq!(
            read_reliable_payload_from_channel(&rx),
            current_ship_frequency_change_payload(&state, player_id)
        );
        {
            let locked = state.lock().expect("state lock");
            let client = locked.clients.get(&player_id).expect("client should exist");
            assert_eq!(client.life_started_tick, Some(request_tick));
            assert_eq!(client.last_energy_recharge_tick, Some(request_tick));
            assert_eq!(client.last_continuous_energy_drain_tick, Some(request_tick));
        }
        consume_pending_lifecycle_baseline_at_tick(&state, player_id, request_tick);

        let frequency_tick = crate::continuum::continuum_tick_add(request_tick, 1);
        update_frequency_at_tick(
            &state,
            player_id,
            &frequency_change_request(&state, player_id, 33),
            frequency_tick,
        );
        assert_eq!(
            read_frequency_roster(&rx),
            current_ship_frequency_change_payload(&state, player_id)
        );
        assert_eq!(
            state
                .lock()
                .expect("state lock")
                .clients
                .get(&player_id)
                .and_then(|client| client.life_started_tick),
            Some(frequency_tick)
        );
    }
}

#[test]
fn server_updates_ship_and_frequency_for_joined_players() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let initial_life_seq = state
        .lock()
        .expect("server state should not be poisoned")
        .clients[&player_id]
        .life_seq;

    update_ship(&state, player_id, &ship_change_request(4));
    let ship_change = read_reliable_payload_from_channel(&rx);
    assert_eq!(
        ship_change,
        current_ship_frequency_change_payload(&state, player_id)
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients[&player_id]
            .life_seq,
        initial_life_seq + 1,
        "a ship change starts a distinct authority life"
    );
    consume_pending_lifecycle_baseline(&state, player_id);

    update_frequency(
        &state,
        player_id,
        &frequency_change_request(&state, player_id, 33),
    );
    let frequency_change = read_frequency_roster(&rx);
    assert_eq!(
        frequency_change,
        current_ship_frequency_change_payload(&state, player_id)
    );
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert_eq!(
            state
                .clients
                .get(&player_id)
                .map(|client| client.player.flags),
            Some(0)
        );
        assert!(state.objectives.is_empty());
        assert_eq!(
            state.clients[&player_id].life_seq,
            initial_life_seq + 2,
            "a frequency change invalidates projectiles and observations from the old team"
        );
    }

    broadcast_position(
        &state,
        player_id,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    update_ship(&state, player_id, &ship_change_request(8));
    let spectator_change = read_reliable_payload_from_channel(&rx);
    assert_eq!(
        spectator_change,
        current_ship_frequency_change_payload(&state, player_id)
    );
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&player_id).expect("player should exist");
        assert!(!client.alive);
        assert!(client.last_position.is_none());
        assert!(client.last_position_server_tick.is_none());
        assert_eq!(client.server_bounty, 0);
        assert_eq!(client.server_energy, 0);
    }

    update_ship(&state, player_id, &ship_change_request(2));
    let respawn_change = read_reliable_payload_from_channel(&rx);
    assert_eq!(
        respawn_change,
        current_ship_frequency_change_payload(&state, player_id)
    );
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("player should exist");
    assert!(client.alive);
    assert_eq!(
        client.server_bounty,
        crate::arena_settings::arena_ship_initial_bounty(2)
    );
    assert!(client.server_weapons.proximity_bombs);
    assert_eq!(client.life_seq, initial_life_seq + 4);
}

#[test]
fn playable_ship_change_retains_spawn_aoi_until_exact_position_baseline() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            free_for_all: false,
            bot_count: 2,
            ..ServerConfig::default()
        },
        ..team_mode_state()
    }));
    state.lock().expect("state").initialize_configured_bots();
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    acknowledge_join_bootstrap(&state, player_id);
    drain_available(&rx);

    update_ship(&state, player_id, &ship_change_request(1));

    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    assert!(client.alive);
    assert!(client.last_position.is_none());
    assert!(client.visibility_cell.is_none());
    assert!(client.aoi_observer_position.is_some());
    let (near, mid) = crate::snapshot::compute_aoi_entities(&state, player_id);
    let actor_ids = near
        .iter()
        .map(|(id, _)| *id)
        .chain(mid.iter().map(|(id, _, _, _)| *id))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!actor_ids.contains(&player_id));
    assert_eq!(
        actor_ids,
        [
            crate::state::SERVER_BOT_ID_BASE,
            crate::state::SERVER_BOT_ID_BASE + 1,
        ]
        .into_iter()
        .collect(),
        "ship-change snapshots must not omit nearby bots before the new baseline"
    );
}

#[test]
fn frequency_cutover_is_atomic_weapon_free_wrap_safe_and_replay_protected() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.set(u16::MAX));

    let request = frequency_change_request(&state, player_id, 17);
    let base = u16::from_le_bytes([request[3], request[4]]);
    assert_eq!(base, u16::MAX);
    update_frequency(&state, player_id, &request);
    let cutover = read_reliable_payload_from_channel(&rx);
    assert_eq!(cutover[0], crate::roster::FREQUENCY_CUTOVER_ACK);
    assert_eq!(u16::from_le_bytes([cutover[3], cutover[4]]), base);
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        current_ship_frequency_change_payload(&state, player_id)
    );

    let (identity, expected, before) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (
            locked.active_player_runtime_identity(player_id).unwrap(),
            client.pending_frequency_baseline.expect("pending baseline"),
            (
                client.last_input_seq,
                client.last_position_sequence,
                client.recent_input_records.clone(),
                client.server_energy,
                client.server_weapons,
                locked.active_projectiles.len(),
            ),
        )
    };
    assert_eq!(expected.client_sequence, Some(base));

    // A delayed pre-cutover position (including a weapon) is rejected before
    // its embedded input can seed the new life.
    let mut delayed_old = frequency_baseline_packet(expected, base.wrapping_sub(1));
    delayed_old[20..22].copy_from_slice(&1_u16.to_le_bytes());
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        delayed_old,
        "delayed-old-frequency-peer",
        &mut joined,
    );
    assert_eq!(
        state
            .lock()
            .expect("state")
            .pending_position_inputs
            .queued_count_for_player(player_id),
        0
    );

    // A current-sequence teleport/weapon mismatch must preserve the exact
    // pending baseline and every authority field atomically.
    let mut mismatch = expected;
    mismatch.x = mismatch.x.wrapping_add(64);
    let mut mismatch_packet = frequency_baseline_packet(mismatch, base);
    mismatch_packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
    crate::connection::handle_client_packet(
        &state,
        player_id,
        mismatch_packet,
        "mismatched-frequency-peer",
        &mut joined,
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(
            (
                client.last_input_seq,
                client.last_position_sequence,
                client.recent_input_records.clone(),
                client.server_energy,
                client.server_weapons,
                locked.active_projectiles.len(),
            ),
            before
        );
        assert_eq!(client.pending_frequency_baseline, Some(expected));
    }

    // The exact reliable baseline succeeds. Replaying it cannot consume a
    // weapon or advance either stream; the next wrapped sequence is accepted.
    let exact = crate::reliable::reliable(0, &frequency_baseline_packet(expected, base));
    crate::connection::handle_client_packet(
        &state,
        player_id,
        exact.clone(),
        "exact-frequency-peer",
        &mut joined,
    );
    let queued = state.lock().expect("state").drain_position_inputs();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].3, Some(0));
    let _ = crate::arena::accept_position_relay_for_life_at_tick(
        &state,
        queued[0].0.life,
        &queued[0].1,
        crate::wire::current_tick(),
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.last_position_sequence, Some(base));
        assert_eq!(client.pending_frequency_baseline, None);
    }
    let after_exact = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (
            client.server_energy,
            client.server_weapons,
            locked.active_projectiles.len(),
        )
    };
    crate::connection::handle_client_packet(
        &state,
        player_id,
        exact,
        "replayed-frequency-peer",
        &mut joined,
    );
    let replayed = state.lock().expect("state").drain_position_inputs();
    for (runtime, packet, _, _) in replayed {
        let _ = crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            runtime.life,
            &packet,
            crate::wire::current_tick(),
        );
    }
    let mut later = expected;
    later.client_sequence = Some(0);
    crate::connection::handle_client_packet(
        &state,
        player_id,
        frequency_baseline_packet(later, 0),
        "wrapped-frequency-peer",
        &mut joined,
    );
    let wrapped = state.lock().expect("state").drain_position_inputs();
    for (runtime, packet, _, _) in wrapped {
        let _ = crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            runtime.life,
            &packet,
            crate::wire::current_tick(),
        );
    }
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert_eq!(client.last_position_sequence, Some(0));
    assert_eq!(
        (
            client.server_energy,
            client.server_weapons,
            locked.active_projectiles.len()
        ),
        after_exact
    );
    assert!(locked.player_runtime_matches(identity));
}

#[test]
fn frequency_cutover_cancels_pending_shutdown_durably_and_new_life_is_not_blocked() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.set(u16::MAX));
    let request = frequency_change_request(&state, player_id, 17);
    let (old_life, old_revision, old_application) = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.apply_server_prize_effect_at_tick(-14, 50_000);
        let old_life = client.life_seq;
        let old_revision = client.server_prize_state_revision;
        let pending = crate::state::PendingPrizeEffectApplication {
            collectible_id: 0x4455_6677,
            prize_id: -14,
            life_seq: old_life,
            prize_state_revision: old_revision,
            minimum_application_sequence: Some(0),
            maximum_application_sequence: Some(0),
            latest_application_sequence: None,
            deferred_application: None,
            movement_tuning_before_bits: None,
        };
        client.pending_prize_effect_applications.push_back(pending);
        (
            old_life,
            old_revision,
            crate::prize_application::PrizeEffectApplied {
                server_session_id: crate::wire::server_session_id(),
                player_id,
                life_seq: old_life,
                collectible_id: pending.collectible_id,
                prize_id: pending.prize_id,
                prize_state_revision: old_revision,
                application_sequence: 0,
            },
        )
    };

    update_frequency(&state, player_id, &request);
    let cancelled_payload = read_reliable_payload_from_channel(&rx);
    let cancelled = crate::prize_application::PrizeEffectAccepted::parse(&cancelled_payload)
        .expect("cutover cancellation terminal");
    assert_eq!(
        cancelled.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(cancelled.life_seq, old_life);
    assert_eq!(cancelled.prize_state_revision, old_revision);
    let cutover = read_reliable_payload_from_channel(&rx);
    assert_eq!(cutover[0], crate::roster::FREQUENCY_CUTOVER_ACK);
    let _roster = read_reliable_payload_from_channel(&rx);

    let (new_life, new_revision) = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        assert_ne!(client.life_seq, old_life);
        assert!(client.pending_prize_effect_applications.is_empty());
        assert_eq!(client.server_engine_shutdown_until_tick, None);
        client.apply_server_prize_effect_at_tick(-14, 50_001);
        let pending = crate::state::PendingPrizeEffectApplication {
            collectible_id: 0x8899_aabb,
            prize_id: -14,
            life_seq: client.life_seq,
            prize_state_revision: client.server_prize_state_revision,
            minimum_application_sequence: Some(1),
            maximum_application_sequence: Some(1),
            latest_application_sequence: None,
            deferred_application: None,
            movement_tuning_before_bits: None,
        };
        client.pending_prize_effect_applications.push_back(pending);
        (client.life_seq, client.server_prize_state_revision)
    };
    let new_application = crate::prize_application::PrizeEffectApplied {
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq: new_life,
        collectible_id: 0x8899_aabb,
        prize_id: -14,
        prize_state_revision: new_revision,
        application_sequence: 1,
    };
    let applied =
        crate::player_events::apply_prize_effect_application(&state, player_id, new_application)
            .expect("new-life head applies without the old-life claim");
    assert_eq!(
        applied.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED
    );
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, old_application,),
        Some(cancelled),
        "late and duplicate old-life applications retain the exact terminal"
    );

    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    let resent_cancelled = rx.try_iter().filter_map(|packet| {
        crate::reliable::reliable_payload(&packet)
            .and_then(|payload| crate::prize_application::PrizeEffectAccepted::parse(payload).ok())
            .filter(|receipt| *receipt == cancelled)
    });
    assert_eq!(
        resent_cancelled.count(),
        1,
        "lost cancellation terminal is retained and retried exactly"
    );
}

#[test]
fn active_rocket_frequency_cutover_starts_an_inactive_wrapped_life_without_refund() {
    use crate::snapshot::{SnapshotEvent, SnapshotFrame};

    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let request_tick = crate::continuum::CONTINUUM_TICK_MASK - 2;
    {
        let mut locked = state.lock().expect("state");
        locked.life_seq_watermarks.insert(player_id, u32::MAX);
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_seq = u32::MAX;
        client.server_weapons.rockets = 2;
        client.server_rocket_until_tick = Some(crate::continuum::continuum_tick_add(
            request_tick,
            crate::arena_settings::arena_ship_rocket_lifetime_ticks(client.player.ship),
        ));
        client.server_rocket_input_asserted = true;
        client
            .server_rocket_authorizations
            .push_back(crate::state::ServerRocketAuthorization {
                start_sequence: u16::MAX - 1,
                start_tick: request_tick,
                deadline_tick: client.server_rocket_until_tick.expect("active deadline"),
                lifetime_ticks: crate::arena_settings::arena_ship_rocket_lifetime_ticks(
                    client.player.ship,
                ),
            });
        client
            .recent_input_rocket_assertions
            .push_back((u16::MAX - 1, true));
    }
    NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.set(u16::MAX));
    let request = frequency_change_request(&state, player_id, 17);
    let base = u16::from_le_bytes([request[3], request[4]]);

    update_frequency_at_tick(&state, player_id, &request, request_tick);
    let cutover = read_reliable_payload_from_channel(&rx);
    assert_eq!(cutover[0], crate::roster::FREQUENCY_CUTOVER_ACK);
    assert_eq!(u16::from_le_bytes([cutover[3], cutover[4]]), u16::MAX);
    assert_eq!(u32::from_le_bytes(cutover[5..9].try_into().unwrap()), 1);
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        current_ship_frequency_change_payload(&state, player_id)
    );
    let expected = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.life_seq, 1);
        assert_eq!(
            client.server_weapons.rockets, 2,
            "spent Rocket is not refunded"
        );
        assert_eq!(client.server_rocket_until_tick, None);
        assert!(!client.server_rocket_input_asserted);
        assert!(client.server_rocket_authorizations.is_empty());
        client.pending_frequency_baseline.expect("pending baseline")
    };

    // The exact reliable new-life baseline and a wrapped resumed input both
    // carry no Rocket pulse. Neither may manufacture a fresh activation.
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        frequency_baseline_packet(expected, base),
        "active-rocket-cutover-peer",
        &mut joined,
    );
    let mut resumed = vec![
        crate::connection::CLIENT_INPUT_RECORD_TYPE,
        0,
        0,
        1,
        0,
        0,
        expected.direction,
        0,
    ];
    resumed.extend_from_slice(&unavailable_combat_view_receipt(0));
    crate::connection::handle_client_packet(
        &state,
        player_id,
        resumed.to_vec(),
        "active-rocket-cutover-peer",
        &mut joined,
    );
    {
        let mut locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.server_weapons.rockets, 2);
        assert_eq!(client.server_rocket_until_tick, None);
        assert!(!client.server_rocket_input_asserted);
        assert!(client.server_rocket_authorizations.is_empty());
        locked
            .clients
            .get_mut(&player_id)
            .expect("client")
            .snapshot_ready = true;
        locked.arena_tick = 2;
    }

    // The very first private RocketState produced for the acknowledged life
    // is structurally valid and explicitly inactive, including across the
    // u31 deadline wrap used by the old life.
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, request_tick);
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("new-life snapshot")).unwrap();
    let rocket = frame
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(rocket) => Some(*rocket),
            _ => None,
        })
        .expect("private RocketState");
    assert_eq!(rocket.life_seq, 1);
    assert!(!rocket.active);
    assert_eq!(rocket.activation_sequence, None);
    assert_eq!(rocket.until_tick, None);
    assert_eq!(rocket.rockets, 2);
}

#[test]
fn server_ignores_no_op_ship_and_frequency_changes() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    update_ship(&state, player_id, &ship_change_request(0));
    assert!(rx.try_recv().is_err());

    update_frequency(
        &state,
        player_id,
        &frequency_change_request(&state, player_id, player_id),
    );
    assert!(rx.try_recv().is_err());

    update_frequency(
        &state,
        player_id,
        &frequency_change_request(&state, player_id, 33),
    );
    let frequency_change = read_frequency_roster(&rx);
    assert_eq!(
        frequency_change,
        current_ship_frequency_change_payload(&state, player_id)
    );
}

#[test]
fn server_rejects_frequency_changes_outside_arena_range() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let invalid = arena_max_frequency();
    update_frequency(
        &state,
        player_id,
        &frequency_change_request(&state, player_id, invalid),
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&player_id)
            .map(|client| client.player.frequency),
        Some(player_id)
    );

    let valid = invalid - 1;
    update_frequency(
        &state,
        player_id,
        &frequency_change_request(&state, player_id, valid),
    );
    let frequency_change = read_frequency_roster(&rx);
    assert_eq!(
        frequency_change,
        current_ship_frequency_change_payload(&state, player_id)
    );
}

#[test]
fn server_rate_limits_ship_and_frequency_change_bursts() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    for ship in [1, 2, 3] {
        update_ship(&state, player_id, &ship_change_request(ship));
        assert_eq!(
            read_reliable_payload_from_channel(&rx),
            current_ship_frequency_change_payload(&state, player_id)
        );
    }
    update_ship(&state, player_id, &ship_change_request(4));
    assert!(rx.try_recv().is_err());

    let frequency_window = Instant::now();
    for (index, frequency) in [10_u16, 11, 12].into_iter().enumerate() {
        let request = frequency_change_request(&state, player_id, frequency);
        update_frequency_instant_at_tick(
            &state,
            player_id,
            &request,
            crate::wire::current_tick(),
            frequency_window + Duration::from_millis(index as u64 * 10),
        );
        let observed = {
            let locked = state.lock().expect("state");
            let client = &locked.clients[&player_id];
            (
                client.player.frequency,
                client.last_input_seq,
                client.last_position_sequence,
                client.life_position_sequence_base,
                client.pending_frequency_baseline,
                client.frequency_change_burst_count,
            )
        };
        assert_eq!(observed.0, frequency, "post-request state: {observed:?}");
        assert_eq!(
            read_frequency_roster(&rx),
            current_ship_frequency_change_payload(&state, player_id)
        );
    }
    let rejected_request = frequency_change_request(&state, player_id, 13);
    let authority_before_reject = frequency_authority_state(&state, player_id);
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &rejected_request,
        crate::wire::current_tick(),
        frequency_window + Duration::from_millis(30),
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(
        frequency_authority_state(&state, player_id),
        authority_before_reject,
        "the fourth valid request must fail before any authority mutation"
    );

    update_frequency_instant_at_tick(
        &state,
        player_id,
        &rejected_request,
        crate::wire::current_tick(),
        frequency_window + Duration::from_millis(271),
    );
    assert_eq!(
        read_frequency_roster(&rx),
        current_ship_frequency_change_payload(&state, player_id),
        "the exact same request/base must succeed after the limiter window"
    );
}

#[test]
fn rejected_frequency_baselines_restore_complete_authority_and_allow_corrected_same_sequence() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let now = Instant::now();

    let exact_direction_request = frequency_change_request(&state, player_id, 17);
    let mut invalid_direction_request = exact_direction_request.clone();
    invalid_direction_request[6] = 40;
    invalid_direction_request
        [5 + crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 6] = 40;
    let direction_before = frequency_authority_state(&state, player_id);
    let direction_rejects_before = crate::security::peek_security_reject_count_for_test(
        "frequency_change",
        "invalid_baseline",
    );
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &invalid_direction_request,
        crate::wire::current_tick(),
        now,
    );
    assert!(
        rx.try_recv().is_err(),
        "rejection must emit no ACK/roster event"
    );
    assert_eq!(
        frequency_authority_state(&state, player_id),
        direction_before
    );
    assert!(
        crate::security::peek_security_reject_count_for_test(
            "frequency_change",
            "invalid_baseline"
        ) >= direction_rejects_before + 1
    );
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &exact_direction_request,
        crate::wire::current_tick(),
        now + Duration::from_millis(1),
    );
    assert_eq!(
        read_frequency_roster(&rx),
        current_ship_frequency_change_payload(&state, player_id)
    );

    let exact_impossible_direction_request = frequency_change_request(&state, player_id, 18);
    let mut impossible_direction_request = exact_impossible_direction_request.clone();
    impossible_direction_request[6] = 20;
    impossible_direction_request
        [5 + crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 6] = 20;
    let baseline = &mut impossible_direction_request[5..];
    set_client_position_canonical_state(
        baseline,
        u16::from_le_bytes(baseline[12..14].try_into().unwrap()) as f32,
        u16::from_le_bytes(baseline[8..10].try_into().unwrap()) as f32,
        i16::from_le_bytes(baseline[6..8].try_into().unwrap()) as f32 / 10.0,
        i16::from_le_bytes(baseline[14..16].try_into().unwrap()) as f32 / 10.0,
        crate::continuum_position::protocol_direction_to_orientation(20),
    );
    let impossible_direction_before = frequency_authority_state(&state, player_id);
    let impossible_direction_rejects_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "invalid_direction_history",
    );
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &impossible_direction_request,
        crate::wire::current_tick(),
        now + Duration::from_millis(2),
    );
    assert!(
        rx.try_recv().is_err(),
        "late direction rejection must emit no ACK/roster event"
    );
    assert_eq!(
        frequency_authority_state(&state, player_id),
        impossible_direction_before,
        "a well-formed but physically impossible direction must roll back staged input"
    );
    assert!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "invalid_direction_history"
        ) >= impossible_direction_rejects_before + 1
    );
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &exact_impossible_direction_request,
        crate::wire::current_tick(),
        now + Duration::from_millis(3),
    );
    assert_eq!(
        read_frequency_roster(&rx),
        current_ship_frequency_change_payload(&state, player_id),
        "the corrected direction at the exact same sequence must succeed"
    );

    let exact_collision_request = frequency_change_request(&state, player_id, 19);
    let blocked_index = zone_map()
        .tiles
        .iter()
        .position(|&tile| position_blocking_tile(tile))
        .expect("Chaos Zone should include a solid tile");
    let blocked_x = ((blocked_index % MAP_TILES) as u16)
        .saturating_mul(TILE_SIZE_PX as u16)
        .saturating_add((TILE_SIZE_PX / 2.0) as u16);
    let blocked_y = ((blocked_index / MAP_TILES) as u16)
        .saturating_mul(TILE_SIZE_PX as u16)
        .saturating_add((TILE_SIZE_PX / 2.0) as u16);
    let mut blocked_request = exact_collision_request.clone();
    blocked_request[5 + 12..5 + 14].copy_from_slice(&blocked_x.to_le_bytes());
    blocked_request[5 + 8..5 + 10].copy_from_slice(&blocked_y.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut blocked_request[5..]);
    let collision_before = frequency_authority_state(&state, player_id);
    let collision_rejects_before =
        crate::security::peek_security_reject_count_for_test("position", "blocked_position_center");
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &blocked_request,
        crate::wire::current_tick(),
        now + Duration::from_millis(4),
    );
    assert!(
        rx.try_recv().is_err(),
        "rejection must emit no ACK/roster event"
    );
    assert_eq!(
        frequency_authority_state(&state, player_id),
        collision_before
    );
    assert!(
        crate::security::peek_security_reject_count_for_test("position", "blocked_position_center")
            >= collision_rejects_before + 1
    );
    update_frequency_instant_at_tick(
        &state,
        player_id,
        &exact_collision_request,
        crate::wire::current_tick(),
        now + Duration::from_millis(5),
    );
    assert_eq!(
        read_frequency_roster(&rx),
        current_ship_frequency_change_payload(&state, player_id),
        "the corrected exact same-base request must remain admissible"
    );
}

#[test]
fn frequency_cutover_commits_when_public_position_fanout_is_token_suppressed() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let request = frequency_change_request(&state, player_id, 27);
    {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("player");
        client.last_position_relay_at = Some(Instant::now());
        client.position_relay_tokens = 0.0;
    }

    update_frequency_at_tick(&state, player_id, &request, crate::wire::current_tick());

    assert_eq!(
        read_frequency_roster(&rx),
        current_ship_frequency_change_payload(&state, player_id)
    );
    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    assert_eq!(client.player.frequency, 27);
    assert_eq!(client.frequency_change_burst_count, 1);
    assert!(client.pending_frequency_baseline.is_some());
}

#[test]
fn frequency_cutover_preserves_spent_portals_and_clears_old_life_replay() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let request = frequency_change_request(&state, player_id, 27);
    let old_life = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("player");
        client.server_weapons.portals = 1;
        client.latest_portal_action_id = Some(u32::MAX);
        client.portal_action_mask = 1;
        client.life_seq
    };

    update_frequency_at_tick(&state, player_id, &request, crate::wire::current_tick());
    assert_eq!(
        read_frequency_roster(&rx),
        current_ship_frequency_change_payload(&state, player_id)
    );

    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    assert_ne!(client.life_seq, old_life);
    assert_eq!(client.server_weapons.portals, 1);
    assert_eq!(client.latest_portal_action_id, None);
    assert_eq!(client.portal_action_mask, 0);
    assert!(client.portal_placement_terminals.is_empty());
}

#[test]
fn frequency_cutover_missing_history_rebases_only_to_server_owned_anchor() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let seed_request = frequency_change_request(&state, player_id, 1);
    update_frequency_at_tick(
        &state,
        player_id,
        &seed_request,
        crate::wire::current_tick(),
    );
    drain_available(&rx);
    consume_pending_lifecycle_baseline(&state, player_id);

    let mut request = frequency_change_request(&state, player_id, 27);
    let original_sequence = u16::from_le_bytes([request[3], request[4]]);
    let far_sequence =
        original_sequence.wrapping_add(crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16 + 2);
    request[3..5].copy_from_slice(&far_sequence.to_le_bytes());
    request[5 + 28..5 + 32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(far_sequence).to_le_bytes(),
    );
    let input_offset = 5 + crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET;
    request[input_offset + 1..input_offset + 3].copy_from_slice(&far_sequence.to_le_bytes());
    request[input_offset + 8
        ..input_offset + crate::continuum_position::CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN]
        .copy_from_slice(&unavailable_combat_view_receipt(far_sequence));

    let expected = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        crate::arena::frequency_baseline_from_large_position(
            client.last_position.as_deref().expect("accepted anchor"),
            far_sequence,
            {
                let witness = client
                    .canonical_movement_witness
                    .expect("exact accepted movement witness");
                crate::continuum_position::ClientPositionCanonicalState::from_f32(
                    witness.state.position_x,
                    witness.state.position_y,
                    witness.state.velocity_x,
                    witness.state.velocity_y,
                    witness.state.orientation,
                )
            },
        )
        .expect("canonical frequency baseline")
    };
    // The unprovable client coordinates must never become the fallback
    // authority. Only the server's accepted anchor above may cross the
    // lifecycle boundary.
    request[5 + 8..5 + 10].copy_from_slice(&1_u16.to_le_bytes());
    request[5 + 12..5 + 14].copy_from_slice(&1_u16.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut request[5..]);

    update_frequency_at_tick(&state, player_id, &request, crate::wire::current_tick());

    let cutover = read_reliable_payload_from_channel(&rx);
    assert_eq!(cutover[0], crate::roster::FREQUENCY_CUTOVER_ACK);
    assert_eq!(
        cutover.len(),
        23 + crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_LEN,
        "current frequency ACK must carry the exact server fallback anchor"
    );
    assert_eq!(u16::from_le_bytes([cutover[3], cutover[4]]), far_sequence);
    let cutover_state = crate::continuum_position::client_position_canonical_state_at(&cutover, 23)
        .expect("frequency ACK exact anchor");
    assert_eq!(cutover_state, expected.canonical_state);
    assert_ne!(
        cutover_state,
        crate::continuum_position::client_position_canonical_state(&request[5..])
            .expect("request exact state"),
        "the request's exact-but-unprovable state cannot replace server authority"
    );
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        current_ship_frequency_change_payload(&state, player_id)
    );
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert_eq!(client.player.frequency, 27);
    assert_eq!(client.pending_frequency_baseline, Some(expected));
    assert_eq!(client.life_position_sequence_base, Some(far_sequence));
}

#[test]
fn queued_frequency_cutover_is_runtime_scoped_and_runs_on_server_tick() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let request = frequency_change_request(&state, player_id, 29);
    let identity = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    assert!(
        state
            .lock()
            .expect("state")
            .queue_frequency_change_for_runtime(identity, 1, request)
    );
    assert_eq!(
        state.lock().expect("state").clients[&player_id]
            .player
            .frequency,
        0
    );

    crate::server_tick::run_server_tick(&state, Instant::now());
    assert_eq!(
        state.lock().expect("state").clients[&player_id]
            .player
            .frequency,
        29
    );
    assert_eq!(
        read_reliable_payload_from_channel(&rx)[0],
        crate::roster::FREQUENCY_CUTOVER_ACK
    );

    consume_pending_lifecycle_baseline(&state, player_id);
    let stale_request = frequency_change_request(&state, player_id, 31);
    let stale_identity = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    assert!(
        state
            .lock()
            .expect("state")
            .queue_frequency_change_for_runtime(stale_identity, 2, stale_request)
    );
    state
        .lock()
        .expect("state")
        .advance_client_life_at_tick(player_id, crate::wire::current_tick());
    crate::server_tick::run_server_tick(&state, Instant::now());
    assert_eq!(
        state.lock().expect("state").clients[&player_id]
            .player
            .frequency,
        29,
        "a queued request from the previous runtime must not reach its replacement"
    );
}

#[test]
fn frequency_baseline_zero_public_tokens_still_emits_and_retries_wrapped_app_receipt() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.set(u16::MAX));
    let request = frequency_change_request(&state, player_id, 31);
    let sequence = u16::from_le_bytes([request[3], request[4]]);
    assert_eq!(sequence, u16::MAX);
    update_frequency_at_tick(&state, player_id, &request, crate::wire::current_tick());
    drain_available(&rx);

    let (life_seq, expected) = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("player");
        client.last_position_relay_at = Some(Instant::now());
        client.position_relay_tokens = -100.0;
        (
            client.life_seq,
            client.pending_frequency_baseline.expect("pending baseline"),
        )
    };
    let delivery_id = 0;
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(delivery_id, &frequency_baseline_packet(expected, sequence)),
        "zero-token-frequency-baseline-peer",
        &mut joined,
    );
    let transport_only = rx.try_iter().collect::<Vec<_>>();
    assert!(transport_only.iter().any(|packet| {
        crate::reliable::reliable_ack_ids(packet).as_deref() == Some(&[delivery_id])
    }));
    assert!(!transport_only.iter().any(|packet| {
        crate::reliable::reliable_payload(packet).is_some_and(|payload| {
            payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
        })
    }));

    let receipt_tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), receipt_tick);
    let receipt = rx
        .try_iter()
        .find(|packet| {
            crate::reliable::reliable_payload(packet).is_some_and(|payload| {
                payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
            })
        })
        .expect("accepted frequency baseline must emit its private receipt");
    let (_, payload) = crate::reliable::reliable_parts(&receipt).expect("reliable receipt");
    assert_eq!(
        payload,
        crate::roster::lifecycle_baseline_accepted(
            delivery_id,
            life_seq,
            sequence,
            receipt_tick,
            (expected.x, expected.y),
        )
    );
    {
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.clients[&player_id].last_position_sequence,
            Some(sequence)
        );
        assert!(
            locked.clients[&player_id]
                .pending_frequency_baseline
                .is_none()
        );
        assert!(locked.clients[&player_id].position_relay_tokens < 1.0);
    }

    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert!(
        rx.try_iter().any(|packet| packet == receipt),
        "a lost frequency application receipt must remain retained for resend"
    );
}

#[test]
fn server_updates_and_relays_player_banners() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let mut banner = [0; PLAYER_BANNER_BYTES];
    for (index, pixel) in banner.iter_mut().enumerate() {
        *pixel = (index % 16) as u8;
    }
    let mut request = vec![SET_BANNER_REQUEST];
    request.extend_from_slice(&banner);

    update_banner(&state, alpha, &request);

    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        personal_banner_change(&banner)
    );
    assert_eq!(
        read_reliable_payload_from_channel(&rx_b),
        player_banner_change(alpha, &banner)
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .map(|client| client.player.banner),
        Some(banner)
    );
    update_banner(&state, alpha, &[SET_BANNER_REQUEST, 1, 2, 3]);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn server_replays_existing_player_banners_to_late_joiners() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);

    let mut banner = [0; PLAYER_BANNER_BYTES];
    banner[0] = 11;
    banner[17] = 12;
    banner[95] = 13;
    let mut request = vec![SET_BANNER_REQUEST];
    request.extend_from_slice(&banner);
    update_banner(&state, alpha, &request);
    drain_available(&rx_a);

    let beta = register_connection(&state, tx_b);
    join_arena(&state, beta, 0);

    assert_server_clock_bootstrap(&read_reliable_payload_from_channel(&rx_b), beta);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), [0x01, 1, 0]);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), [0x02]);
    assert_eq!(read_reliable_payload_from_channel(&rx_b)[51..53], [1, 0]);
    assert_eq!(read_reliable_payload_from_channel(&rx_b)[51..53], [0, 0]);
    assert_eq!(
        read_reliable_payload_from_channel(&rx_b),
        player_banner_change(alpha, &banner)
    );
}

#[test]
fn server_preserves_spectator_ship_on_arena_join() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);

    join_arena(&state, player_id, 8);

    assert_server_clock_bootstrap(&read_reliable_payload_from_channel(&rx), player_id);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x01, 0, 0]);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x02]);
    let entering = read_reliable_payload_from_channel(&rx);
    assert_eq!(entering[0], 0x03);
    assert_eq!(entering[1], 8);
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("player should exist");
    assert!(!client.alive);
    assert!(client.last_position.is_none());
}

#[test]
fn damaged_ship_change_is_rejected_without_reset_and_full_energy_can_retry() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    join_arena(&state, id, 0);
    drain_available(&rx);
    let before = {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&id).unwrap();
        // Use an upgraded capacity; the gate must not compare against the
        // ship's initial energy or trust a client-reported full-energy value.
        client.server_max_energy = 2000;
        client.server_energy = 1999;
        client.last_reported_energy = 2000;
        (
            client.player.ship,
            client.life_seq,
            client.server_bounty,
            client.server_weapons,
            client.last_position.clone(),
        )
    };
    for target in [1, 8] {
        let request = ship_change_request(target);
        update_ship(&state, id, &request);
        let rejection = read_reliable_payload_from_channel(&rx);
        let mut expected = vec![0x7f, 1, target];
        expected.extend(crate::wire::server_session_id().to_le_bytes());
        expected.extend(
            state.lock().unwrap().clients[&id]
                .arena_stream_id
                .to_le_bytes(),
        );
        expected.extend(id.to_le_bytes());
        expected.extend(before.1.to_le_bytes());
        expected.extend(&request[2..4]);
        assert_eq!(rejection, expected);
        let state = state.lock().unwrap();
        let client = &state.clients[&id];
        assert_eq!(
            (
                client.player.ship,
                client.life_seq,
                client.server_bounty,
                client.server_weapons,
                client.last_position.clone()
            ),
            before
        );
        assert_eq!(client.server_energy, 1999);
        assert_eq!(client.ship_change_burst_count, 0);
    }
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&id)
        .unwrap()
        .server_energy = 2000;
    update_ship(&state, id, &ship_change_request(1));
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        current_ship_frequency_change_payload(&state, id)
    );
    let state = state.lock().unwrap();
    assert_eq!(state.clients[&id].player.ship, 1);
    assert_eq!(state.clients[&id].life_seq, before.1 + 1);
}

#[test]
fn spectator_can_choose_a_ship_without_energy_but_dead_pilot_cannot_skip_respawn() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    join_arena(&state, id, 0);
    drain_available(&rx);
    {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&id).unwrap();
        client.alive = false;
        client.server_energy = 0;
    }
    update_ship(&state, id, &ship_change_request(1));
    assert_eq!(read_reliable_payload_from_channel(&rx)[0], 0x7f);
    assert!(!state.lock().unwrap().clients[&id].alive);
    {
        let mut state = state.lock().unwrap();
        state.clients.get_mut(&id).unwrap().player.ship = 8;
    }
    update_ship(&state, id, &ship_change_request(1));
    assert_eq!(
        read_reliable_payload_from_channel(&rx)[0],
        SHIP_FREQUENCY_CHANGE
    );
    assert!(state.lock().unwrap().clients[&id].alive);
}

#[test]
fn quality_force_spectator_retires_live_or_dead_authority_without_energy_or_rate_gate() {
    for alive in [true, false] {
        let state = Arc::new(Mutex::new(team_mode_state()));
        let (tx, rx) = mpsc::channel();
        let id = register_connection(&state, tx);
        join_arena(&state, id, 0);
        drain_available(&rx);
        let now = Instant::now();
        let (identity, life, frequency, stream) = {
            let mut guard = state.lock().unwrap();
            let identity = guard.active_player_runtime_identity(id).unwrap();
            let client = guard.clients.get_mut(&id).unwrap();
            client.alive = alive;
            client.server_energy = 1;
            client.respawn_at_tick = (!alive).then_some(500);
            client.last_ship_change_at = Some(now);
            client.ship_change_burst_count = u8::MAX;
            client.last_input_seq = Some(30);
            client.server_engine_shutdown_until_tick = Some(400);
            client.server_rocket_until_tick = Some(400);
            client.last_position = Some(vec![5; 30]);
            (
                identity,
                client.life_seq,
                client.player.frequency,
                client.arena_stream_id,
            )
        };
        let packet = {
            let mut guard = state.lock().unwrap();
            match crate::arena::force_quality_spectator_in_state(&mut guard, identity, 100, now) {
                crate::arena::RosterChangeOutcome::Applied(packet) => packet,
                _ => panic!("quality spectator transition should commit"),
            }
        };
        assert_eq!(packet, current_ship_frequency_change_payload(&state, id));
        let mut guard = state.lock().unwrap();
        let client = &guard.clients[&id];
        assert_eq!(client.player.ship, 8);
        assert!(!client.alive);
        assert_eq!(client.life_seq, life + 1);
        assert_eq!(client.arena_stream_id, stream);
        assert_eq!(client.player.frequency, frequency);
        assert_eq!(client.server_energy, 0);
        assert!(client.respawn_at_tick.is_none());
        assert!(client.last_position.is_none());
        assert!(client.last_input_seq.is_none());
        assert!(client.life_position_sequence_base.is_none());
        assert!(client.server_engine_shutdown_until_tick.is_none());
        assert!(client.server_rocket_until_tick.is_none());
        assert_eq!(
            client.ship_change_burst_count,
            u8::MAX,
            "forced path cannot spend user request credits"
        );
        // Neither an old-life replay nor a second decision may advance life twice.
        assert!(matches!(
            crate::arena::force_quality_spectator_in_state(&mut guard, identity, 101, now),
            crate::arena::RosterChangeOutcome::TerminalNoop
        ));
        let current = guard.active_player_runtime_identity(id).unwrap();
        assert!(matches!(
            crate::arena::force_quality_spectator_in_state(&mut guard, current, 101, now),
            crate::arena::RosterChangeOutcome::TerminalNoop
        ));
        assert_eq!(guard.clients[&id].life_seq, life + 1);
    }
}

#[test]
fn quality_hold_blocks_join_and_ship_returns_without_affecting_other_players() {
    use crate::connection_quality_protocol::{
        ConnectionQualityDirective, ConnectionQualityDirectiveKind,
    };
    for elapsed in [0, 121] {
        let state = Arc::new(Mutex::new(team_mode_state()));
        let (tx, rx) = mpsc::channel();
        let held = register_connection(&state, tx);
        assert!(set_player_identity_and_name(
            &state,
            held,
            "held-pilot".into(),
            "held".into()
        ));
        let hold = state
            .lock()
            .unwrap()
            .quality_holds
            .begin("held-pilot", Instant::now() - Duration::from_secs(elapsed))
            .unwrap();
        join_arena(&state, held, 0);
        assert_server_clock_bootstrap(&read_reliable_payload_from_channel(&rx), held);
        assert_eq!(read_reliable_payload_from_channel(&rx)[0], 0x01);
        assert_eq!(read_reliable_payload_from_channel(&rx), [0x02]);
        assert_eq!(read_reliable_payload_from_channel(&rx)[1], 8);
        let status =
            ConnectionQualityDirective::parse(&read_reliable_payload_from_channel(&rx)).unwrap();
        assert_eq!(status.kind, ConnectionQualityDirectiveKind::Hold);
        assert_eq!(status.hold_id, hold.id);
        assert_eq!(status.remaining_ms == 0, elapsed >= 120);
        drain_available(&rx);
        let (tx_other, rx_other) = mpsc::channel();
        let other = register_connection(&state, tx_other);
        assert!(set_player_identity_and_name(
            &state,
            other,
            "healthy-pilot".into(),
            "other".into()
        ));
        join_arena(&state, other, 1);
        drain_available(&rx_other);
        drain_available(&rx);
        let life = state.lock().unwrap().clients[&held].life_seq;
        let request = ship_change_request(0);
        update_ship(&state, held, &request);
        let denied =
            ConnectionQualityDirective::parse(&read_reliable_payload_from_channel(&rx)).unwrap();
        assert_eq!(denied.kind, ConnectionQualityDirectiveKind::Hold);
        assert_eq!(denied.hold_id, hold.id);
        assert_eq!(denied.life_seq, life);
        assert_eq!(
            denied.rejected_request,
            Some((0, u16::from_le_bytes([request[2], request[3]])))
        );
        assert_eq!(state.lock().unwrap().clients[&held].player.ship, 8);
        assert_eq!(state.lock().unwrap().clients[&held].life_seq, life);
        assert_eq!(state.lock().unwrap().clients[&other].player.ship, 1);
        assert!(rx_other.try_recv().is_err());
        // Ordinary leave/rejoin and a new transport with the same stable id
        // remain observers, even when the original deadline has expired.
        crate::arena::leave_arena(&state, held);
        join_arena(&state, held, 7);
        assert_eq!(state.lock().unwrap().clients[&held].player.ship, 8);
        crate::arena::disconnect_player(&state, held);
        let (tx_new, _rx_new) = mpsc::channel();
        let reconnected = register_connection(&state, tx_new);
        assert!(set_player_identity_and_name(
            &state,
            reconnected,
            "held-pilot".into(),
            "held".into()
        ));
        join_arena(&state, reconnected, 3);
        let guard = state.lock().unwrap();
        assert_eq!(guard.clients[&reconnected].player.ship, 8);
        assert!(!guard.clients[&reconnected].alive);
        assert_eq!(guard.quality_holds.get("held-pilot"), Some(hold));
    }
}
