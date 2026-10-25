use super::*;

#[test]
fn server_admin_commands_report_status_players_and_shutdown() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            free_for_all: false,
            bot_count: 1,
            ..ServerConfig::from_env_args(["zubspace_server".to_string()]).unwrap()
        },
        ..team_mode_state()
    }));
    state.lock().expect("state").initialize_configured_bots();
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "123456789012".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 2);

    assert_eq!(parse_admin_command(""), None);
    assert_eq!(parse_admin_command("help"), Some(AdminCommand::Help));
    assert_eq!(parse_admin_command("reset"), Some(AdminCommand::Reset));
    assert_eq!(parse_admin_command("quit"), Some(AdminCommand::Shutdown));
    assert_eq!(
        parse_admin_command("wat"),
        Some(AdminCommand::Unknown("wat".to_string()))
    );
    assert_eq!(
        admin_command_response(&state, &AdminCommand::Help),
        "commands: status, players, reset, shutdown"
    );
    let status = admin_command_response(&state, &AdminCommand::Status);
    assert!(status.contains("zone=chaos-zone clients=1 joined=1"));
    assert!(status.contains("configured_bots=1 active_bots=1"));
    let players = admin_command_response(&state, &AdminCommand::Players);
    assert!(players.contains("#0 pilot ship=2 freq=0 score=0/0"));
    let bot_name = state.lock().unwrap().bots[0].player.name.clone();
    assert!(players.contains(&format!("#768 {bot_name} ship=0 freq=768 score=0/0")));
    assert_eq!(
        admin_command_response(&state, &AdminCommand::Shutdown),
        "shutdown requested"
    );
}

#[test]
fn server_admin_reset_clears_arena_lifecycle_state_without_disconnect() {
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

    let stale_objective_generation = {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .objectives
            .reset(synthetic_profile(1, 1))
            .expect("synthetic objective generation should advance");
        let alpha_runtime = state
            .active_player_runtime_identity(alpha)
            .expect("alpha runtime");
        let stale_flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: alpha_runtime,
                x: 512,
                y: 640,
                owner: None,
            })
            .expect("synthetic flag should fit profile");
        state.turret_links.insert(alpha, beta);
        state.active_bricks.push(ServerBrick {
            tiles: vec![(12, 34)],
            packet: vec![BRICK_DROPPED, 12, 0, 34, 0],
            expires_at_tick: crate::continuum::continuum_tick_add(crate::wire::current_tick(), 100),
        });
        state
            .objectives
            .create_powerball(PowerballState::OnMap {
                x: 16,
                y: 16,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .expect("synthetic powerball should fit profile");
        state.active_hidden_prizes.push(ServerHiddenPrize {
            collectible_id: 0x1122_3344,
            x: 640,
            y: 768,
            prize_id: 15,
            expires_at_tick: crate::continuum::continuum_tick_add(crate::wire::current_tick(), 100),
        });
        state.next_hidden_prize_spawn_at_tick = Some(crate::continuum::continuum_tick_add(
            crate::wire::current_tick(),
            50,
        ));
        let claim = ServerPrizeClaim {
            collectible_id: 0x1122_3344,
            x: 512,
            y: 640,
            prize_id: 15,
        };
        let winner = state
            .active_player_runtime_identity(alpha)
            .expect("alpha runtime");
        state.claimed_prizes.insert(
            claim.collectible_id,
            ServerPrizeReceipt {
                claim,
                winner,
                claimed_at_tick: crate::wire::current_tick(),
                authoritative_energy: 1234,
                prize_state_revision: 1,
                engine_shutdown_until_tick: None,
                engine_shutdown_application_required: false,
            },
        );
        let alpha_client = state.clients.get_mut(&alpha).expect("alpha should exist");
        alpha_client.player.flags = 1;
        alpha_client.alive = false;
        alpha_client.respawn_at_tick = Some(crate::continuum::continuum_tick_add(
            crate::wire::current_tick(),
            crate::arena_settings::arena_respawn_delay_ticks(),
        ));
        alpha_client.last_reported_energy = 100;
        stale_flag.identity.generation
    };

    assert_eq!(
        admin_command_response(&state, &AdminCommand::Reset),
        "arena reset: joined=2 bricks_cleared=1"
    );
    assert_eq!(
        read_reliable_payload_with_type(&rx_a, 0x16),
        drop_flag_packet(alpha)
    );
    assert_eq!(
        read_reliable_payload_with_type(&rx_a, 0x0e),
        create_turret_link(alpha, u16::MAX)
    );
    assert_eq!(
        read_reliable_payload_with_type(&rx_a, BRICK_DROPPED),
        clear_bricks_packet()
    );
    assert_eq!(
        read_reliable_payload_with_type(&rx_a, POWERBALL_POSITION),
        powerball_removal_packet(0)
    );
    let reset_flag = crate::objective_protocol::ObjectiveStateUpdate::parse(
        &read_reliable_payload_with_type(&rx_a, 0x68),
    )
    .unwrap();
    let reset_ball = crate::objective_protocol::ObjectiveStateUpdate::parse(
        &read_reliable_payload_with_type(&rx_a, 0x68),
    )
    .unwrap();
    assert_eq!(
        (reset_flag.kind, reset_ball.kind),
        (
            crate::objective_protocol::ObjectiveStateUpdate::KIND_FLAG,
            crate::objective_protocol::ObjectiveStateUpdate::KIND_POWERBALL,
        )
    );
    assert!(reset_flag.generation > stale_objective_generation);
    assert_eq!(reset_ball.generation, reset_flag.generation);
    drain_available(&rx_b);

    let state = state.lock().expect("server state should not be poisoned");
    let objective_replay = state.objectives.replay();
    assert_eq!(objective_replay.flags.len(), 1);
    assert_eq!(objective_replay.powerballs.len(), 1);
    assert!(matches!(
        objective_replay.flags[0].state,
        FlagState::OnMap { .. }
    ));
    assert!(matches!(
        objective_replay.powerballs[0].state,
        PowerballState::OnMap { .. }
    ));
    assert!(state.turret_links.is_empty());
    assert!(state.active_bricks.is_empty());
    assert!(state.active_hidden_prizes.is_empty());
    assert!(state.next_hidden_prize_spawn_at_tick.is_none());
    assert!(state.claimed_prizes.is_empty());
    let alpha_client = state.clients.get(&alpha).expect("alpha should still exist");
    assert!(alpha_client.joined);
    assert!(alpha_client.alive);
    assert_eq!(alpha_client.player.flags, 0);
    assert!(alpha_client.respawn_at_tick.is_none());
    assert!(alpha_client.last_position.is_none());
    assert_eq!(alpha_client.last_reported_energy, 0);
    assert_eq!(
        alpha_client.server_bounty,
        crate::arena_settings::arena_ship_initial_bounty(0)
    );
    assert_eq!(
        alpha_client.server_energy,
        crate::arena_settings::arena_ship_max_energy(0)
    );
    assert!(alpha_client.server_weapons.proximity_bombs);
}

#[test]
fn admin_reset_threads_one_tick_through_all_players_under_contention_and_wrap() {
    for reset_tick in [456_789, 0x7fff_fff0] {
        let state = Arc::new(Mutex::new(team_mode_state()));
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        let alpha = register_connection(&state, tx_a);
        let beta = register_connection(&state, tx_b);
        join_arena(&state, alpha, 0);
        join_arena(&state, beta, 0);
        drain_available(&rx_a);
        drain_available(&rx_b);

        let stale_tick = crate::continuum::continuum_tick_sub(reset_tick, 75);
        let guard = state.lock().expect("state lock");
        let mut guard = guard;
        for player_id in [alpha, beta] {
            let client = guard
                .clients
                .get_mut(&player_id)
                .expect("player should exist");
            client.life_started_tick = Some(stale_tick);
            client.last_energy_recharge_tick = Some(stale_tick);
            client.last_continuous_energy_drain_tick = Some(stale_tick);
            client.server_super_until_tick = Some(stale_tick);
            client.server_shield_until_tick = Some(stale_tick);
            client.server_emp_until_tick = Some(stale_tick);
            client.server_repel_until_tick = Some(stale_tick);
        }

        let (started_tx, started_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).expect("signal reset worker");
            reset_arena_at_tick(&worker_state, reset_tick)
        });
        started_rx.recv().expect("reset worker should start");
        std::thread::sleep(Duration::from_millis(15));
        drop(guard);
        assert_eq!(
            worker.join().expect("reset worker should finish"),
            "arena reset: joined=2 bricks_cleared=0"
        );

        let locked = state.lock().expect("state lock");
        let alpha_client = locked.clients.get(&alpha).expect("alpha should exist");
        let beta_client = locked.clients.get(&beta).expect("beta should exist");
        for client in [alpha_client, beta_client] {
            assert_eq!(client.life_started_tick, Some(reset_tick));
            assert_eq!(client.last_energy_recharge_tick, Some(reset_tick));
            assert_eq!(client.last_continuous_energy_drain_tick, Some(reset_tick));
            assert_eq!(
                client.server_super_until_tick,
                client.server_super_until_tick.map(|_| {
                    crate::continuum::continuum_tick_add(
                        reset_tick,
                        crate::arena_settings::arena_ship_super_lifetime_ticks(0),
                    )
                })
            );
            assert_eq!(
                client.server_shield_until_tick,
                client.server_shield_until_tick.map(|_| {
                    crate::continuum::continuum_tick_add(
                        reset_tick,
                        crate::arena_settings::arena_ship_shield_lifetime_ticks(0),
                    )
                })
            );
            assert_eq!(client.server_emp_until_tick, None);
            assert_eq!(client.server_repel_until_tick, None);
        }
        assert_eq!(
            alpha_client.server_super_until_tick,
            beta_client.server_super_until_tick
        );
        assert_eq!(
            alpha_client.server_shield_until_tick,
            beta_client.server_shield_until_tick
        );
    }
}

#[test]
fn admin_reset_retires_saturated_old_life_objective_actions_for_new_life_capacity() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    let old_runtime = {
        let mut locked = state.lock().expect("state lock");
        let alpha_runtime = locked.active_player_runtime_identity(alpha).unwrap();
        let beta_runtime = locked.active_player_runtime_identity(beta).unwrap();
        let request = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: locked.clients[&alpha].arena_stream_id,
            player_id: alpha,
            life_seq: alpha_runtime.life.life_seq,
            action_id: 0,
            position_sequence: 0,
            objective_id: 0,
            objective_generation: 1,
            expected_version: 1,
        };
        for action_id in 0..crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
            assert!(locked.queue_objective_action_for_runtime(
                alpha_runtime,
                action_id,
                crate::objective_protocol::ObjectiveActionRequest {
                    action_id,
                    ..request
                },
                10,
            ));
        }
        assert_eq!(
            locked.pending_reliable_gameplay.runtime_len(beta_runtime),
            0
        );
        alpha_runtime
    };

    assert_eq!(
        reset_arena_at_tick(&state, 20),
        "arena reset: joined=2 bricks_cleared=0"
    );

    let mut locked = state.lock().expect("state lock");
    assert!(locked.pending_reliable_gameplay.is_empty());
    let new_runtime = locked.active_player_runtime_identity(alpha).unwrap();
    assert_ne!(new_runtime, old_runtime);
    let request = crate::objective_protocol::ObjectiveActionRequest {
        kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: locked.clients[&alpha].arena_stream_id,
        player_id: alpha,
        life_seq: new_runtime.life.life_seq,
        action_id: 0,
        position_sequence: 0,
        objective_id: 0,
        objective_generation: 1,
        expected_version: 1,
    };
    for action_id in 100..100 + crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
        assert!(locked.queue_objective_action_for_runtime(
            new_runtime,
            action_id,
            crate::objective_protocol::ObjectiveActionRequest {
                action_id,
                ..request
            },
            20,
        ));
    }
    assert!(!locked.queue_objective_action_for_runtime(
        new_runtime,
        999,
        crate::objective_protocol::ObjectiveActionRequest {
            action_id: 999,
            ..request
        },
        20,
    ));
    assert_eq!(
        locked.pending_reliable_gameplay.len(),
        crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME
    );
}

#[test]
fn queued_leave_commits_before_admin_reset_and_same_rtc_rejoins_once() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (alpha_tx, alpha_rx) = mpsc::channel();
    let alpha = register_connection(&state, alpha_tx);
    set_player_identity_and_name(
        &state,
        alpha,
        "123456789012".to_string(),
        "alpha".to_string(),
    );
    join_arena(&state, alpha, 0);
    let (beta_tx, beta_rx) = mpsc::channel();
    let beta = register_connection(&state, beta_tx);
    join_arena(&state, beta, 0);
    acknowledge_join_bootstrap(&state, alpha);
    acknowledge_join_bootstrap(&state, beta);
    drain_available(&alpha_rx);
    drain_available(&beta_rx);
    let (request, old_runtime, old_stream) = {
        let mut locked = state.lock().expect("state lock");
        locked.objectives.reset(synthetic_profile(1, 1)).unwrap();
        let runtime = locked.active_player_runtime_identity(alpha).unwrap();
        let stream = locked.clients[&alpha].arena_stream_id;
        locked
            .objectives
            .create_flag(FlagState::Carried {
                carrier: runtime,
                x: 320,
                y: 320,
                owner: Some(0),
            })
            .unwrap();
        let ball = locked
            .objectives
            .create_powerball(PowerballState::OnMap {
                x: 320,
                y: 320,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .unwrap();
        (
            crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::BallPickup,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: stream,
                player_id: alpha,
                life_seq: runtime.life.life_seq,
                action_id: 55,
                position_sequence: 42,
                objective_id: u16::from(ball.identity.id),
                objective_generation: ball.identity.generation,
                expected_version: ball.version.get(),
            },
            runtime,
            stream,
        )
    };
    let mut rtc_joined = true;
    crate::connection::handle_client_packet(
        &state,
        alpha,
        reliable(0, &request.encode()),
        "admin-leave-reset",
        &mut rtc_joined,
    );
    crate::connection::handle_client_packet(
        &state,
        alpha,
        reliable(1, &[crate::arena::LEAVE_ARENA_REQUEST]),
        "admin-leave-reset",
        &mut rtc_joined,
    );
    assert!(!rtc_joined, "the RTC-local joined gate closes on admission");
    {
        let locked = state.lock().expect("state lock");
        assert!(locked.clients[&alpha].joined);
        assert_eq!(locked.pending_reliable_gameplay.len(), 2);
    }

    assert_eq!(
        reset_arena_at_tick(&state, 700),
        "arena reset: joined=1 bricks_cleared=0"
    );
    {
        let locked = state.lock().expect("state lock");
        assert!(!locked.clients[&alpha].joined);
        assert!(locked.pending_reliable_gameplay.is_empty());
        assert!(!locked.objectives.replay().flags.iter().any(|flag| {
            matches!(flag.state, FlagState::Carried { carrier, .. } if carrier == old_runtime)
        }));
    }
    let first_fanout = beta_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(
        first_fanout
            .iter()
            .filter_map(|packet| reliable_payload(packet))
            .filter(|payload| *payload == crate::roster::player_leaving(alpha).as_slice())
            .count(),
        1,
        "admin reset commits exactly one authoritative Leave fanout"
    );

    crate::connection::handle_client_packet(
        &state,
        alpha,
        reliable(2, &[crate::arena::ARENA_LOGIN, 0]),
        "admin-leave-reset",
        &mut rtc_joined,
    );
    assert!(
        rtc_joined,
        "the same RTC can re-enter after the Leave terminal"
    );
    {
        let locked = state.lock().expect("state lock");
        let client = &locked.clients[&alpha];
        assert!(client.joined && client.alive && client.player.ship == 0);
        assert!(client.arena_stream_id > old_stream);
        assert_ne!(
            locked.active_player_runtime_identity(alpha),
            Some(old_runtime)
        );
        assert!(client.bootstrap_reliable_barrier.is_some());
    }
    crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), 701);
    assert_eq!(
        beta_rx
            .try_iter()
            .filter(|packet| {
                reliable_payload(packet).is_some_and(|payload| {
                    payload == crate::roster::player_leaving(alpha).as_slice()
                })
            })
            .count(),
        0,
        "the retired queued Leave cannot fan out twice"
    );
}

#[test]
fn admin_reset_generation_exhaustion_is_atomic_and_a_later_valid_reset_succeeds() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let before = {
        let mut locked = state.lock().unwrap();
        locked.objectives.reset(synthetic_profile(1, 1)).unwrap();
        locked
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        locked.arena_tick = 77;
        locked.objectives.force_generation(u64::MAX);
        (
            locked.config.default_zone.clone(),
            locked.arena_tick,
            locked.objectives.replay(),
        )
    };

    assert_eq!(
        reset_arena_at_tick(&state, 88),
        "arena reset failed: objective generation space is exhausted"
    );
    {
        let locked = state.lock().unwrap();
        assert_eq!(locked.config.default_zone, before.0);
        assert_eq!(locked.arena_tick, before.1);
        assert_eq!(locked.objectives.replay(), before.2);
    }

    state.lock().unwrap().objectives.force_generation(100);
    assert!(reset_arena_at_tick(&state, 89).starts_with("arena reset:"));
    let locked = state.lock().unwrap();
    assert_eq!(locked.arena_tick, 0);
    assert_eq!(locked.objectives.replay().generation, 101);
}

#[test]
fn pending_leave_recovery_generation_is_reserved_before_admin_reset_mutates_anything() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    acknowledge_join_bootstrap(&state, player_id);
    drain_available(&rx);
    let before = {
        let mut state = state.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let stream = state.clients[&player_id].arena_stream_id;
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: runtime,
                x: 320,
                y: 320,
                owner: Some(0),
            })
            .unwrap();
        state.objectives.force_generation(u64::MAX - 1);
        assert!(
            state
                .objectives
                .force_flag_version(flag.identity.id, crate::continuum::CONTINUUM_TICK_MASK,)
        );
        let flag = state.objectives.flag(flag.identity.id).unwrap();
        let action = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: stream,
            player_id,
            life_seq: runtime.life.life_seq,
            action_id: 77,
            position_sequence: 9,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        };
        assert!(state.queue_reliable_gameplay_for_runtime(
            runtime,
            u32::MAX,
            crate::state::PendingReliableGameplayMutationKind::Objective(
                crate::state::PendingObjectiveAction {
                    identity: runtime,
                    reliable_delivery_id: u32::MAX,
                    request: action,
                    queued_at_tick: 500,
                },
            ),
        ));
        assert!(state.queue_reliable_gameplay_for_runtime(
            runtime,
            0,
            crate::state::PendingReliableGameplayMutationKind::Leave,
        ));
        state.arena_tick = 99;
        (
            runtime,
            state.objectives.replay(),
            state.clients[&player_id].clone(),
            state.pending_reliable_gameplay.len(),
            state.arena_tick,
        )
    };

    assert_eq!(
        reset_arena_at_tick(&state, 501),
        "arena reset failed: objective generation space is exhausted"
    );
    assert!(rx.try_iter().next().is_none(), "failure emits no bytes");
    {
        let state = state.lock().unwrap();
        assert_eq!(state.objectives.replay(), before.1);
        assert_eq!(state.clients[&player_id].joined, before.2.joined);
        assert_eq!(state.clients[&player_id].life_seq, before.2.life_seq);
        assert_eq!(state.pending_reliable_gameplay.len(), before.3);
        assert_eq!(state.arena_tick, before.4);
        assert_eq!(
            state.active_player_runtime_identity(player_id),
            Some(before.0)
        );
    }

    state.lock().unwrap().objectives.force_generation(100);
    assert!(reset_arena_at_tick(&state, 502).starts_with("arena reset:"));
    let state = state.lock().unwrap();
    assert!(!state.clients[&player_id].joined);
    assert!(state.pending_reliable_gameplay.is_empty());
    assert_eq!(state.objectives.replay().generation, 102);
}

#[test]
fn admin_reset_closes_every_admitted_variant_and_reliably_retries_ordered_terminals() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    acknowledge_join_bootstrap(&state, player_id);
    drain_available(&rx);
    let frequency_packet = frequency_change_request(&state, player_id, 1);
    {
        let mut state = state.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let stream = state.clients[&player_id].arena_stream_id;
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position =
            Some(client_position_packet_with_sequence(1, 320, 320, 9));
        let objective = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: stream,
            player_id,
            life_seq: runtime.life.life_seq,
            action_id: 1,
            position_sequence: 9,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        };
        let correction = crate::movement_correction::MovementCorrectionApplied {
            intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq: runtime.life.life_seq,
            correction_generation: 1,
            detected_sequence: 9,
            application_sequence: 10,
        };
        let frequency = crate::state::ReliableFrequencyChange::parse(&frequency_packet).unwrap();
        let variants = [
            crate::state::PendingReliableGameplayMutationKind::Objective(
                crate::state::PendingObjectiveAction {
                    identity: runtime,
                    reliable_delivery_id: 1,
                    request: objective,
                    queued_at_tick: 600,
                },
            ),
            crate::state::PendingReliableGameplayMutationKind::Brick {
                request: crate::state::ReliableBrickRequest { x: 320, y: 320 },
                source: crate::state::BrickSourceBinding::Unavailable,
            },
            crate::state::PendingReliableGameplayMutationKind::Prize(
                crate::state::ReliablePrizeRequest { collectible_id: 9 },
            ),
            crate::state::PendingReliableGameplayMutationKind::MovementCorrection(correction),
            crate::state::PendingReliableGameplayMutationKind::Attach(
                crate::state::ReliableAttachRequest {
                    destination_id: u16::MAX,
                },
            ),
            crate::state::PendingReliableGameplayMutationKind::DropAttached,
            crate::state::PendingReliableGameplayMutationKind::Frequency(frequency),
            crate::state::PendingReliableGameplayMutationKind::Ship(
                crate::state::ReliableShipChange {
                    ship: 1,
                    position_sequence_base: 11,
                },
            ),
            crate::state::PendingReliableGameplayMutationKind::Leave,
        ];
        for (index, kind) in variants.into_iter().enumerate() {
            assert!(state.queue_reliable_gameplay_for_runtime(runtime, index as u32 + 1, kind,));
        }
        let loss =
            crate::simulated_loss::SimulatedTxLossSettings::new(1_000_000, Some(7)).controller();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.simulated_tx_loss = loss;
        client.simulated_tx_loss_after_bootstrap = false;
    }

    assert!(reset_arena_at_tick(&state, 601).starts_with("arena reset:"));
    assert!(
        rx.try_iter().next().is_none(),
        "first terminal send is lost"
    );
    {
        let state = state.lock().unwrap();
        assert!(state.pending_reliable_gameplay.is_empty());
        assert!(
            state.clients[&player_id]
                .pending_reset_application_terminals
                .is_empty()
        );
        assert!(!state.clients[&player_id].joined);
    }
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&player_id)
        .unwrap()
        .simulated_tx_loss = crate::simulated_loss::SimulatedTxLoss::disabled();
    let resend_at = Instant::now() + Duration::from_secs(4);
    crate::reliable::resend_due_reliable_to_clients(&state, resend_at);
    let delivered = rx.try_iter().collect::<Vec<_>>();
    let application_payloads = delivered
        .iter()
        .filter_map(|packet| reliable_payload(packet))
        .filter(|payload| {
            matches!(
                payload.first().copied(),
                Some(crate::objective_protocol::ObjectiveStateUpdate::TYPE)
                    | Some(crate::movement_correction::MovementCorrectionAccepted::TYPE)
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(application_payloads.len(), 2);
    assert_eq!(
        application_payloads[0].last().copied(),
        Some(crate::objective_protocol::ObjectiveStateUpdate::OUTCOME_REJECTED)
    );
    assert_eq!(
        crate::movement_correction::MovementCorrectionAccepted::parse(application_payloads[1])
            .unwrap()
            .outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );

    crate::reliable::resend_due_reliable_to_clients(&state, resend_at + Duration::from_secs(4));
    let duplicate_payloads = rx
        .try_iter()
        .filter_map(|packet| reliable_payload(&packet).map(|payload| payload.to_vec()))
        .filter(|payload| {
            matches!(
                payload.first().copied(),
                Some(crate::objective_protocol::ObjectiveStateUpdate::TYPE)
                    | Some(crate::movement_correction::MovementCorrectionAccepted::TYPE)
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(duplicate_payloads, application_payloads);
}

#[test]
fn admin_reset_terminals_survive_full_reliable_sender_until_capacity_returns() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    acknowledge_join_bootstrap(&state, player_id);
    drain_available(&rx);
    let (session, runtime) = {
        let mut state = state.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let stream = state.clients[&player_id].arena_stream_id;
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position =
            Some(client_position_packet_with_sequence(1, 320, 320, 9));
        let objective = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: stream,
            player_id,
            life_seq: runtime.life.life_seq,
            action_id: 1,
            position_sequence: 9,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        };
        assert!(state.queue_reliable_gameplay_for_runtime(
            runtime,
            1,
            crate::state::PendingReliableGameplayMutationKind::Objective(
                crate::state::PendingObjectiveAction {
                    identity: runtime,
                    reliable_delivery_id: 1,
                    request: objective,
                    queued_at_tick: 700,
                },
            ),
        ));
        assert!(state.queue_reliable_gameplay_for_runtime(
            runtime,
            2,
            crate::state::PendingReliableGameplayMutationKind::MovementCorrection(
                crate::movement_correction::MovementCorrectionApplied {
                    intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
                    server_session_id: crate::wire::server_session_id(),
                    player_id,
                    life_seq: runtime.life.life_seq,
                    correction_generation: 1,
                    detected_sequence: 9,
                    application_sequence: 10,
                },
            ),
        ));
        (runtime.session, runtime)
    };
    for _ in 0..4096 {
        crate::reliable::send_reliable_to(&state, player_id, vec![0x55]);
    }
    drain_available(&rx);
    assert_eq!(
        state.lock().unwrap().clients[&player_id]
            .outgoing_reliable
            .pending_len(),
        4096
    );

    assert!(reset_arena_at_tick(&state, 701).starts_with("arena reset:"));
    {
        let state = state.lock().unwrap();
        assert_ne!(
            state.active_player_runtime_identity(player_id),
            Some(runtime)
        );
        assert_eq!(
            state.clients[&player_id]
                .pending_reset_application_terminals
                .len(),
            2
        );
    }
    let ids = state.lock().unwrap().clients[&player_id]
        .outgoing_reliable
        .pending_ids();
    for (index, id) in ids.into_iter().take(2).enumerate() {
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            session,
            &crate::reliable::reliable_ack(id),
        ));
        crate::reliable::retry_pending_movement_correction_delivery(&state);
        assert_eq!(
            state.lock().unwrap().clients[&player_id]
                .pending_reset_application_terminals
                .len(),
            1 - index
        );
        let payload = read_reliable_payload_from_channel(&rx);
        assert_eq!(
            payload.first().copied(),
            Some(if index == 0 {
                crate::objective_protocol::ObjectiveStateUpdate::TYPE
            } else {
                crate::movement_correction::MovementCorrectionAccepted::TYPE
            })
        );
    }
}
