use super::*;

fn grant_server_bricks(state: &Arc<Mutex<ServerState>>, player_id: u16, count: u8) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    client.server_weapons.bricks = count;
    client.last_brick_drop_tick = None;
}

#[test]
fn server_relays_drop_bricks_to_joined_players() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );
    drain_available(&rx_b);
    update_frequency(&state, alpha, &frequency_change_request(&state, alpha, 7));
    consume_pending_lifecycle_baseline(&state, alpha);
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(&state, alpha, &client_position_packet(0, 512, 640));
    drain_available(&rx_b);

    grant_server_bricks(&state, alpha, 1);
    let request = drop_brick_request_payload(512, 640);
    relay_drop_brick(&state, alpha, &request);

    let alpha_brick = read_reliable_payload_from_channel(&rx_a);
    let beta_brick = read_reliable_payload_from_channel(&rx_b);
    assert_eq!(alpha_brick, beta_brick);
    assert_eq!(alpha_brick[0], BRICK_DROPPED);
    // Historical audit gap (multi-tile brick walls): the packet now carries
    // the wall's start/end endpoints rather than a single tile
    // repeated twice. The center the player aimed at must lie
    // inclusively between start and end along both axes.
    let center = brick_tile_from_ship_position(512, 640, 0);
    let start_x = u16::from_le_bytes(alpha_brick[1..3].try_into().unwrap());
    let start_y = u16::from_le_bytes(alpha_brick[3..5].try_into().unwrap());
    let end_x = u16::from_le_bytes(alpha_brick[5..7].try_into().unwrap());
    let end_y = u16::from_le_bytes(alpha_brick[7..9].try_into().unwrap());
    assert!(
        (start_x.min(end_x)..=start_x.max(end_x)).contains(&center.0)
            && (start_y.min(end_y)..=start_y.max(end_y)).contains(&center.1),
        "center {center:?} must lie inside wall span [{:?} .. {:?}]",
        (start_x, start_y),
        (end_x, end_y),
    );
    assert_eq!(
        u16::from_le_bytes(alpha_brick[9..11].try_into().unwrap()),
        7
    );
    assert_eq!(
        u16::from_le_bytes(alpha_brick[11..13].try_into().unwrap()),
        0
    );
    assert!(u32::from_le_bytes(alpha_brick[13..17].try_into().unwrap()) > 0);

    relay_drop_brick(&state, alpha, &[DROP_BRICK_REQUEST, 1, 2]);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    update_ship(&state, alpha, &ship_change_request(8));
    drain_available(&rx_a);
    drain_available(&rx_b);
    relay_drop_brick(&state, alpha, &request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    update_ship(&state, alpha, &ship_change_request(0));
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3345, 500, 700),
    );
    drain_available(&rx_b);
    relay_death(
        &state,
        alpha,
        &[
            DEATH_REQUEST,
            (alpha & 0xff) as u8,
            (alpha >> 8) as u8,
            0,
            0,
        ],
    );
    drain_available(&rx_a);
    drain_available(&rx_b);
    relay_drop_brick(&state, alpha, &request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn server_rejects_brick_drops_without_current_ship_position() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    grant_server_bricks(&state, alpha, 1);

    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );
    drain_available(&rx_b);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(900, 900));
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    let state = state.lock().expect("server state should not be poisoned");
    assert!(state.active_bricks.is_empty());
}

#[test]
fn server_replays_active_bricks_to_late_joiners() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );

    grant_server_bricks(&state, alpha, 1);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));
    let dropped = read_reliable_payload_from_channel(&rx_a);
    assert_eq!(dropped[0], BRICK_DROPPED);

    let (tx_b, rx_b) = mpsc::channel();
    let beta = register_connection(&state, tx_b);
    join_arena(&state, beta, 0);

    assert_eq!(
        read_reliable_payload_with_type(&rx_b, BRICK_DROPPED),
        dropped
    );
}

#[test]
fn server_expires_bricks_with_clear_and_replay_packets() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);

    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );
    grant_server_bricks(&state, alpha, 1);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));
    let expired = read_reliable_payload_from_channel(&rx_a);
    assert_eq!(expired[0], BRICK_DROPPED);

    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3345, 700, 700),
    );
    grant_server_bricks(&state, alpha, 1);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(700, 700));
    let active = read_reliable_payload_from_channel(&rx_a);
    assert_eq!(active[0], BRICK_DROPPED);

    let tick = crate::wire::current_tick();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.active_bricks[0].expires_at_tick = crate::continuum::continuum_tick_sub(tick, 100);
        state.active_bricks[1].expires_at_tick = crate::continuum::continuum_tick_add(tick, 100);
    }

    let packets = expire_bricks(&state, tick);
    assert_eq!(packets, vec![clear_bricks_packet(), active.clone()]);

    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(state.active_bricks.len(), 1);
    assert_eq!(state.active_bricks[0].packet, active);
}

#[test]
fn server_rejects_brick_drops_without_server_inventory() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let _beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, _beta, 0);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .server_weapons
        .bricks = 0;
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );
    drain_available(&rx_b);

    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));

    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    assert!(state.active_bricks.is_empty());
}

#[test]
fn server_spends_server_brick_inventory() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );

    grant_server_bricks(&state, alpha, 1);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));
    assert_eq!(read_reliable_payload_from_channel(&rx_a)[0], BRICK_DROPPED);

    {
        let state = state.lock().expect("server state should not be poisoned");
        assert_eq!(
            state
                .clients
                .get(&alpha)
                .map(|client| client.server_weapons.bricks),
            Some(0)
        );
    }

    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));
    assert!(rx_a.try_recv().is_err());
}

#[test]
fn server_rejects_brick_drops_inside_server_cooldown() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );

    grant_server_bricks(&state, alpha, 2);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 640));
    assert_eq!(read_reliable_payload_from_channel(&rx_a)[0], BRICK_DROPPED);
    // A different, unoccupied row within the source tolerance isolates cooldown
    // from the independent occupied-center rejection.
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(512, 688));

    assert!(rx_a.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_weapons.bricks, 1);
    assert_eq!(state.active_bricks.len(), 1);
}

#[test]
fn server_rejects_brick_drops_during_manual_warp_bomb_cooldown() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 512, 640),
    );
    let tick = 40_000;
    {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&alpha).unwrap();
        client.server_weapons.bricks = 1;
        client.server_warp_bomb_lock_until_tick =
            Some(crate::continuum::continuum_tick_add(tick, 50));
    }
    relay_drop_brick_at_tick(&state, alpha, &drop_brick_request_payload(512, 640), tick);
    let state = state.lock().unwrap();
    assert_eq!(state.clients[&alpha].server_weapons.bricks, 1);
    assert!(state.active_bricks.is_empty());
}

#[test]
fn brick_drop_threads_request_entry_tick_through_mutex_contention_and_wrap() {
    for request_tick in [345_678, 0x7fff_fff0] {
        let state = Arc::new(Mutex::new(team_mode_state()));
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        let alpha = register_connection(&state, tx_a);
        let beta = register_connection(&state, tx_b);
        join_arena(&state, alpha, 0);
        join_arena(&state, beta, 0);
        drain_available(&rx_a);
        drain_available(&rx_b);
        broadcast_position(
            &state,
            alpha,
            &client_position_packet(0x1122_3344, 512, 640),
        );
        drain_available(&rx_b);
        grant_server_bricks(&state, alpha, 1);

        let request = drop_brick_request_payload(512, 640);
        let guard = state.lock().expect("state lock");
        let (started_tx, started_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).expect("signal worker start");
            relay_drop_brick_at_tick(&worker_state, alpha, &request, request_tick);
        });
        started_rx.recv().expect("worker should start");
        std::thread::sleep(Duration::from_millis(15));
        drop(guard);
        worker.join().expect("brick worker should finish");

        let alpha_packet = read_reliable_payload_from_channel(&rx_a);
        let beta_packet = read_reliable_payload_from_channel(&rx_b);
        assert_eq!(alpha_packet, beta_packet);
        assert_eq!(alpha_packet[0], BRICK_DROPPED);
        assert_eq!(
            u32::from_le_bytes(alpha_packet[13..17].try_into().unwrap()),
            request_tick
        );

        let locked = state.lock().expect("state lock");
        let client = locked.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(client.last_brick_drop_tick, Some(request_tick));
        assert_eq!(locked.active_bricks.len(), 1);
        assert_eq!(locked.active_bricks[0].packet, alpha_packet);
        assert_eq!(
            locked.active_bricks[0].expires_at_tick,
            crate::continuum::continuum_tick_add(
                request_tick,
                crate::arena_settings::arena_brick_lifetime_ticks(),
            )
        );
    }
}

#[test]
fn server_brick_drop_in_open_space_lays_multi_tile_wall() {
    // Historical audit gap (multi-tile brick walls). When a player drops a
    // brick in open space (no nearby static walls or other bricks),
    // the resulting `ServerBrick.tiles` list should contain more
    // than one tile (the wall extends perpendicular to heading up
    // to `arena_brick_span()` tiles total). The wire packet must
    // also report `start != end`.
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 8200, 8200),
    );

    grant_server_bricks(&state, alpha, 1);
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(8200, 8200));
    let dropped = read_reliable_payload_from_channel(&rx_a);
    assert_eq!(dropped[0], BRICK_DROPPED);

    let start_x = u16::from_le_bytes(dropped[1..3].try_into().unwrap());
    let start_y = u16::from_le_bytes(dropped[3..5].try_into().unwrap());
    let end_x = u16::from_le_bytes(dropped[5..7].try_into().unwrap());
    let end_y = u16::from_le_bytes(dropped[7..9].try_into().unwrap());
    assert!(
        (start_x, start_y) != (end_x, end_y),
        "open-space wall must report distinct endpoints, got {:?}..{:?}",
        (start_x, start_y),
        (end_x, end_y),
    );

    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(state.active_bricks.len(), 1);
    let wall = &state.active_bricks[0].tiles;
    assert!(
        wall.len() > 1,
        "open-space wall must occupy more than one tile, got {wall:?}"
    );
    assert!(
        wall.len() <= crate::arena_settings::arena_brick_span() as usize + 1,
        "wall length must not exceed brick_span (+1 for center), got {wall:?}"
    );
}

#[test]
fn nearby_brick_request_coordinates_cannot_move_the_authoritative_drop_origin() {
    for (direction, dx, dy) in [(0_u8, 0_i32, 48_i32), (10, 48, 0), (6, 48, 0), (26, -48, 0)] {
        let state = Arc::new(Mutex::new(team_mode_state()));
        let (tx, rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        drain_available(&rx);
        let mut position = client_position_packet(1, 8200, 8200);
        position[1] = direction;
        sync_client_position_canonical_state_from_wire(&mut position);
        broadcast_position(&state, player, &position);
        grant_server_bricks(&state, player, 1);
        let request = drop_brick_request_payload((8200_i32 + dx) as u16, (8200_i32 + dy) as u16);
        relay_drop_brick(&state, player, &request);
        let packet = read_reliable_payload_from_channel(&rx);
        assert_eq!(packet[0], BRICK_DROPPED);
        let expected = brick_tile_from_ship_position(8200, 8200, direction);
        let locked = state.lock().unwrap();
        let wall = &locked.active_bricks[0].tiles;
        if direction == 0 {
            assert!(
                wall.iter().all(|tile| tile.1 == expected.1),
                "a tolerated request is not authority to displace the row: {wall:?}"
            );
        } else {
            assert!(
                wall.iter().all(|tile| tile.0 == expected.0),
                "a tolerated request is not authority to displace the column: {wall:?}"
            );
        }
    }
}

#[test]
fn occupied_brick_center_does_not_shrink_existing_wall_or_spend_inventory() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player = register_connection(&state, tx);
    join_arena(&state, player, 0);
    drain_available(&rx);
    broadcast_position(
        &state,
        player,
        &client_position_packet(0x1122_3344, 8200, 8200),
    );
    grant_server_bricks(&state, player, 2);
    relay_drop_brick_at_tick(
        &state,
        player,
        &drop_brick_request_payload(8200, 8200),
        1000,
    );
    let first = read_reliable_payload_from_channel(&rx);
    assert_eq!(first[0], BRICK_DROPPED);
    let before = {
        let state = state.lock().unwrap();
        assert!(state.active_bricks[0].tiles.len() > 1);
        (
            state.active_bricks.clone(),
            state.next_brick_id,
            state.clients[&player].last_brick_drop_tick,
        )
    };
    // Well after cooldown, but before expiry: occupancy must be the reason.
    relay_drop_brick_at_tick(
        &state,
        player,
        &drop_brick_request_payload(8200, 8200),
        2000,
    );
    assert!(rx.try_recv().is_err());
    let guard = state.lock().unwrap();
    assert_eq!(guard.active_bricks, before.0);
    assert_eq!(guard.next_brick_id, before.1);
    assert_eq!(guard.clients[&player].last_brick_drop_tick, before.2);
    assert_eq!(guard.clients[&player].server_weapons.bricks, 1);
    let expires = before.0[0].expires_at_tick;
    drop(guard);
    relay_drop_brick_at_tick(
        &state,
        player,
        &drop_brick_request_payload(8200, 8200),
        expires,
    );
    assert_eq!(read_reliable_payload_from_channel(&rx)[0], BRICK_DROPPED);
    let state = state.lock().unwrap();
    assert_eq!(state.active_bricks.len(), 1);
    assert_eq!(state.active_bricks[0].tiles, before.0[0].tiles);
    assert_eq!(state.clients[&player].server_weapons.bricks, 0);
}

#[test]
fn brick_id_wrap_skips_live_lines_without_overwriting_their_replay_identity() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player = register_connection(&state, tx);
    join_arena(&state, player, 0);
    broadcast_position(&state, player, &client_position_packet(0, 512, 640));
    grant_server_bricks(&state, player, 1);
    drain_available(&rx);
    let originals = [u16::MAX, 0].map(|id| {
        let mut packet = vec![BRICK_DROPPED];
        for value in [10_u16, 10, 10, 10, 77, id] {
            packet.extend(value.to_le_bytes());
        }
        packet.extend(50_u32.to_le_bytes());
        packet
    });
    {
        let mut guard = state.lock().unwrap();
        guard.next_brick_id = u16::MAX;
        for packet in &originals {
            guard.active_bricks.push(ServerBrick {
                tiles: vec![(10, 10)],
                packet: packet.clone(),
                expires_at_tick: 1000,
            });
        }
    }
    relay_drop_brick_at_tick(&state, player, &drop_brick_request_payload(512, 640), 100);
    let published = read_reliable_payload_from_channel(&rx);
    assert_eq!(published[0], BRICK_DROPPED);
    assert_eq!(u16::from_le_bytes(published[11..13].try_into().unwrap()), 1);
    let guard = state.lock().unwrap();
    assert_eq!(guard.active_bricks.len(), 3);
    for original in originals {
        assert!(
            guard
                .active_bricks
                .iter()
                .any(|line| line.packet == original)
        );
    }
    assert_eq!(guard.next_brick_id, 2);
    assert_eq!(guard.clients[&player].server_weapons.bricks, 0);
}

#[test]
fn exhausted_brick_identity_space_rejects_without_inventory_cooldown_or_cursor_mutation() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player = register_connection(&state, tx);
    join_arena(&state, player, 0);
    broadcast_position(&state, player, &client_position_packet(0, 512, 640));
    grant_server_bricks(&state, player, 1);
    drain_available(&rx);
    {
        let mut guard = state.lock().unwrap();
        guard.next_brick_id = 7;
        // Reserve every wire identity. Geometry stays away from the request;
        // this fixture isolates ID pressure from occupied-center rejection.
        guard.active_bricks = (0..=u16::MAX)
            .map(|id| {
                let mut packet = vec![BRICK_DROPPED];
                for value in [10_u16, 10, 10, 10, 77, id] {
                    packet.extend(value.to_le_bytes());
                }
                packet.extend(50_u32.to_le_bytes());
                ServerBrick {
                    tiles: vec![(10, 10)],
                    packet,
                    expires_at_tick: 1000,
                }
            })
            .collect();
    }
    let request = drop_brick_request_payload(512, 640);
    relay_drop_brick_at_tick(&state, player, &request, 100);
    assert!(rx.try_recv().is_err());
    {
        let mut guard = state.lock().unwrap();
        assert_eq!(guard.next_brick_id, 7);
        assert_eq!(guard.clients[&player].server_weapons.bricks, 1);
        assert_eq!(guard.clients[&player].last_brick_drop_tick, None);
        assert_eq!(guard.active_bricks.len(), usize::from(u16::MAX) + 1);
        // Exactly expired identities become reusable without touching any
        // other still-live line. The next valid drop must commit once.
        guard.active_bricks[7].expires_at_tick = 100;
    }
    relay_drop_brick_at_tick(&state, player, &request, 100);
    let published = read_reliable_payload_from_channel(&rx);
    assert_eq!(u16::from_le_bytes(published[11..13].try_into().unwrap()), 7);
    let guard = state.lock().unwrap();
    assert_eq!(guard.next_brick_id, 8);
    assert_eq!(guard.clients[&player].server_weapons.bricks, 0);
    assert_eq!(guard.clients[&player].last_brick_drop_tick, Some(100));
    assert_eq!(guard.active_bricks.len(), usize::from(u16::MAX) + 1);
}
