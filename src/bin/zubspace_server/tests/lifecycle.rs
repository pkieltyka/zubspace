use super::*;

#[test]
fn arena_join_materializes_bot_positions_on_the_request_clock_before_first_step() {
    for request_tick in [53_808, 65_535, 65_536] {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            bot_count: 2,
            ..ServerConfig::default()
        })));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena_at_tick(&state, player_id, 0, request_tick);
        let positions = rx
            .try_iter()
            .filter(|packet| packet.first() == Some(&crate::arena::LARGE_POSITION_PACKET))
            .collect::<Vec<_>>();
        assert_eq!(
            positions.len(),
            2,
            "both bootstrap bot positions are present"
        );
        for packet in positions {
            assert_eq!(
                u16::from_le_bytes([packet[2], packet[3]]),
                request_tick as u16
            );
        }
    }
}

#[test]
fn arena_join_threads_one_request_tick_through_bootstrap_and_runtime_at_wrap() {
    for request_tick in [345_678, 0x7fff_fffe] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena_at_tick(&state, player_id, 0, request_tick);

        let bootstrap_packet = read_reliable_payload_from_channel(&rx);
        let bootstrap = crate::clock_bootstrap::ServerClockBootstrap::parse(&bootstrap_packet)
            .expect("bootstrap should parse");
        assert_eq!(bootstrap.server_tick, request_tick);
        let assignment = read_reliable_payload_from_channel(&rx);
        assert_eq!(assignment[0], 0x01);

        let locked = state.lock().expect("state lock");
        let client = locked.clients.get(&player_id).expect("client should exist");
        assert_eq!(client.life_started_tick, Some(request_tick));
        assert_eq!(client.last_energy_recharge_tick, Some(request_tick));
        assert_eq!(client.last_continuous_energy_drain_tick, Some(request_tick));
        let directive = client
            .observer_combat_timeline
            .current_directive()
            .expect("join must initialize the observer combat timeline");
        assert_ne!(directive.generation, 0);
        assert_eq!(directive.issued_server_tick, request_tick);
        assert_eq!(directive.effective_server_tick, request_tick);
        assert_eq!(
            directive.effective_from_input_sequence, bootstrap.input_sequence_base,
            "timeline and clock bootstrap must name the same first input"
        );
        assert_eq!(
            client.observer_combat_timeline.arena_stream_id(),
            Some(bootstrap.arena_stream_id)
        );
        assert_eq!(
            client.observer_combat_timeline.observer(),
            locked.active_player_runtime_identity(player_id)
        );
    }
}

#[test]
fn arena_join_uses_the_trusted_server_observer_network_profile() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        observer_network_profile: crate::combat_timeline::ObserverNetworkProfile::SupportedSlowWan,
        ..ServerConfig::default()
    })));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena_at_tick(&state, player_id, 0, 1_000);

    let locked = state.lock().expect("state lock");
    let directive = locked.clients[&player_id]
        .observer_combat_timeline
        .current_directive()
        .expect("join must install the configured observer policy");
    assert_eq!(
        directive.profile,
        crate::combat_timeline::ObserverNetworkProfile::SupportedSlowWan
    );
    assert_eq!(directive.target_delay_ticks, 15);
    assert_eq!(directive.ceiling_delay_ticks, 20);
}

#[test]
fn observer_timeline_survives_soft_life_and_rekeys_on_hard_rejoin() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena_at_tick(&state, player_id, 0, 100);

    let (first_runtime, first_stream, first_directive) = {
        let locked = state.lock().expect("state");
        let client = locked.clients.get(&player_id).expect("client");
        (
            locked
                .active_player_runtime_identity(player_id)
                .expect("joined runtime"),
            client.arena_stream_id,
            client
                .observer_combat_timeline
                .current_directive()
                .expect("first directive"),
        )
    };

    state
        .lock()
        .expect("state")
        .advance_client_life(player_id)
        .expect("soft life boundary");
    {
        let locked = state.lock().expect("state");
        let client = locked.clients.get(&player_id).expect("client");
        let second_runtime = locked
            .active_player_runtime_identity(player_id)
            .expect("second runtime");
        assert_ne!(second_runtime.life, first_runtime.life);
        assert_eq!(
            client.observer_combat_timeline.observer(),
            Some(second_runtime)
        );
        assert_eq!(
            client.observer_combat_timeline.current_directive(),
            Some(first_directive),
            "death/respawn keeps the observer-wide schedule"
        );
    }

    leave_arena(&state, player_id);
    join_arena_at_tick(&state, player_id, 0, 200);
    let locked = state.lock().expect("state");
    let client = locked.clients.get(&player_id).expect("client");
    let replacement = client
        .observer_combat_timeline
        .current_directive()
        .expect("replacement directive");
    assert_ne!(client.arena_stream_id, first_stream);
    assert_ne!(replacement.generation, first_directive.generation);
    assert_eq!(replacement.issued_server_tick, 200);
}

#[test]
fn server_assigns_spawn_coordinates_to_live_arena_joins() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);

    join_arena(&state, player_id, 0);

    assert_join_clock_bootstrap(&read_reliable_payload_from_channel(&rx), player_id);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x01, 0, 0]);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x02]);
    assert_eq!(read_reliable_payload_from_channel(&rx)[0], 0x03);
    assert_arena_settings(&read_reliable_payload_from_channel(&rx));
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        [TOGGLE_SEND_DAMAGE, 0]
    );
    assert_map_information(&read_reliable_payload_from_channel(&rx));
    assert_set_coordinates_for_map(&read_reliable_payload_from_channel(&rx), zone_map());
}

#[test]
fn server_does_not_assign_spawn_coordinates_to_spectator_joins() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);

    join_arena(&state, player_id, 8);

    assert_join_clock_bootstrap(&read_reliable_payload_from_channel(&rx), player_id);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x01, 0, 0]);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x02]);
    assert_eq!(read_reliable_payload_from_channel(&rx)[0], 0x03);
    assert_arena_settings(&read_reliable_payload_from_channel(&rx));
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        [TOGGLE_SEND_DAMAGE, 0]
    );
    assert_map_information(&read_reliable_payload_from_channel(&rx));
    assert_arena_directory(&read_reliable_payload_from_channel(&rx), 1);
}

#[test]
fn server_replays_configured_bots_to_arena_joins() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            bot_count: 2,
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    {
        let mut state = state.lock().expect("state");
        state.initialize_configured_bots();
        state.bots[0].player.assists = 3;
        state.bots[0].player.kill_points = 123;
        state.bots[0].player.flag_points = 2_100;
        state.bots[0].player.wins = 4;
        state.bots[0].player.losses = 5;
    }
    let names = {
        let guard = state.lock().unwrap();
        [
            guard.bots[0].player.name.clone(),
            guard.bots[1].player.name.clone(),
        ]
    };
    assert_ne!(names[0], names[1]);
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);

    join_arena(&state, player_id, 0);

    assert_join_clock_bootstrap(&read_reliable_payload_from_channel(&rx), player_id);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x01, 0, 0]);
    assert_eq!(read_reliable_payload_from_channel(&rx), [0x02]);
    assert_eq!(read_reliable_payload_from_channel(&rx)[0], 0x03);
    let bot_one = read_reliable_payload_from_channel(&rx);
    let bot_two = read_reliable_payload_from_channel(&rx);
    assert_eq!(
        String::from_utf8_lossy(&bot_one[3..23]).trim_end_matches('\0'),
        names[0]
    );
    assert_eq!(u16::from_le_bytes(bot_one[51..53].try_into().unwrap()), 768);
    assert_eq!(u32::from_le_bytes(bot_one[43..47].try_into().unwrap()), 123);
    assert_eq!(
        &bot_one[64..],
        crate::roster::score_update(&state.lock().unwrap().bots[0].player)
    );
    assert_eq!(u32::from_le_bytes(bot_one[80..84].try_into().unwrap()), 3);
    assert_eq!(
        u32::from_le_bytes(bot_one[47..51].try_into().unwrap()),
        2_100
    );
    assert_eq!(u16::from_le_bytes(bot_one[55..57].try_into().unwrap()), 4);
    assert_eq!(u16::from_le_bytes(bot_one[57..59].try_into().unwrap()), 5);
    assert_eq!(
        String::from_utf8_lossy(&bot_two[3..23]).trim_end_matches('\0'),
        names[1]
    );
    assert_eq!(u16::from_le_bytes(bot_two[51..53].try_into().unwrap()), 769);
    assert_arena_settings(&read_reliable_payload_from_channel(&rx));
    assert_eq!(
        read_reliable_payload_from_channel(&rx),
        [TOGGLE_SEND_DAMAGE, 0]
    );
    assert_map_information(&read_reliable_payload_from_channel(&rx));
    assert_set_coordinates_for_map(&read_reliable_payload_from_channel(&rx), zone_map());

    let bot_position_one = read_non_reliable_from_channel(&rx);
    let bot_position_two = read_non_reliable_from_channel(&rx);
    let mut ids = vec![
        u16::from_le_bytes(bot_position_one[8..10].try_into().unwrap()),
        u16::from_le_bytes(bot_position_two[8..10].try_into().unwrap()),
    ];
    ids.sort_unstable();
    assert_eq!(ids, vec![768, 769]);
}

#[test]
fn server_orders_simultaneous_join_rosters_after_player_assignment() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_alpha, rx_alpha) = mpsc::channel();
    let (tx_beta, rx_beta) = mpsc::channel();
    let alpha = register_connection(&state, tx_alpha);
    let beta = register_connection(&state, tx_beta);

    join_arena(&state, beta, 0);

    assert!(
        rx_alpha.try_recv().is_err(),
        "unjoined registered clients must not receive another player's roster during bootstrap"
    );
    assert_join_clock_bootstrap(&read_reliable_payload_from_channel(&rx_beta), beta);
    assert_eq!(
        read_reliable_payload_from_channel(&rx_beta),
        [0x01, beta.to_le_bytes()[0], beta.to_le_bytes()[1]]
    );
    assert_eq!(read_reliable_payload_from_channel(&rx_beta), [0x02]);
    assert_eq!(
        player_entering_id(&read_reliable_payload_from_channel(&rx_beta)),
        beta
    );
    let beta_next = read_reliable_payload_from_channel(&rx_beta);
    assert_ne!(
        beta_next[0], 0x03,
        "beta must not receive alpha's roster before alpha has joined"
    );
    drain_available(&rx_beta);

    join_arena(&state, alpha, 0);

    assert_join_clock_bootstrap(&read_reliable_payload_from_channel(&rx_alpha), alpha);
    assert_eq!(
        read_reliable_payload_from_channel(&rx_alpha),
        [0x01, alpha.to_le_bytes()[0], alpha.to_le_bytes()[1]]
    );
    assert_eq!(read_reliable_payload_from_channel(&rx_alpha), [0x02]);
    assert_eq!(
        player_entering_id(&read_reliable_payload_from_channel(&rx_alpha)),
        alpha
    );
    assert_eq!(
        player_entering_id(&read_reliable_payload_from_channel(&rx_alpha)),
        beta,
        "late-finishing joiner must replay already joined peers"
    );

    assert_eq!(
        player_entering_id(&read_reliable_payload_from_channel(&rx_beta)),
        alpha,
        "already joined peer must receive the late-finishing joiner"
    );
}

#[test]
fn join_objective_replay_is_entry_snapshot_before_later_brick_and_prize_broadcasts() {
    for entry_tick in [567_890, 0x7fff_ff00] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_alpha, rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        drain_available(&rx_alpha);

        let cooldown = crate::arena_settings::arena_ship_weapon_fire_delay_ticks(0, 5);
        let old_brick_tick = crate::continuum::continuum_tick_sub(entry_tick, cooldown + 1);
        broadcast_position(&state, alpha, &client_position_packet(0, 512, 640));
        state
            .lock()
            .expect("state lock")
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .bricks = 1;
        relay_drop_brick_at_tick(
            &state,
            alpha,
            &drop_brick_request_payload(512, 640),
            old_brick_tick,
        );
        let old_brick = read_reliable_payload_from_channel(&rx_alpha);
        assert_eq!(old_brick[0], BRICK_DROPPED);
        state
            .lock()
            .expect("state lock")
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .bricks = 1;

        let old_prize = crate::hidden_prizes::hidden_prize_spawn_packet(1, 640, 768, 15, 300);
        {
            let mut locked = state.lock().expect("state lock");
            locked.active_hidden_prizes.push(ServerHiddenPrize {
                collectible_id: 1,
                x: 640,
                y: 768,
                prize_id: 15,
                expires_at_tick: crate::continuum::continuum_tick_add(entry_tick, 300),
            });
        }

        let (tx_beta, rx_beta) = mpsc::channel();
        let beta = register_connection(&state, tx_beta);
        let new_brick_tick = crate::continuum::continuum_tick_add(entry_tick, 1);
        let new_prize = crate::hidden_prizes::hidden_prize_spawn_packet(2, 900, 1000, 21, 400);
        let (capture_tx, capture_rx) = mpsc::channel();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_new_prize = new_prize.clone();
        let worker = std::thread::spawn(move || {
            capture_rx.recv().expect("join should capture objectives");
            attempted_tx.send(()).expect("signal mutation attempt");
            // This native ordering fixture needs a distinct trusted source,
            // not a client request that steers away from its stored position.
            // Seed its second pose under the same join lock; this test does
            // not claim movement admission (covered by the position suite).
            worker_state
                .lock()
                .expect("state lock")
                .set_client_position(
                    alpha,
                    large_position_from_client(alpha, &client_position_packet(1, 512, 688))
                        .unwrap(),
                );
            relay_drop_brick_at_tick(
                &worker_state,
                alpha,
                &drop_brick_request_payload(512, 688),
                new_brick_tick,
            );
            {
                let mut locked = worker_state.lock().expect("state lock");
                locked.active_hidden_prizes.push(ServerHiddenPrize {
                    collectible_id: 2,
                    x: 900,
                    y: 1000,
                    prize_id: 21,
                    expires_at_tick: crate::continuum::continuum_tick_add(new_brick_tick, 400),
                });
            }
            crate::reliable::broadcast_reliable_to_joined(&worker_state, worker_new_prize);
        });

        join_arena_at_tick_with_objective_capture_hook(&state, beta, 0, entry_tick, || {
            capture_tx.send(()).expect("release objective mutation");
            attempted_rx.recv().expect("mutation should be attempted");
            // The mutation has announced its attempt but cannot acquire
            // ServerState until the join transaction reserves its replay.
            std::thread::sleep(Duration::from_millis(15));
        });
        worker.join().expect("objective mutation should finish");

        let packets = rx_beta
            .try_iter()
            .filter_map(|packet| {
                reliable_parts(&packet).map(|(id, payload)| (id, payload.to_vec()))
            })
            .collect::<Vec<_>>();
        let packet_ids = |expected: &[u8]| {
            packets
                .iter()
                .filter_map(|(id, payload)| (payload == expected).then_some(*id))
                .collect::<Vec<_>>()
        };

        let old_brick_ids = packet_ids(&old_brick);
        let old_prize_ids = packet_ids(&old_prize);
        assert_eq!(old_brick_ids.len(), 1, "old brick must replay exactly once");
        assert_eq!(old_prize_ids.len(), 1, "old prize must replay exactly once");
        assert_eq!(
            u32::from_le_bytes(old_brick[13..17].try_into().unwrap()),
            old_brick_tick,
            "replay must preserve the authoritative brick version"
        );
        assert_eq!(
            u16::from_le_bytes(old_prize[11..13].try_into().unwrap()),
            300,
            "replay TTL must be computed at the join entry tick"
        );

        let new_brick_ids = packets
            .iter()
            .filter_map(|(id, payload)| {
                (payload.len() >= 17
                    && payload[0] == BRICK_DROPPED
                    && u32::from_le_bytes(payload[13..17].try_into().unwrap()) == new_brick_tick)
                    .then_some(*id)
            })
            .collect::<Vec<_>>();
        let new_prize_ids = packet_ids(&new_prize);
        assert_eq!(
            new_brick_ids.len(),
            1,
            "new brick must arrive once by broadcast"
        );
        assert_eq!(
            new_prize_ids.len(),
            1,
            "new prize must arrive once by broadcast"
        );
        let last_replay_id = *old_brick_ids
            .iter()
            .chain(old_prize_ids.iter())
            .max()
            .expect("replay ids");
        let first_later_id = *new_brick_ids
            .iter()
            .chain(new_prize_ids.iter())
            .min()
            .expect("later ids");
        assert!(
            last_replay_id < first_later_id,
            "immutable replay must precede later normal broadcasts"
        );
    }
}

#[test]
fn join_snapshot_selection_starts_only_after_all_bootstrap_acks() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);

    join_arena_at_tick_with_delivery_hooks(
        &state,
        player_id,
        0,
        123_456,
        || {
            {
                let locked = state.lock().expect("state lock");
                let client = locked.clients.get(&player_id).expect("client should exist");
                assert!(client.joined, "input/session join must already be active");
                assert!(!client.snapshot_ready);
                assert_eq!(client.snapshot_seq, 0);
            }
            crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 123_456);
            assert_eq!(
                state.lock().expect("state lock").clients[&player_id].snapshot_seq,
                0,
                "pre-bootstrap session must not be selected for snapshots"
            );
            assert!(
                rx.try_recv().is_err(),
                "no packet may precede bootstrap enqueue"
            );
        },
        || {
            let locked = state.lock().expect("state lock");
            assert!(locked.clients[&player_id].joined);
            assert!(
                !locked.clients[&player_id].snapshot_ready,
                "readiness flips only after enqueue and session validation"
            );
        },
    );

    let (identity, mut ids) = {
        let locked = state.lock().expect("state lock");
        let identity = locked
            .active_player_session_identity(player_id)
            .expect("active session");
        let ids = locked.clients[&player_id]
            .bootstrap_reliable_barrier
            .as_ref()
            .expect("bootstrap barrier")
            .pending_ids();
        (identity, ids)
    };
    assert!(
        ids.len() > 2,
        "bootstrap should reserve multiple reliable ids"
    );
    ids.reverse();
    let final_id = ids.pop().expect("bootstrap id");
    for id in ids {
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }
    assert!(
        !state.lock().expect("state lock").clients[&player_id].snapshot_ready,
        "partial, reordered ACKs must leave the fence closed"
    );
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 123_457);
    assert_eq!(
        state.lock().expect("state lock").clients[&player_id].snapshot_seq,
        0
    );
    crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(final_id));
    assert!(
        state.lock().expect("state lock").clients[&player_id].snapshot_ready,
        "the last exact bootstrap ACK must make the captured session ready"
    );
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 123_457);
    let snapshot = read_non_reliable_from_channel(&rx);
    assert_eq!(snapshot[0], crate::snapshot::SNAPSHOT_PACKET);
    assert_eq!(
        state.lock().expect("state lock").clients[&player_id].snapshot_seq,
        1
    );
}

#[test]
fn join_snapshot_readiness_requires_successful_bootstrap_enqueue() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    drop(rx);

    join_arena_at_tick(&state, player_id, 0, 123_457);

    let (identity, ids) = {
        let locked = state.lock().expect("state lock");
        (
            locked
                .active_player_session_identity(player_id)
                .expect("active session"),
            locked.clients[&player_id]
                .bootstrap_reliable_barrier
                .as_ref()
                .expect("failed delivery keeps its barrier")
                .pending_ids(),
        )
    };
    for id in ids {
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }

    {
        let locked = state.lock().expect("state lock");
        let client = locked.clients.get(&player_id).expect("client should exist");
        assert!(client.joined, "gameplay/session lifecycle remains joined");
        assert!(
            !client.snapshot_ready,
            "failed enqueue cannot enable snapshots"
        );
        assert_eq!(client.snapshot_seq, 0);
    }
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 123_458);
    assert_eq!(
        state.lock().expect("state lock").clients[&player_id].snapshot_seq,
        0
    );
}

#[test]
fn total_bootstrap_loss_waits_for_resend_and_ack_without_snapshot_deadlock() {
    let config = ServerConfig {
        simulated_tx_loss: crate::simulated_loss::SimulatedTxLossSettings::new(1_000_000, Some(17)),
        ..ServerConfig::default()
    };
    let state = Arc::new(Mutex::new(ServerState::new(config)));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena_at_tick(&state, player_id, 0, 345_000);

    assert!(
        rx.try_recv().is_err(),
        "100% loss must suppress the first pass"
    );
    let (identity, expected_ids) = {
        let mut locked = state.lock().expect("state lock");
        let identity = locked
            .active_player_session_identity(player_id)
            .expect("active session");
        let ids = locked.clients[&player_id]
            .bootstrap_reliable_barrier
            .as_ref()
            .expect("lost bootstrap remains fenced")
            .pending_ids();
        assert!(!locked.clients[&player_id].snapshot_ready);
        locked
            .clients
            .get_mut(&player_id)
            .unwrap()
            .simulated_tx_loss = crate::simulated_loss::SimulatedTxLoss::disabled();
        (identity, ids)
    };

    let resent = crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert_eq!(resent, expected_ids.len());
    let resent_ids = rx
        .try_iter()
        .map(|packet| reliable_parts(&packet).expect("reliable resend").0)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        resent_ids,
        expected_ids.iter().copied().collect(),
        "resend must recover every synthetically dropped bootstrap frame"
    );
    for id in expected_ids {
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }
    assert!(state.lock().expect("state lock").clients[&player_id].snapshot_ready);
}

#[test]
fn post_bootstrap_loss_preserves_join_and_file_fences_then_starts_seeded_loss() {
    let config = ServerConfig {
        simulated_tx_loss: crate::simulated_loss::SimulatedTxLossSettings::new(1_000_000, Some(17)),
        simulated_tx_loss_after_bootstrap: true,
        ..ServerConfig::default()
    };
    let state = Arc::new(Mutex::new(ServerState::new(config)));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena_at_tick(&state, player_id, 0, 350_000);

    let bootstrap_packets = rx.try_iter().collect::<Vec<_>>();
    assert!(
        !bootstrap_packets.is_empty(),
        "the deferred loss mode must deliver the complete bootstrap first"
    );
    let identity = state
        .lock()
        .expect("state lock")
        .active_player_session_identity(player_id)
        .expect("active session");
    for packet in bootstrap_packets {
        let (id, _) = reliable_parts(&packet).expect("reliable bootstrap");
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }
    assert!(state.lock().expect("state lock").clients[&player_id].snapshot_ready);

    crate::reliable::send_reliable_to(&state, player_id, vec![0x55]);
    assert_eq!(
        reliable_parts(&rx.try_recv().expect("file bootstrap remains lossless"))
            .expect("reliable test payload")
            .1,
        vec![0x55],
    );
    assert!(
        state
            .lock()
            .expect("state lock")
            .simulated_tx_loss
            .drain_counts()
            == (0, 0),
        "post-bootstrap loss must not sample while zone files are incomplete"
    );
    {
        let mut locked = state.lock().expect("state lock");
        let transfer = locked.clients[&player_id]
            .map_transfer
            .as_ref()
            .expect("joined client owns a map transfer");
        let file_count = transfer.file_count;
        let transfer = locked
            .clients
            .get_mut(&player_id)
            .and_then(|client| client.map_transfer.as_mut())
            .expect("joined client owns a mutable map transfer");
        transfer.next_index = file_count;
        transfer.pending_index = None;
        transfer.active = None;
    }

    crate::reliable::send_reliable_to(&state, player_id, vec![0x56]);
    assert!(
        rx.try_recv().is_err(),
        "the configured 100% loss must begin after every file is acknowledged"
    );
    assert_eq!(
        state
            .lock()
            .expect("state lock")
            .simulated_tx_loss
            .drain_counts(),
        (1, 7),
    );
}

#[test]
fn partial_bootstrap_loss_promotes_only_after_missing_resends_are_acked() {
    let config = ServerConfig {
        simulated_tx_loss: crate::simulated_loss::SimulatedTxLossSettings::new(500_000, Some(7)),
        ..ServerConfig::default()
    };
    let state = Arc::new(Mutex::new(ServerState::new(config)));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena_at_tick(&state, player_id, 0, 456_000);

    let delivered_ids = rx
        .try_iter()
        .map(|packet| reliable_parts(&packet).expect("reliable bootstrap").0)
        .collect::<Vec<_>>();
    let (identity, all_ids) = {
        let mut locked = state.lock().expect("state lock");
        let identity = locked
            .active_player_session_identity(player_id)
            .expect("active session");
        let ids = locked.clients[&player_id]
            .bootstrap_reliable_barrier
            .as_ref()
            .expect("partial bootstrap barrier")
            .pending_ids();
        locked
            .clients
            .get_mut(&player_id)
            .unwrap()
            .simulated_tx_loss = crate::simulated_loss::SimulatedTxLoss::disabled();
        (identity, ids)
    };
    assert!(!delivered_ids.is_empty());
    assert!(
        delivered_ids.len() < all_ids.len(),
        "seeded run must realize loss"
    );
    for &id in &delivered_ids {
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }
    assert!(!state.lock().expect("state lock").clients[&player_id].snapshot_ready);

    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    let missing_ids = rx
        .try_iter()
        .map(|packet| reliable_parts(&packet).expect("reliable resend").0)
        .collect::<Vec<_>>();
    assert_eq!(missing_ids.len(), all_ids.len() - delivered_ids.len());
    for id in missing_ids {
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }
    assert!(state.lock().expect("state lock").clients[&player_id].snapshot_ready);
}

#[test]
fn bootstrap_ack_barrier_handles_reliable_id_wrap_and_reordering() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    state
        .lock()
        .expect("state lock")
        .clients
        .get_mut(&player_id)
        .unwrap()
        .outgoing_reliable
        .set_next_id_for_test(u32::MAX - 2);
    join_arena_at_tick(&state, player_id, 0, 567_000);

    let (identity, mut ids) = {
        let locked = state.lock().expect("state lock");
        (
            locked.active_player_session_identity(player_id).unwrap(),
            locked.clients[&player_id]
                .bootstrap_reliable_barrier
                .as_ref()
                .unwrap()
                .pending_ids(),
        )
    };
    assert!(ids.contains(&u32::MAX));
    assert!(ids.contains(&0));
    ids.reverse();
    let final_id = ids.pop().unwrap();
    for id in ids {
        crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(id));
    }
    assert!(!state.lock().expect("state lock").clients[&player_id].snapshot_ready);
    crate::reliable::receive_reliable_acks(&state, identity, &reliable_ack(final_id));
    assert!(state.lock().expect("state lock").clients[&player_id].snapshot_ready);
}

#[test]
fn stale_bootstrap_acks_cannot_promote_reused_player_id_session() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (old_tx, _old_rx) = mpsc::channel();
    let player_id = register_connection(&state, old_tx);
    join_arena_at_tick(&state, player_id, 0, 678_000);
    let (old_identity, old_ids) = {
        let locked = state.lock().expect("state lock");
        (
            locked.active_player_session_identity(player_id).unwrap(),
            locked.clients[&player_id]
                .bootstrap_reliable_barrier
                .as_ref()
                .unwrap()
                .pending_ids(),
        )
    };
    crate::arena::disconnect_player_session(&state, old_identity);
    state.lock().expect("state lock").next_player_id = player_id;
    let (replacement_tx, _replacement_rx) = mpsc::channel();
    assert_eq!(register_connection(&state, replacement_tx), player_id);
    join_arena_at_tick(&state, player_id, 0, 678_001);

    for id in old_ids {
        crate::reliable::receive_reliable_acks(&state, old_identity, &reliable_ack(id));
    }
    let locked = state.lock().expect("state lock");
    let replacement = &locked.clients[&player_id];
    assert_ne!(replacement.session_id, old_identity.session_id);
    assert!(!replacement.snapshot_ready);
    assert!(replacement.bootstrap_reliable_barrier.is_some());
}

#[test]
fn join_delivery_does_not_retarget_a_reused_joining_session() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_alpha, rx_alpha) = mpsc::channel();
    let alpha = register_connection(&state, tx_alpha);
    join_arena(&state, alpha, 0);
    drain_available(&rx_alpha);
    broadcast_position(&state, alpha, &client_position_packet(0, 512, 640));

    let (tx_beta, rx_beta) = mpsc::channel();
    let beta = register_connection(&state, tx_beta);
    let beta_session = state
        .lock()
        .expect("state lock")
        .active_player_session_identity(beta)
        .expect("beta session");
    let mut replacement_rx = None;
    let mut replacement_session = None;

    join_arena_at_tick_with_delivery_hooks(
        &state,
        beta,
        0,
        234_567,
        || {},
        || {
            crate::arena::disconnect_player_session(&state, beta_session);
            state.lock().expect("state lock").next_player_id = beta;
            let (replacement_tx, rx) = mpsc::channel();
            assert_eq!(register_connection(&state, replacement_tx), beta);
            replacement_session = state
                .lock()
                .expect("state lock")
                .active_player_session_identity(beta);
            replacement_rx = Some(rx);
        },
    );

    let replacement_session = replacement_session.expect("replacement session");
    assert_ne!(replacement_session, beta_session);
    let replacement_rx = replacement_rx.expect("replacement receiver");
    assert!(
        replacement_rx.try_recv().is_err(),
        "replacement must receive neither old bootstrap nor cached positions"
    );
    assert!(
        rx_beta
            .try_iter()
            .all(|packet| reliable_parts(&packet).is_some()),
        "the old session may receive its enqueued bootstrap, but no post-reuse positions"
    );
    let alpha_payloads = rx_alpha
        .try_iter()
        .filter_map(|packet| reliable_parts(&packet).map(|(_, payload)| payload.to_vec()))
        .collect::<Vec<_>>();
    assert_eq!(
        alpha_payloads
            .iter()
            .filter(|payload| payload.first() == Some(&0x03) && player_entering_id(payload) == beta)
            .count(),
        0,
        "existing peers must not receive stale PlayerEntering for the removed joiner"
    );
    assert_eq!(
        alpha_payloads
            .iter()
            .filter(|payload| payload.as_slice() == crate::roster::player_leaving(beta))
            .count(),
        1,
        "the normal disconnect fact remains authoritative"
    );
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 234_568);
    assert!(
        replacement_rx.try_recv().is_err(),
        "replacement must remain snapshot-ineligible for the removed join"
    );
    let locked = state.lock().expect("state lock");
    let replacement = locked.clients.get(&beta).expect("replacement should exist");
    assert_eq!(replacement.session_id, replacement_session.session_id);
    assert!(!replacement.joined);
    assert!(!replacement.snapshot_ready);
}

#[test]
fn join_fanout_does_not_retarget_a_reused_existing_peer() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_alpha, rx_alpha) = mpsc::channel();
    let alpha = register_connection(&state, tx_alpha);
    join_arena(&state, alpha, 0);
    drain_available(&rx_alpha);
    broadcast_position(&state, alpha, &client_position_packet(0, 512, 640));
    let alpha_session = state
        .lock()
        .expect("state lock")
        .active_player_session_identity(alpha)
        .expect("alpha session");

    let (tx_beta, rx_beta) = mpsc::channel();
    let beta = register_connection(&state, tx_beta);
    state
        .lock()
        .expect("state lock")
        .clients
        .get_mut(&beta)
        .expect("beta should exist")
        .player
        .banner[0] = 7;
    let mut replacement_rx = None;
    let mut replacement_session = None;

    join_arena_at_tick_with_delivery_hooks(
        &state,
        beta,
        0,
        345_678,
        || {
            crate::arena::disconnect_player_session(&state, alpha_session);
            state.lock().expect("state lock").next_player_id = alpha;
            let (replacement_tx, rx) = mpsc::channel();
            assert_eq!(register_connection(&state, replacement_tx), alpha);
            replacement_session = state
                .lock()
                .expect("state lock")
                .active_player_session_identity(alpha);
            replacement_rx = Some(rx);
        },
        || {},
    );

    let replacement_session = replacement_session.expect("replacement session");
    assert_ne!(replacement_session, alpha_session);
    let replacement_rx = replacement_rx.expect("replacement receiver");
    assert!(
        rx_alpha.try_recv().is_err(),
        "removed peer must receive no join fanout"
    );
    assert!(
        replacement_rx.try_recv().is_err(),
        "replacement peer must receive no stale PlayerEntering or banner"
    );

    let beta_packets = rx_beta.try_iter().collect::<Vec<_>>();
    let beta_reliables = beta_packets
        .iter()
        .filter_map(|packet| reliable_parts(packet).map(|(id, payload)| (id, payload.to_vec())))
        .collect::<Vec<_>>();
    let assignment_id = beta_reliables
        .iter()
        .find_map(|(id, payload)| (payload.first() == Some(&0x01)).then_some(*id))
        .expect("assignment id");
    let entering_ids = beta_reliables
        .iter()
        .filter_map(|(id, payload)| {
            (payload.first() == Some(&0x03) && player_entering_id(payload) == alpha).then_some(*id)
        })
        .collect::<Vec<_>>();
    let leaving_ids = beta_reliables
        .iter()
        .filter_map(|(id, payload)| {
            (payload.as_slice() == crate::roster::player_leaving(alpha)).then_some(*id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entering_ids.len(),
        1,
        "as-of-entry peer replay must be unique"
    );
    assert_eq!(leaving_ids.len(), 1, "later disconnect fact must be unique");
    assert!(
        assignment_id < entering_ids[0] && entering_ids[0] < leaving_ids[0],
        "reliable ids must order assignment, as-of-entry replay, then later leave"
    );

    join_arena(&state, alpha, 0);
    let replacement_reliables = replacement_rx
        .try_iter()
        .filter_map(|packet| reliable_parts(&packet).map(|(id, payload)| (id, payload.to_vec())))
        .collect::<Vec<_>>();
    let replacement_assignment_id = replacement_reliables
        .iter()
        .find_map(|(id, payload)| (payload.first() == Some(&0x01)).then_some(*id))
        .expect("replacement assignment id");
    let beta_entering_ids = replacement_reliables
        .iter()
        .filter_map(|(id, payload)| {
            (payload.first() == Some(&0x03) && player_entering_id(payload) == beta).then_some(*id)
        })
        .collect::<Vec<_>>();
    let beta_banner_ids = replacement_reliables
        .iter()
        .filter_map(|(id, payload)| {
            (payload.first() == Some(&PLAYER_BANNER_CHANGE)
                && u16::from_le_bytes(payload[1..3].try_into().unwrap()) == beta)
                .then_some(*id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        beta_entering_ids.len(),
        1,
        "replacement's own join gets beta exactly once"
    );
    assert_eq!(
        beta_banner_ids.len(),
        1,
        "replacement's own join gets beta banner exactly once"
    );
    assert!(
        replacement_assignment_id < beta_entering_ids[0]
            && beta_entering_ids[0] < beta_banner_ids[0],
        "replacement reliable ids must order assignment before peer roster and banner"
    );
}

fn player_entering_id(packet: &[u8]) -> u16 {
    assert_eq!(packet[0], 0x03, "expected player entering packet");
    u16::from_le_bytes(packet[51..53].try_into().expect("player id bytes"))
}

fn assert_join_clock_bootstrap(packet: &[u8], player_id: u16) {
    let bootstrap = crate::clock_bootstrap::ServerClockBootstrap::parse(packet)
        .expect("bootstrap should parse");
    assert_ne!(bootstrap.server_session_id, 0);
    assert_eq!(bootstrap.tick_rate_hz, ServerConfig::default().tick_rate_hz);
    assert_eq!(bootstrap.snapshot_rate_hz, crate::snapshot::SNAPSHOT_HZ);
    assert_eq!(
        bootstrap.input_sequence_base,
        bootstrap.position_sequence_base
    );
    assert_eq!(
        bootstrap.input_sequence_base,
        bootstrap.weapon_sequence_base
    );
    assert_eq!(
        bootstrap.input_sequence_base,
        (bootstrap.server_tick as u16)
            .wrapping_mul(31)
            .wrapping_add(player_id)
    );
    assert_eq!(bootstrap.reliable_sequence_base, 0);
}

#[test]
fn server_rejects_safe_zone_damage_and_objective_relays() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let (safe_x, safe_y) = first_safe_zone_position();
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, safe_x, safe_y),
    );
    drain_available(&rx_b);

    relay_flag_request(&state, alpha, &flag_request_payload(3));
    relay_drop_brick(&state, alpha, &drop_brick_request_payload(safe_x, safe_y));
    relay_ball_pickup(&state, alpha, &ball_pickup_request_payload(2, 0x1122_3345));

    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    let alpha_client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(alpha_client.player.flags, 0);
    assert_eq!(alpha_client.last_reported_energy, 0);
    assert!(state.objectives.is_empty());
    assert!(state.active_bricks.is_empty());
}

#[test]
fn server_broadcasts_leave_to_other_joined_players() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let a = register_connection(&state, tx_a);
    let b = register_connection(&state, tx_b);
    join_arena(&state, a, 0);
    join_arena(&state, b, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    leave_arena(&state, a);

    assert_eq!(read_reliable_payload_from_channel(&rx_b), [0x04, 0, 0]);
    assert!(rx_a.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    assert!(state.objectives.is_empty());
    assert_eq!(
        state.clients.get(&a).map(|client| client.player.flags),
        Some(0)
    );
}

#[test]
fn server_disconnect_keeps_disabled_objective_registry_empty_for_late_joiners() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    disconnect_player(&state, alpha);

    assert_eq!(read_reliable_payload_from_channel(&rx_b), [0x04, 0, 0]);
    assert!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .objectives
            .is_empty()
    );

    let (tx_c, rx_c) = mpsc::channel();
    let late = register_connection(&state, tx_c);
    join_arena(&state, late, 0);
    drain_available(&rx_c);
    assert!(current_flag_packets(&state).is_empty());
}

#[test]
fn reused_player_id_receives_a_new_first_life_identity() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (first_tx, _first_rx) = mpsc::channel();
    let player_id = register_connection(&state, first_tx);
    join_arena(&state, player_id, 0);
    let first_identity = state
        .lock()
        .expect("state")
        .active_life_identity(player_id)
        .expect("first life should be active");

    disconnect_player(&state, player_id);
    state.lock().expect("state").next_player_id = player_id;
    let (second_tx, _second_rx) = mpsc::channel();
    let reused_id = register_connection(&state, second_tx);
    assert_eq!(reused_id, player_id);
    join_arena(&state, reused_id, 0);
    let second_identity = state
        .lock()
        .expect("state")
        .active_life_identity(reused_id)
        .expect("replacement life should be active");

    assert_ne!(first_identity, second_identity);
    assert!(second_identity.life_seq > first_identity.life_seq);
}

#[test]
fn life_generation_wraps_without_allocating_zero() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut state = state.lock().expect("state");
    state.clients.get_mut(&player_id).expect("client").life_seq = u32::MAX;
    state.life_seq_watermarks.insert(player_id, u32::MAX);

    let wrapped = state
        .advance_client_life(player_id)
        .expect("life should advance across wrap");

    assert_eq!(wrapped.life_seq, 1);
}

fn decoy_for(owner: crate::authority_identity::PlayerRuntimeIdentity) -> crate::state::ServerDecoy {
    crate::state::ServerDecoy {
        presentation_id: 0,
        presentation_source: None,
        source_input_sequence: None,
        owner,
        source_weapon_data: 6,
        frequency: 0,
        ship: 0,
        position_x: 1_000.0,
        position_y: 1_000.0,
        velocity_x: 1.0,
        velocity_y: 0.0,
        orientation: 0.0,
        created_tick: 10,
        last_step_tick: 10,
        expires_at_tick: 1_000,
    }
}

#[test]
fn life_advance_retires_server_decoys_for_the_previous_runtime() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("state");
    let owner = state
        .active_player_runtime_identity(player_id)
        .expect("joined runtime");
    state.active_decoys.push(decoy_for(owner));

    state
        .advance_client_life(player_id)
        .expect("life should advance");

    assert!(state.active_decoys.is_empty());
}

#[test]
fn damage_death_preserves_decoy_source_and_deadline_but_ship_change_cancels_old_lives() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("state");
    let owner = state.active_player_runtime_identity(player_id).unwrap();
    let decoy = decoy_for(owner);
    state.active_decoys.push(decoy);

    let next = state
        .advance_client_life_after_damage_death(player_id, 100)
        .unwrap();
    assert_ne!(next, owner.life);
    assert_eq!(state.active_decoys, vec![decoy]);
    state
        .advance_client_life_after_damage_death(player_id, 200)
        .unwrap();
    assert_eq!(state.active_decoys, vec![decoy]);

    // An intentional ship/spectator change clears even earlier-life decoys.
    state.advance_client_life_at_tick(player_id, 300).unwrap();
    assert!(state.active_decoys.is_empty());
}

#[test]
fn disconnect_retires_server_decoys_for_the_removed_session() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    {
        let mut state = state.lock().expect("state");
        let owner = state
            .active_player_runtime_identity(player_id)
            .expect("joined runtime");
        state.active_decoys.push(decoy_for(owner));
    }

    disconnect_player(&state, player_id);

    assert!(state.lock().expect("state").active_decoys.is_empty());
}

#[test]
fn life_advance_resets_the_joint_input_position_sequence_stream() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("state");
    let client = state.clients.get_mut(&player_id).expect("client");
    client.last_position_sequence = Some(40_000);
    client.last_input_seq = Some(40_000);
    client.last_input_record = Some(crate::state::ClientInputObservation {
        bits: 1,
        direction: 3,
    });
    client
        .recent_input_records
        .push_back(crate::state::SequencedInputObservation {
            sequence: 40_000,
            input: client.last_input_record.expect("seeded input"),
            door_seed: 0,
            door_phase_tick: 0,
        });

    state
        .advance_client_life(player_id)
        .expect("life should advance");

    let client = state.clients.get(&player_id).expect("client");
    assert_eq!(client.last_position_sequence, None);
    assert_eq!(client.last_input_seq, None);
    assert_eq!(client.last_input_record, None);
    assert!(client.recent_input_records.is_empty());
}

#[test]
fn departure_lifecycle_retains_exact_cutoff_before_player_left_for_leave_and_disconnect() {
    for disconnect in [false, true] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let (peer_tx, peer_rx) = mpsc::channel();
        let owner = register_connection(&state, tx);
        let peer = register_connection(&state, peer_tx);
        join_arena(&state, owner, 0);
        join_arena(&state, peer, 0);
        drain_available(&rx);
        drain_available(&peer_rx);
        let (identity, life, stream) = {
            let mut s = state.lock().unwrap();
            s.next_projectile_group_id = 42;
            let old_owner = s.active_player_runtime_identity(owner).unwrap();
            let peer_owner = s.active_player_runtime_identity(peer).unwrap();
            for owner in [old_owner, peer_owner] {
                s.active_decoys.push(crate::state::ServerDecoy {
                    presentation_id: 0,
                    presentation_source: None,
                    source_input_sequence: None,
                    owner,
                    source_weapon_data: 6,
                    frequency: 0,
                    ship: 0,
                    position_x: 8200.0,
                    position_y: 8200.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    orientation: 0.0,
                    created_tick: 1,
                    last_step_tick: 1,
                    expires_at_tick: 10000,
                });
            }
            s.advance_client_life_after_damage_death(owner, 2).unwrap();
            assert_eq!(
                s.active_decoys.len(),
                2,
                "death must preserve Decoys before intentional departure"
            );
            (
                s.active_player_session_identity(owner).unwrap(),
                s.clients[&owner].life_seq,
                s.clients[&peer].arena_stream_id,
            )
        };
        // A delayed callback from another session may not publish a cutoff.
        crate::arena::disconnect_player_session(
            &state,
            crate::authority_identity::PlayerSessionIdentity::new(
                owner,
                crate::authority_identity::PlayerSessionId::new(identity.session_id.get() + 100),
            ),
        );
        assert!(peer_rx.try_recv().is_err());
        if disconnect {
            crate::arena::disconnect_player_session(&state, identity);
        } else {
            leave_arena(&state, owner);
        }
        {
            let s = state.lock().unwrap();
            assert_eq!(
                s.active_decoys.len(),
                1,
                "departure must immediately remove even earlier-life Decoys"
            );
            assert_eq!(
                s.active_decoys[0].owner.player_id(),
                peer,
                "foreign Decoy must remain"
            );
        }
        let payloads = peer_rx
            .try_iter()
            .filter_map(|packet| {
                crate::reliable::reliable_parts(&packet).map(|(_, payload)| payload.to_vec())
            })
            .collect::<Vec<_>>();
        let (index, event) = payloads
            .iter()
            .enumerate()
            .find_map(|(index, p)| {
                crate::realtime_event::AuthenticatedRealtimeEvent::parse(p)
                    .ok()
                    .map(|e| (index, e))
            })
            .expect("departure must publish a reliable weapon cutoff");
        assert_eq!(event.server_session_id, crate::wire::server_session_id());
        assert_eq!(event.arena_stream_id, stream);
        assert_eq!(
            event.payload,
            crate::realtime_event::RealtimeEventPayload::OwnerWeaponsCancelled(
                crate::realtime_event::RealtimeOwnerWeaponCancellation {
                    source_player_id: owner,
                    source_session_id: identity.session_id.get(),
                    through_shot_group_id: 42,
                    owner_life_seq: life,
                    owner_input_sequence: None,
                }
            )
        );
        let left = payloads
            .iter()
            .position(|p| p == &crate::roster::player_leaving(owner))
            .expect("player-left follows cleanup");
        assert!(index < left);
        crate::reliable::resend_due_reliable_to_clients(
            &state,
            Instant::now() + Duration::from_secs(10),
        );
        let retained = peer_rx.try_iter().collect::<Vec<_>>();
        assert!(
            retained
                .iter()
                .any(|packet| crate::reliable::reliable_parts(packet)
                    .is_some_and(|(_, p)| p == event.encode())),
            "cutoff must survive a lost reliable delivery"
        );
    }
}
