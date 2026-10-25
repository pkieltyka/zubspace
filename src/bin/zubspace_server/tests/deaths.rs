use super::*;

#[test]
fn connection_rejects_client_death_request_packets() {
    // Delta, not absolute: the reject counters are process-global and `cargo
    // test` runs this beside other security tests that reset them.
    let rejects_before = crate::security::peek_security_reject_count_for_test(
        "death",
        "client_reported_death_disabled",
    );
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    broadcast_position(
        &state,
        player_id,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx);

    let payload = [
        DEATH_REQUEST,
        (player_id & 0xff) as u8,
        (player_id >> 8) as u8,
        0,
        0,
    ];
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &payload),
        "test-peer",
        &mut joined,
    );

    let state = state.lock().expect("server state should not be poisoned");
    let player = state.clients.get(&player_id).expect("player exists");
    assert!(player.alive);
    assert_eq!(player.player.losses, 0);
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "death",
            "client_reported_death_disabled"
        ) - rejects_before,
        1
    );
}

#[test]
fn server_rejects_non_self_death_requests_without_server_damage() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let killer = register_connection(&state, tx_a);
    let killed = register_connection(&state, tx_b);
    join_arena(&state, killer, 0);
    join_arena(&state, killed, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position(
        &state,
        killed,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_a);

    relay_death(&state, killed, &[DEATH_REQUEST, 0, 0, 44, 0]);

    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state.clients.get(&killer).map(|client| client.player.wins),
        Some(0)
    );
    assert_eq!(
        state
            .clients
            .get(&killed)
            .map(|client| client.player.losses),
        Some(0)
    );
    assert_eq!(
        state.clients.get(&killed).map(|client| client.alive),
        Some(true)
    );
    assert_eq!(
        state.clients.get(&killed).map(|client| client.player.flags),
        Some(0)
    );
    assert!(
        state
            .clients
            .get(&killed)
            .and_then(|client| client.last_position.as_ref())
            .is_some()
    );
}

#[test]
fn server_rejects_death_requests_from_invalid_killers() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let killer = register_connection(&state, tx_a);
    let killed = register_connection(&state, tx_b);
    let cleanup_killer = register_connection(&state, tx_c);
    join_arena(&state, killer, 0);
    join_arena(&state, killed, 0);
    join_arena(&state, cleanup_killer, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    broadcast_position(
        &state,
        killed,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_a);
    drain_available(&rx_c);

    let unknown_killer_id = 77u16;
    relay_death(
        &state,
        killed,
        &[
            DEATH_REQUEST,
            (unknown_killer_id & 0xff) as u8,
            (unknown_killer_id >> 8) as u8,
            0,
            0,
        ],
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert!(rx_c.try_recv().is_err());

    update_ship(&state, killer, &ship_change_request(8));
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    relay_death(
        &state,
        killed,
        &[
            DEATH_REQUEST,
            (killer & 0xff) as u8,
            (killer >> 8) as u8,
            0,
            0,
        ],
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert!(rx_c.try_recv().is_err());

    update_ship(&state, killer, &ship_change_request(0));
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    broadcast_position(
        &state,
        killer,
        &client_position_packet(0x1122_3344, 520, 720),
    );
    drain_available(&rx_b);
    drain_available(&rx_c);
    relay_death(
        &state,
        killer,
        &[
            DEATH_REQUEST,
            (cleanup_killer & 0xff) as u8,
            (cleanup_killer >> 8) as u8,
            0,
            0,
        ],
    );
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    relay_death(
        &state,
        killed,
        &[
            DEATH_REQUEST,
            (killer & 0xff) as u8,
            (killer >> 8) as u8,
            0,
            0,
        ],
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert!(rx_c.try_recv().is_err());

    let state = state.lock().expect("server state should not be poisoned");
    let killed = state.clients.get(&killed).expect("killed player exists");
    assert!(killed.alive);
    assert_eq!(killed.player.losses, 0);
}

#[test]
fn server_rejects_death_requests_without_current_unsafe_position() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let killer = register_connection(&state, tx_a);
    let killed = register_connection(&state, tx_b);
    join_arena(&state, killer, 0);
    join_arena(&state, killed, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_death(
        &state,
        killed,
        &[
            DEATH_REQUEST,
            (killer & 0xff) as u8,
            (killer >> 8) as u8,
            0,
            0,
        ],
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    let (safe_x, safe_y) = first_safe_zone_position();
    broadcast_position(
        &state,
        killed,
        &client_position_packet(0x1122_3344, safe_x, safe_y),
    );
    drain_available(&rx_a);
    relay_death(
        &state,
        killed,
        &[
            DEATH_REQUEST,
            (killer & 0xff) as u8,
            (killer >> 8) as u8,
            0,
            0,
        ],
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    let state = state.lock().expect("server state should not be poisoned");
    let killed = state.clients.get(&killed).expect("killed player exists");
    assert!(killed.alive);
    assert_eq!(killed.player.losses, 0);
}

#[test]
fn server_records_self_kill_without_awarding_a_win() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    reset_server_runtime_without_initial_bounty(&state, player_id);
    broadcast_position(
        &state,
        player_id,
        &client_position_packet(0x1122_3344, 500, 700),
    );

    relay_death(
        &state,
        player_id,
        &[
            DEATH_REQUEST,
            (player_id & 0xff) as u8,
            (player_id >> 8) as u8,
            0,
            0,
        ],
    );

    super::assert_death_packet_eq_ignoring_green(
        &read_reliable_payload_from_channel(&rx),
        &[
            PLAYER_DEATH,
            0xff,
            (player_id & 0xff) as u8,
            (player_id >> 8) as u8,
            (player_id & 0xff) as u8,
            (player_id >> 8) as u8,
            0,
            0,
            0,
            0,
        ],
    );
    assert_score_update(&read_reliable_payload_from_channel(&rx), player_id, 0, 1);
    assert!(rx.try_recv().is_err());

    let state = state.lock().expect("server state should not be poisoned");
    let player = state.clients.get(&player_id).expect("player exists");
    assert!(!player.alive);
    assert_eq!(player.player.wins, 0);
    assert_eq!(player.player.losses, 1);
}

#[test]
fn server_persists_scores_for_reconnecting_player_identity() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let killer = register_connection(&state, tx_a);
    let killed = register_connection(&state, tx_b);
    set_player_identity_and_name(
        &state,
        killer,
        "100000000001".to_string(),
        "killer".to_string(),
    );
    set_player_identity_and_name(
        &state,
        killed,
        "100000000002".to_string(),
        "killed".to_string(),
    );
    join_arena(&state, killer, 0);
    join_arena(&state, killed, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(
        &state,
        killed,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_a);
    broadcast_position(
        &state,
        killer,
        &client_position_packet(0x1122_3345, 520, 700),
    );
    drain_available(&rx_b);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&killed)
            .expect("killed player exists")
            .server_energy = 1;
    }

    crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 1,
            owner_id: killer,
            owner_life_seq: 1,
            victim_id: killed,
            victim_life_seq: 1,
            weapon_data: 0x42,
            x: 500,
            y: 700,
            tick: crate::wire::current_tick(),
        },
    );
    drain_available(&rx_a);
    drain_available(&rx_b);
    disconnect_player(&state, killed);

    let (tx_rejoin, _rx_rejoin) = mpsc::channel();
    let rejoined = register_connection(&state, tx_rejoin);
    set_player_identity_and_name(
        &state,
        rejoined,
        "100000000002".to_string(),
        "killed".to_string(),
    );

    disconnect_player(&state, killer);
    let (winner_tx, _winner_rx) = mpsc::channel();
    let rejoined_winner = register_connection(&state, winner_tx);
    assert!(set_player_identity_and_name(
        &state,
        rejoined_winner,
        "100000000001".to_owned(),
        "killer".to_owned()
    ));
    let state = state.lock().expect("server state should not be poisoned");
    let winner = &state.clients[&rejoined_winner].player;
    assert_eq!((winner.kill_points, winner.wins, winner.losses), (30, 1, 0));
    let roster = crate::roster::player_entering(winner);
    assert_eq!(u32::from_le_bytes(roster[43..47].try_into().unwrap()), 30);
    assert_eq!(
        state.identity_stats.get("100000000001").copied(),
        Some(PlayerStats {
            assists: 0,
            kill_points: 30,
            flag_points: 0,
            wins: 1,
            losses: 0,
        })
    );
    assert_eq!(
        state.identity_stats.get("100000000002").copied(),
        Some(PlayerStats {
            assists: 0,
            kill_points: 0,
            flag_points: 0,
            wins: 0,
            losses: 1,
        })
    );
    let player = &state
        .clients
        .get(&rejoined)
        .expect("rejoined player should exist")
        .player;
    assert_eq!(player.wins, 0);
    assert_eq!(player.losses, 1);
}
