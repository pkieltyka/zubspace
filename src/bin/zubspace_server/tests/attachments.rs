use super::*;

#[test]
fn server_relays_attach_drop_and_spectate_requests() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    update_frequency(&state, beta, &frequency_change_request(&state, beta, 0));
    consume_pending_lifecycle_baseline(&state, beta);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let carried_flag = {
        let mut state = state.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let carrier = state.active_player_runtime_identity(alpha).unwrap();
        state
            .objectives
            .create_flag(FlagState::Carried {
                carrier,
                x: 512,
                y: 512,
                owner: None,
            })
            .unwrap()
    };

    update_spectate(&state, alpha, &spectate_request_payload(beta));
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        spectate_packet(true)
    );
    assert!(matches!(
        state
            .lock()
            .unwrap()
            .objectives
            .flag(carried_flag.identity.id)
            .unwrap()
            .state,
        FlagState::Carried { carrier, .. } if carrier.player_id() == alpha
    ));
    update_spectate(&state, alpha, &spectate_request_payload(u16::MAX));
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        spectate_packet(false)
    );
    assert!(rx_b.try_recv().is_err());

    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    let expected_attach = create_turret_link(alpha, beta);
    assert_eq!(read_reliable_payload_from_channel(&rx_a), expected_attach);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), expected_attach);
    assert_eq!(
        current_turret_packets(&state),
        vec![expected_attach.clone()]
    );
    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    relay_drop_attached(&state, beta);
    let expected_drop = destroy_turret_link(beta);
    assert_eq!(read_reliable_payload_from_channel(&rx_a), expected_drop);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), expected_drop);
    assert!(current_turret_packets(&state).is_empty());

    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    drain_available(&rx_a);
    drain_available(&rx_b);
    relay_attach_request(&state, alpha, &attach_request_payload(u16::MAX));
    let expected_detach = create_turret_link(alpha, u16::MAX);
    assert_eq!(read_reliable_payload_from_channel(&rx_a), expected_detach);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), expected_detach);
    relay_attach_request(&state, alpha, &attach_request_payload(u16::MAX));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    relay_attach_request(&state, alpha, &[ATTACH_REQUEST, 1]);
    relay_drop_attached(&state, beta);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn server_rejects_dead_player_turret_attach_requests() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    update_frequency(&state, beta, &frequency_change_request(&state, beta, 0));
    consume_pending_lifecycle_baseline(&state, beta);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .alive = false;
    }
    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .alive = true;
        state
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .alive = false;
    }
    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert!(current_turret_packets(&state).is_empty());
}

#[test]
fn server_clears_turret_links_when_player_dies() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let killed = register_connection(&state, tx_a);
    let killer = register_connection(&state, tx_b);
    let child = register_connection(&state, tx_c);
    join_arena(&state, killed, 0);
    join_arena(&state, killer, 0);
    join_arena(&state, child, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    reset_server_runtime_without_initial_bounty(&state, killed);
    reset_server_runtime_without_initial_bounty(&state, killer);
    reset_server_runtime_without_initial_bounty(&state, child);
    broadcast_position(
        &state,
        killed,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_b);
    drain_available(&rx_c);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.turret_links.insert(killed, killer);
        state.turret_links.insert(child, killed);
    }

    relay_death(
        &state,
        killed,
        &[DEATH_REQUEST, killed as u8, (killed >> 8) as u8, 44, 0],
    );

    super::assert_death_packet_eq_ignoring_green(
        &read_reliable_payload_from_channel(&rx_a),
        &[PLAYER_DEATH, 0xff, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        create_turret_link(killed, u16::MAX)
    );
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        destroy_turret_link(killed)
    );
    assert_score_update(&read_reliable_payload_from_channel(&rx_a), killed, 0, 1);
    assert!(rx_a.try_recv().is_err());
    drain_available(&rx_b);
    drain_available(&rx_c);
    assert!(current_turret_packets(&state).is_empty());
}

#[test]
fn server_clears_turret_links_when_authoritative_damage_kills_player() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let damaged = register_connection(&state, tx_a);
    let shooter = register_connection(&state, tx_b);
    join_arena(&state, damaged, 0);
    join_arena(&state, shooter, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, damaged);
    reset_server_runtime_without_initial_bounty(&state, shooter);
    broadcast_position(
        &state,
        damaged,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_b);
    broadcast_position(
        &state,
        shooter,
        &client_position_packet(0x1122_3345, 520, 700),
    );
    drain_available(&rx_a);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.turret_links.insert(damaged, shooter);
        state
            .clients
            .get_mut(&damaged)
            .expect("damaged player exists")
            .server_energy = 1;
    }

    crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 1,
            owner_id: shooter,
            owner_life_seq: 1,
            victim_id: damaged,
            victim_life_seq: 1,
            weapon_data: 0x42,
            x: 500,
            y: 700,
            tick: crate::wire::current_tick(),
        },
    );

    super::assert_death_packet_eq_ignoring_green(
        &read_reliable_payload_from_channel(&rx_a),
        &[PLAYER_DEATH, 0xff, 1, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_score_update(&read_reliable_payload_from_channel(&rx_a), damaged, 0, 1);
    assert_score_update(&read_reliable_payload_from_channel(&rx_a), shooter, 1, 0);
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        create_turret_link(damaged, u16::MAX)
    );
    drain_available(&rx_b);
    assert!(current_turret_packets(&state).is_empty());
}

#[test]
fn server_rate_limits_attach_request_bursts() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    update_frequency(&state, beta, &frequency_change_request(&state, beta, 0));
    consume_pending_lifecycle_baseline(&state, beta);
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        create_turret_link(alpha, beta)
    );
    drain_available(&rx_b);

    relay_attach_request(&state, alpha, &attach_request_payload(u16::MAX));
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        create_turret_link(alpha, u16::MAX)
    );
    drain_available(&rx_b);

    relay_attach_request(&state, alpha, &attach_request_payload(beta));
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        create_turret_link(alpha, beta)
    );
    drain_available(&rx_b);

    relay_attach_request(&state, alpha, &attach_request_payload(u16::MAX));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn server_rejects_invalid_spectate_targets() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    update_spectate(&state, alpha, &spectate_request_payload(999));
    update_spectate(&state, alpha, &spectate_request_payload(alpha));
    assert!(rx_a.try_recv().is_err());

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let beta = state.clients.get_mut(&beta).expect("beta should exist");
        beta.player.ship = 8;
        beta.alive = false;
    }
    update_spectate(&state, alpha, &spectate_request_payload(beta));
    assert!(rx_a.try_recv().is_err());

    update_spectate(&state, alpha, &spectate_request_payload(u16::MAX));
    assert_eq!(
        read_reliable_payload_from_channel(&rx_a),
        spectate_packet(false)
    );
}

#[test]
fn scoped_observer_views_are_read_only_and_reject_stale_or_playing_scopes() {
    use crate::spectator_view::SpectatorViewRequest;
    let shared = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        free_for_all: false,
        bot_count: 1,
        ..ServerConfig::default()
    })));
    let (tx, _rx) = mpsc::channel();
    let id = register_connection(&shared, tx);
    join_arena(&shared, id, 8);
    let mut request = {
        let mut state = shared.lock().unwrap();
        state.initialize_configured_bots();
        let viewer = state.clients.get_mut(&id).unwrap();
        viewer.snapshot_ready = true;
        viewer.last_security_valid = Some(true);
        SpectatorViewRequest {
            target: u16::MAX,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: viewer.arena_stream_id,
            life_seq: viewer.life_seq,
            sequence: u16::MAX,
            next_input_sequence: Some(321),
            x: 1000.0,
            y: 1200.0,
        }
    };
    update_spectate(&shared, id, &request.encode());
    let assert_anchor = |x: u16, y: u16| {
        let state = shared.lock().unwrap();
        let viewer = &state.clients[&id];
        let anchor = viewer.aoi_observer_position.as_ref().unwrap();
        assert_eq!(u16::from_le_bytes(anchor[4..6].try_into().unwrap()), x);
        assert_eq!(u16::from_le_bytes(anchor[15..17].try_into().unwrap()), y);
        assert!(!viewer.alive);
        assert!(viewer.last_position.is_none());
        assert!(viewer.visibility_cell.is_none());
        assert_eq!(viewer.positions_received, 0);
        assert_eq!(viewer.spectator_next_input_sequence, Some(321));
    };
    assert_anchor(1000, 1200);
    for stale in [
        SpectatorViewRequest {
            server_session_id: request.server_session_id + 1,
            sequence: 0,
            x: 2000.0,
            ..request
        },
        SpectatorViewRequest {
            arena_stream_id: request.arena_stream_id + 1,
            sequence: 0,
            x: 2000.0,
            ..request
        },
        SpectatorViewRequest {
            life_seq: request.life_seq + 1,
            sequence: 0,
            x: 2000.0,
            ..request
        },
        SpectatorViewRequest {
            x: -1.0,
            sequence: 0,
            ..request
        },
        SpectatorViewRequest {
            x: 16384.0,
            sequence: 0,
            ..request
        },
        SpectatorViewRequest {
            x: f32::NAN,
            sequence: 0,
            ..request
        },
        SpectatorViewRequest {
            x: 2000.0,
            ..request
        },
        SpectatorViewRequest {
            target: id,
            sequence: 0,
            ..request
        },
    ] {
        update_spectate(&shared, id, &stale.encode());
        assert_anchor(1000, 1200);
    }
    request.sequence = 0;
    request.x = 1400.0;
    update_spectate(&shared, id, &request.encode());
    assert_anchor(1400, 1200);
    let (bot_id, bx, by) = {
        let state = shared.lock().unwrap();
        let bot = &state.bots[0];
        (bot.player.id, bot.x.round() as u16, bot.y.round() as u16)
    };
    request.sequence = 1;
    request.target = bot_id;
    update_spectate(&shared, id, &request.encode());
    assert_anchor(bx, by);
    request.sequence = 2;
    request.next_input_sequence = None;
    update_spectate(&shared, id, &request.encode());
    assert_anchor(bx, by);
    shared
        .lock()
        .unwrap()
        .clients
        .get_mut(&id)
        .unwrap()
        .player
        .ship = 0;
    request.sequence = 3;
    request.next_input_sequence = Some(999);
    request.target = u16::MAX;
    request.x = 2000.0;
    update_spectate(&shared, id, &request.encode());
    let state = shared.lock().unwrap();
    assert_eq!(state.clients[&id].spectator_view_sequence, Some(2));
    assert_eq!(state.clients[&id].spectator_next_input_sequence, Some(321));
}
