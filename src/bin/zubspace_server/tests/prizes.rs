use super::*;

fn assert_live_stat(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= f32::EPSILON * expected.abs().max(1.0) * 4.0,
        "expected live stat {expected}, got {actual}",
    );
}

fn replayed_live_stat(
    prize_ids: &[i16],
    stat_prize_id: i16,
    initial: f32,
    step: f32,
    max: f32,
) -> f32 {
    let expanded = prize_ids.iter().flat_map(|&id| {
        let mut ids = vec![id];
        if id == 25 {
            ids.extend(crate::authority_prizes::multiprize_sub_prize_ids(
                &crate::arena_settings::arena_prize_weights(),
                crate::arena_settings::arena_prize_weight_total(),
                crate::arena_settings::arena_multiprize_count(),
                1,
            ));
        }
        ids
    });
    expanded.fold(initial, |current, prize_id| {
        if prize_id.checked_abs() == Some(stat_prize_id) {
            crate::gameplay_rules::stat_value_after_prize(current, initial, step, max, prize_id < 0)
        } else {
            current
        }
    })
}

fn seed_collectible_at_tick(
    state: &Arc<Mutex<ServerState>>,
    collectible_id: u32,
    x: u16,
    y: u16,
    prize_id: i16,
    tick: u32,
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    state.next_collectible_id = state.next_collectible_id.max(collectible_id);
    state.active_hidden_prizes.push(ServerHiddenPrize {
        collectible_id,
        x,
        y,
        prize_id,
        expires_at_tick: crate::continuum::continuum_tick_add(tick, 10_000),
    });
}

fn seed_collectible(
    state: &Arc<Mutex<ServerState>>,
    collectible_id: u32,
    x: u16,
    y: u16,
    prize_id: i16,
) {
    seed_collectible_at_tick(
        state,
        collectible_id,
        x,
        y,
        prize_id,
        crate::wire::current_tick(),
    );
}

#[test]
fn prize_claim_retry_succeeds_after_authoritative_position_catches_up() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);

    seed_collectible_at_tick(&state, 1, 800, 800, 11, 100);
    let request = take_prize_request_payload(1, 800, 800, 11);
    broadcast_position(&state, alpha, &client_position_packet(1, 500, 500));
    drain_available(&rx);
    relay_prize_pickup_at_tick(&state, alpha, &request, 100);
    assert!(rx.try_recv().is_err());
    assert!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .claimed_prizes
            .is_empty()
    );

    broadcast_position(&state, alpha, &client_position_packet(2, 800, 800));
    drain_available(&rx);
    relay_prize_pickup_at_tick(&state, alpha, &request, 101);
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx), 800, 800, 11),
        1
    );
}

#[test]
fn movement_prize_claim_waits_for_its_exact_client_application_boundary() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    reset_server_runtime_without_initial_bounty(&state, player_id);
    broadcast_position(&state, player_id, &client_position_packet(41, 800, 800));
    drain_available(&rx);

    let tick = crate::wire::current_tick();
    let before = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (
            client.server_thrust_acceleration,
            client.server_top_speed,
            client.server_rotation_per_second,
            client.server_recharge_per_second,
        )
    };
    seed_collectible_at_tick(&state, 0x4455, 800, 800, 25, tick);
    relay_prize_pickup_at_tick(
        &state,
        player_id,
        &take_prize_request_payload(0x4455, 800, 800, 25),
        tick,
    );
    let ack = read_reliable_payload_from_channel(&rx);
    assert_eq!(ack.len(), 44);
    assert_eq!(ack[0], 0x61);
    assert_ne!(ack[25] & 1, 0, "movement prize requires application");
    let ack_life_seq = u32::from_le_bytes(ack[13..17].try_into().unwrap());
    let ack_revision = u32::from_le_bytes(ack[17..21].try_into().unwrap());

    let (after, pending) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (
            (
                client.server_thrust_acceleration,
                client.server_top_speed,
                client.server_rotation_per_second,
                client.server_recharge_per_second,
            ),
            client
                .pending_prize_effect_applications
                .front()
                .copied()
                .expect("movement boundary retained"),
        )
    };
    assert_ne!(before, after);
    assert_eq!(
        pending.movement_tuning_before_bits,
        Some([
            before.0.to_bits(),
            before.1.to_bits(),
            before.2.to_bits(),
            before.3.to_bits(),
        ])
    );
    {
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.clients[&player_id].movement_tuning_values_for_replay(),
            before,
            "pre-application inputs retain predecessor tuning"
        );
    }

    let application = crate::prize_application::PrizeEffectApplied {
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq: ack_life_seq,
        collectible_id: 0x4455,
        prize_id: 25,
        prize_state_revision: ack_revision,
        application_sequence: pending
            .minimum_application_sequence
            .expect("exact successor"),
    };
    let accepted =
        crate::player_events::apply_prize_effect_application(&state, player_id, application)
            .expect("exact application terminal");
    assert_eq!(
        accepted.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED
    );
    let locked = state.lock().expect("state");
    assert!(
        locked.clients[&player_id]
            .pending_prize_effect_applications
            .is_empty()
    );
    assert_eq!(
        locked.clients[&player_id].movement_tuning_values_for_replay(),
        after,
        "post-application inputs switch atomically to the new tuning"
    );
}

#[test]
fn accepted_warp_prize_rebases_only_when_exact_spawn_report_arrives() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(1, 800, 800));
    drain_available(&rx);

    let claim_tick = crate::wire::current_tick();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_sim_state = Some(crate::state::ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.25,
            velocity_x: 100.0,
            velocity_y: -50.0,
            energy: 1_000.0,
            max_energy: 1_700.0,
        });
        client.server_sim_anchor_tick = Some(crate::continuum::continuum_tick_add(
            claim_tick,
            crate::continuum::CONTINUUM_TICK_MASK,
        ));
        client.server_sim_input_bits = Some(3);
        client.movement_drift_x = 10.0;
        client.movement_drift_y = -8.0;
    }

    seed_collectible_at_tick(&state, 1, 800, 800, 7, claim_tick);
    let request = take_prize_request_payload(1, 800, 800, 7);
    relay_prize_pickup_at_tick(&state, alpha, &request, claim_tick);
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx), 800, 800, 7),
        1
    );

    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert!(client.server_sim_state.is_some());
        assert_eq!(
            client.server_sim_anchor_tick,
            Some(crate::continuum::continuum_tick_add(
                claim_tick,
                crate::continuum::CONTINUUM_TICK_MASK,
            ))
        );
        assert_eq!(client.server_sim_input_bits, Some(3));
        assert_eq!(
            client.pending_warp_prize_relocations.front().copied(),
            Some(crate::state::PendingWarpPrizeRelocation {
                collectible_id: 1,
                life_seq: client.life_seq,
                expected_energy: client.server_energy,
                expected_bounty: client.server_bounty,
                prize_state_revision: client.server_prize_state_revision,
                claim: crate::state::ServerPrizeClaim {
                    collectible_id: 1,
                    x: 800,
                    y: 800,
                    prize_id: 7,
                },
                pickup_ack_sent: true,
            })
        );
    }

    // Realtime positions already in flight when the reliable pickup ACK is
    // sent must not consume the one-shot Warp authorization.
    broadcast_position(&state, alpha, &client_position_packet(2, 801, 800));
    drain_available(&rx);
    assert!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .pending_warp_prize_relocations
            .is_empty()
            == false
    );

    let (spawn_x, spawn_y, spawn_status, spawn_bounty, spawn_energy) = {
        let state = state.lock().expect("server state should not be poisoned");
        let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
        let in_safe = crate::zone::world_position_in_safe_tile_for_map(state.zone_map(), x, y);
        let client = state.clients.get(&alpha).expect("alpha should exist");
        let status = client.server_status.position_status_bits(0, in_safe).bits();
        (
            x,
            y,
            status,
            client.server_bounty,
            client
                .pending_warp_prize_relocations
                .front()
                .copied()
                .expect("Warp authorization should remain")
                .expected_energy,
        )
    };
    let mut warp_position = client_position_packet(3, spawn_x, spawn_y);
    warp_position[11] = spawn_status;
    warp_position[16..18].copy_from_slice(&spawn_bounty.to_le_bytes());
    warp_position[18..20].copy_from_slice(&spawn_energy.to_le_bytes());
    let warp_position = append_warp_collectible_marker(warp_position, 1);
    {
        let snapshot = crate::continuum_position::parse_client_position_snapshot(&warp_position)
            .expect("Warp position should parse");
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(snapshot.timestamp, 0);
        assert_eq!(snapshot.direction, 0);
        assert_eq!((snapshot.velocity_x, snapshot.velocity_y), (0, 0));
        assert_eq!(snapshot.weapon, 0);
        assert_eq!(snapshot.repel_snapshot_id, None);
        assert_eq!(snapshot.bounty, client.server_bounty);
        assert_eq!(snapshot.togglables, spawn_status);
        assert_eq!(client.pending_warp_prize_relocations.len(), 1);
    }
    broadcast_position(&state, alpha, &warp_position);
    drain_available(&rx);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert!(client.pending_warp_prize_relocations.is_empty());
    assert!(client.server_sim_state.is_some());
    assert!(client.server_sim_anchor_tick.is_some());
    assert_eq!(client.movement_drift_x, 0.0);
    assert_eq!(client.movement_drift_y, 0.0);
    assert_eq!(
        client
            .last_position
            .as_deref()
            .and_then(crate::arena::large_position_coordinates),
        Some((spawn_x, spawn_y))
    );
}

#[test]
fn warp_authorization_is_positive_winner_scoped_idempotent_and_ordered() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(&state, alpha, &client_position_packet(1, 800, 800));
    broadcast_position(&state, beta, &client_position_packet(1, 1_600, 1_600));
    drain_available(&rx_a);
    drain_available(&rx_b);

    let base_tick = crate::wire::current_tick();
    seed_collectible_at_tick(&state, 1, 800, 800, -7, base_tick);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(1, 800, 800, -7),
        base_tick,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 800, 800, -7),
        1
    );
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .is_empty(),
        "a negative Warp effect never authorizes relocation"
    );

    // The request's advisory coordinates/prize id cannot manufacture Warp:
    // the hidden collectible selected by its server id owns the claim.
    seed_collectible_at_tick(&state, 2, 800, 800, 11, base_tick + 1);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(2, 42, 43, 7),
        base_tick + 1,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 800, 800, 11),
        2
    );
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .is_empty(),
        "a forged advisory prize id cannot authorize Warp"
    );

    seed_collectible_at_tick(&state, 3, 800, 800, 7, base_tick + 2);
    let first_warp_request = take_prize_request_payload(3, 800, 800, 7);
    relay_prize_pickup_at_tick(&state, alpha, &first_warp_request, base_tick + 2);
    let first_ack = read_reliable_payload_from_channel(&rx_a);
    assert_eq!(assert_prize_ack(first_ack.clone(), 800, 800, 7), 3);
    let first_authorization = state.lock().expect("state").clients[&alpha]
        .pending_warp_prize_relocations
        .front()
        .copied()
        .expect("positive canonical Warp claim should authorize");
    assert_eq!(first_authorization.collectible_id, 3);
    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&alpha)
        .unwrap()
        .server_energy = first_authorization.expected_energy.saturating_add(7);

    relay_prize_pickup_at_tick(&state, alpha, &first_warp_request, base_tick + 100);
    let duplicate_ack = read_reliable_payload_from_channel(&rx_a);
    assert_eq!(assert_prize_ack(duplicate_ack.clone(), 800, 800, 7), 3);
    assert_eq!(
        duplicate_ack, first_ack,
        "an idempotent retry must echo the original exact energy even after recharge advances"
    );
    assert_eq!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![first_authorization],
        "a duplicate receipt is idempotent and cannot add authority"
    );

    // The receipt is bound to the winner's session/life. Another player
    // cannot replay the collectible id to gain either an ACK or authority.
    drain_available(&rx_b);
    relay_prize_pickup_at_tick(&state, beta, &first_warp_request, base_tick + 101);
    assert!(rx_b.try_recv().is_err());
    assert!(
        state.lock().expect("state").clients[&beta]
            .pending_warp_prize_relocations
            .is_empty()
    );

    seed_collectible_at_tick(&state, 4, 800, 800, 7, base_tick + 102);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(4, 800, 800, 7),
        base_tick + 102,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 800, 800, 7),
        4
    );
    let second_authorization = state.lock().expect("state").clients[&alpha]
        .pending_warp_prize_relocations
        .back()
        .copied()
        .expect("second Warp should append");
    assert_eq!(second_authorization.collectible_id, 4);
    assert_ne!(second_authorization, first_authorization);
    assert_eq!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .len(),
        2,
        "a newer ordered ACK cannot strand or supersede an older delayed ACK"
    );

    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&alpha)
        .expect("alpha")
        .clear_life_scoped_observations();
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .is_empty(),
        "life/zone runtime reset clears every ordered authorization"
    );
}

fn exact_warp_position(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    sequence: u16,
    authorization: crate::state::PendingWarpPrizeRelocation,
) -> Vec<u8> {
    let (spawn_x, spawn_y, status) = {
        let state = state.lock().expect("state");
        let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
        let in_safe = crate::zone::world_position_in_safe_tile_for_map(state.zone_map(), x, y);
        let status = state.clients[&player_id]
            .server_status
            .position_status_bits(0, in_safe)
            .bits();
        (x, y, status)
    };
    let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
    packet[11] = status;
    packet[16..18].copy_from_slice(&authorization.expected_bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&authorization.expected_energy.to_le_bytes());
    append_warp_collectible_marker(packet, authorization.collectible_id)
}

fn record_warp_position_input(state: &Arc<Mutex<ServerState>>, player_id: u16, sequence: u16) {
    let mut input_packet = vec![
        crate::connection::CLIENT_INPUT_RECORD_TYPE,
        sequence as u8,
        (sequence >> 8) as u8,
        1,
    ];
    input_packet.extend_from_slice(&0_u16.to_le_bytes());
    input_packet.extend_from_slice(&[0, 0]);
    input_packet.extend_from_slice(&unavailable_combat_view_receipt(sequence));
    let mut joined = true;
    crate::connection::handle_client_packet(
        state,
        player_id,
        input_packet,
        "warp-ack-delay-test",
        &mut joined,
    );
}

#[test]
fn delayed_warp_ack_survives_three_retries_receipt_eviction_and_consumes_exactly_once() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(0, 800, 800));
    drain_available(&rx);

    let claim_tick = crate::wire::current_tick();
    seed_collectible_at_tick(&state, 50, 800, 800, 7, claim_tick);
    let request = take_prize_request_payload(50, 800, 800, 7);
    relay_prize_pickup_at_tick(&state, alpha, &request, claim_tick);
    let first_ack = read_reliable_payload_from_channel(&rx);
    for retry in 1..=3 {
        relay_prize_pickup_at_tick(
            &state,
            alpha,
            &request,
            crate::continuum::continuum_tick_add(claim_tick, retry * 300),
        );
        assert_eq!(
            read_reliable_payload_from_channel(&rx),
            first_ack,
            "retry {retry} echoes the same receipt"
        );
    }

    let delayed_tick = crate::continuum::continuum_tick_add(
        claim_tick,
        crate::arena_settings::arena_prize_claim_retention_ticks() + 601,
    );
    let (identity, authorization, sequence) = {
        let mut locked = state.lock().expect("state");
        locked.expire_prize_claims_older_than(
            delayed_tick,
            crate::arena_settings::arena_prize_claim_retention_ticks(),
        );
        assert!(!locked.claimed_prizes.contains_key(&50));
        let client = &locked.clients[&alpha];
        assert_eq!(client.pending_warp_prize_relocations.len(), 1);
        (
            locked.active_life_identity(alpha).expect("life"),
            client.pending_warp_prize_relocations[0],
            client.last_position_sequence.unwrap().wrapping_add(1),
        )
    };
    let packet = exact_warp_position(&state, alpha, sequence, authorization);
    record_warp_position_input(&state, alpha, sequence);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            delayed_tick,
        )
        .is_some()
    );
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .is_empty()
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            crate::continuum::continuum_tick_add(delayed_tick, 1),
        )
        .is_none()
    );
}

#[test]
fn ordered_warp_acks_cross_reliable_wrap_and_latest_retires_missing_intermediate() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    acknowledge_join_bootstrap(&state, alpha);
    drain_available(&rx);
    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&alpha)
        .unwrap()
        .outgoing_reliable
        .set_next_id_for_test(u32::MAX);
    broadcast_position(&state, alpha, &client_position_packet(0, 800, 800));
    drain_available(&rx);

    let tick = crate::wire::current_tick();
    for collectible_id in [70, 71, 72] {
        seed_collectible_at_tick(&state, collectible_id, 800, 800, 7, tick);
        relay_prize_pickup_at_tick(
            &state,
            alpha,
            &take_prize_request_payload(collectible_id, 800, 800, 7),
            tick,
        );
        let packet = rx.recv_timeout(Duration::from_secs(1)).expect("Warp ACK");
        let (reliable_id, payload) = reliable_parts(&packet).expect("reliable wrapper");
        assert_eq!(
            reliable_id,
            match collectible_id {
                70 => u32::MAX,
                71 => 0,
                _ => 1,
            }
        );
        assert_eq!(
            assert_prize_ack(payload.to_vec(), 800, 800, 7),
            collectible_id
        );
    }

    let (identity, authorizations, first_sequence) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&alpha];
        (
            locked.active_life_identity(alpha).expect("life"),
            client
                .pending_warp_prize_relocations
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            client.last_position_sequence.unwrap().wrapping_add(1),
        )
    };
    assert_eq!(authorizations.len(), 3);

    // The browser applies the ordered ACK prefix before realtime emission.
    // When several Warp prizes land in one frame, the first publishable
    // relocation therefore carries the current state after the newest ACK;
    // accepting a stale intermediate bounty would contradict server-owned
    // prize state. Matching the newest authorization retires the coalesced
    // prefix in one exact relocation.
    let latest = exact_warp_position(&state, alpha, first_sequence, authorizations[2]);
    record_warp_position_input(&state, alpha, first_sequence);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &latest, tick)
            .is_some()
    );
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .is_empty()
    );
}

#[test]
fn warp_queue_bound_rejects_but_explicit_identity_supports_saturated_bounty() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(0, 800, 800));
    drain_available(&rx);
    let tick = crate::wire::current_tick();

    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&alpha).expect("client");
        for index in 0..crate::state::MAX_PENDING_WARP_PRIZE_RELOCATIONS {
            client.pending_warp_prize_relocations.push_back(
                crate::state::PendingWarpPrizeRelocation {
                    collectible_id: index as u32 + 1,
                    life_seq: client.life_seq,
                    expected_energy: client.server_energy,
                    expected_bounty: index as u16,
                    prize_state_revision: client.server_prize_state_revision,
                    claim: crate::state::ServerPrizeClaim {
                        collectible_id: index as u32 + 1,
                        x: 800,
                        y: 800,
                        prize_id: 7,
                    },
                    pickup_ack_sent: true,
                },
            );
        }
    }
    seed_collectible_at_tick(&state, 90, 800, 800, 7, tick);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(90, 800, 800, 7),
        tick,
    );
    assert!(rx.try_recv().is_err());
    {
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.clients[&alpha].pending_warp_prize_relocations.len(),
            crate::state::MAX_PENDING_WARP_PRIZE_RELOCATIONS
        );
        assert!(
            locked
                .active_hidden_prizes
                .iter()
                .any(|prize| prize.collectible_id == 90)
        );
    }

    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&alpha).expect("client");
        client.pending_warp_prize_relocations.clear();
        client.server_bounty = u16::MAX;
        client
            .pending_warp_prize_relocations
            .push_back(crate::state::PendingWarpPrizeRelocation {
                collectible_id: 90,
                life_seq: client.life_seq,
                expected_energy: client.server_energy,
                expected_bounty: u16::MAX,
                prize_state_revision: client.server_prize_state_revision,
                claim: crate::state::ServerPrizeClaim {
                    collectible_id: 90,
                    x: 800,
                    y: 800,
                    prize_id: 7,
                },
                pickup_ack_sent: true,
            });
    }
    seed_collectible_at_tick(&state, 91, 800, 800, 7, tick);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(91, 800, 800, 7),
        tick,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx), 800, 800, 7),
        91,
        "the collectible id, not a bounty delta, disambiguates saturated Warp claims",
    );
    let (identity, sequence, authorizations) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&alpha];
        (
            locked.active_life_identity(alpha).expect("active life"),
            client
                .last_position_sequence
                .expect("initial position sequence")
                .wrapping_add(1),
            client
                .pending_warp_prize_relocations
                .iter()
                .copied()
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        authorizations
            .iter()
            .map(|authorization| authorization.collectible_id)
            .collect::<Vec<_>>(),
        vec![90, 91],
    );
    assert!(
        !state
            .lock()
            .expect("state")
            .active_hidden_prizes
            .iter()
            .any(|prize| prize.collectible_id == 91)
    );

    let latest = exact_warp_position(&state, alpha, sequence, authorizations[1]);
    record_warp_position_input(&state, alpha, sequence);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &latest, tick + 1,)
            .is_some(),
        "the explicit newer marker consumes the ordered saturated-bounty prefix",
    );
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_warp_prize_relocations
            .is_empty()
    );
}

#[test]
fn prize_request_tick_is_stable_across_mutex_delay_at_normal_and_wrapped_ticks() {
    for request_tick in [234_567, 0x7fff_fffe] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        let alpha = register_connection(&state, tx_a);
        let beta = register_connection(&state, tx_b);
        join_arena(&state, alpha, 0);
        join_arena(&state, beta, 0);
        drain_available(&rx_a);
        drain_available(&rx_b);
        broadcast_position(&state, alpha, &client_position_packet(0, 512, 640));
        drain_available(&rx_a);
        drain_available(&rx_b);

        seed_collectible_at_tick(&state, 1, 512, 640, 17, request_tick);
        let request = take_prize_request_payload(1, 512, 640, 17);
        let guard = state.lock().expect("state lock");
        let delayed_state = Arc::clone(&state);
        let (started_tx, started_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).expect("start signal");
            relay_prize_pickup_at_tick(&delayed_state, alpha, &request, request_tick);
        });
        started_rx.recv().expect("worker should start");
        std::thread::sleep(Duration::from_millis(15));
        drop(guard);
        worker.join().expect("prize worker should finish");

        assert_eq!(
            assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 512, 640, 17,),
            1
        );
        assert_player_prize(
            read_reliable_payload_from_channel(&rx_b),
            alpha,
            1,
            512,
            640,
            17,
        );

        let locked = state.lock().expect("state lock");
        assert_eq!(
            locked
                .claimed_prizes
                .get(&1)
                .map(|receipt| receipt.claimed_at_tick),
            Some(request_tick),
            "claim version and retention anchor must share the entry tick"
        );
        let max_ticks = crate::arena_settings::arena_ship_super_lifetime_ticks(0);
        let mut expected_seed = crate::authority_prizes::SECURITY_PRIZE_SEED;
        let expected_duration = crate::vie::vie_rng_next(&mut expected_seed) % max_ticks;
        assert_eq!(
            locked
                .clients
                .get(&alpha)
                .and_then(|client| client.server_super_until_tick),
            Some(crate::continuum::continuum_tick_add(
                request_tick,
                expected_duration,
            )),
            "timed prize deadline must share the entry tick"
        );
    }
}

#[test]
fn server_relays_prize_pickups_to_other_joined_players() {
    let state = Arc::new(Mutex::new(ServerState::default()));
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
        &client_position_packet(0x1122_3300, 512, 640),
    );
    broadcast_position(&state, beta, &client_position_packet(0x1122_3300, 512, 640));
    drain_available(&rx_a);
    drain_available(&rx_b);

    seed_collectible(&state, 0x1122_3344, 512, 640, 15);
    let request = take_prize_request_payload(0x1122_3344, 512, 640, 15);
    relay_prize_pickup(&state, alpha, &request);

    let first_timestamp = assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 512, 640, 15);
    assert_eq!(first_timestamp, 0x1122_3344);
    assert_player_prize(
        read_reliable_payload_from_channel(&rx_b),
        alpha,
        first_timestamp,
        512,
        640,
        15,
    );

    // Model several 0x07 retry timers firing before the browser processes any
    // of the corresponding ACKs. Every response has the same authoritative
    // identity; client-side ACK history applies only the first.
    for _ in 0..3 {
        relay_prize_pickup(&state, alpha, &request);
    }
    for _ in 0..3 {
        assert_eq!(
            assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 512, 640, 15),
            0x1122_3344
        );
    }
    assert!(
        rx_b.try_recv().is_err(),
        "winner retry must not rebroadcast"
    );

    relay_prize_pickup(&state, beta, &request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .claimed_prizes
            .len(),
        1
    );

    let second_request = take_prize_request_payload(0x1122_3345, 512, 640, 15);
    relay_prize_pickup(&state, beta, &second_request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    let forged_different_prize = take_prize_request_payload(0x1122_3346, 512, 640, 16);
    relay_prize_pickup(&state, beta, &forged_different_prize);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .claimed_prizes
            .len(),
        1
    );

    seed_collectible(&state, 0x1122_3345, 524, 640, 15);
    let nearby_request = take_prize_request_payload(0x1122_3345, 524, 640, 15);
    relay_prize_pickup(&state, beta, &nearby_request);
    let nearby_prize = read_reliable_payload_from_channel(&rx_a);
    let nearby_timestamp = u32::from_le_bytes(nearby_prize[1..5].try_into().unwrap());
    assert_player_prize(nearby_prize, beta, nearby_timestamp, 524, 640, 15);
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx_b), 524, 640, 15),
        nearby_timestamp
    );

    for invalid_prize_id in [29, -29, i16::MIN] {
        let invalid_request = take_prize_request_payload(0x1122_3347, 500, 640, invalid_prize_id);
        relay_prize_pickup(&state, beta, &invalid_request);
        assert!(rx_a.try_recv().is_err());
        assert!(rx_b.try_recv().is_err());
    }
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .claimed_prizes
            .len(),
        2
    );

    let far_request = take_prize_request_payload(0x1122_3346, 5000, 640, 15);
    relay_prize_pickup(&state, beta, &far_request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        for receipt in state.claimed_prizes.values_mut() {
            receipt.claimed_at_tick = crate::continuum::continuum_tick_sub(
                crate::wire::current_tick(),
                crate::arena_settings::arena_prize_claim_retention_ticks() * 2,
            );
        }
    }
    relay_prize_pickup(&state, beta, &request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    relay_prize_pickup(&state, alpha, &[TAKE_PRIZE_REQUEST, 1, 2, 3]);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    update_ship(&state, alpha, &ship_change_request(8));
    drain_available(&rx_a);
    drain_available(&rx_b);
    relay_prize_pickup(&state, alpha, &request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    update_ship(&state, alpha, &ship_change_request(0));
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3301, 512, 640),
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
    relay_prize_pickup(&state, alpha, &request);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn server_rejects_zero_and_unknown_collectible_ids() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    broadcast_position(
        &state,
        alpha,
        &client_position_packet(0x1122_3300, 512, 640),
    );
    drain_available(&rx_a);

    relay_prize_pickup(&state, alpha, &take_prize_request_payload(0, 512, 640, 15));
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3344, 512, 640, 15),
    );

    assert!(rx_a.try_recv().is_err());
    assert!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .claimed_prizes
            .is_empty()
    );
}

#[test]
fn server_canonicalizes_hidden_prize_claims_before_relaying() {
    let state = Arc::new(Mutex::new(ServerState::default()));
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
        &client_position_packet(0x1122_3300, 512, 640),
    );
    drain_available(&rx_b);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.active_hidden_prizes.push(ServerHiddenPrize {
            collectible_id: 0x1122_3344,
            x: 512,
            y: 640,
            prize_id: 21,
            expires_at_tick: crate::continuum::continuum_tick_add(
                crate::wire::current_tick(),
                1_000,
            ),
        });
    }

    let forged_request = take_prize_request_payload(0x1122_3344, 512, 640, 16);
    relay_prize_pickup(&state, alpha, &forged_request);

    let hidden_timestamp =
        assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 512, 640, 21);
    assert_player_prize(
        read_reliable_payload_from_channel(&rx_b),
        alpha,
        hidden_timestamp,
        512,
        640,
        21,
    );
    let state = state.lock().expect("server state should not be poisoned");
    assert!(state.active_hidden_prizes.is_empty());
    assert!(
        state
            .claimed_prizes
            .values()
            .any(|receipt| receipt.claim.prize_id == 21)
    );
}

#[test]
fn server_applies_shared_prize_to_exact_teammates_without_aoi_or_client_authority() {
    let tick = 4_000;
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let (tx_d, rx_d) = mpsc::channel();
    let (tx_e, rx_e) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let dead_teammate = register_connection(&state, tx_c);
    let spectator_teammate = register_connection(&state, tx_d);
    let other_frequency = register_connection(&state, tx_e);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, dead_teammate, 0);
    join_arena(&state, spectator_teammate, 0);
    join_arena(&state, other_frequency, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    drain_available(&rx_d);
    drain_available(&rx_e);
    broadcast_position(&state, alpha, &client_position_packet(1, 512, 640));
    broadcast_position(&state, beta, &client_position_packet(1, 30_000, 30_000));
    drain_available(&rx_a);
    drain_available(&rx_b);
    let beta_revision_before;
    let excluded_revisions_before;
    {
        let mut guard = state.lock().expect("state");
        let alpha_frequency = guard.clients[&alpha].player.frequency;
        let beta_client = guard.clients.get_mut(&beta).expect("beta");
        beta_client.player.frequency = alpha_frequency;
        beta_client.server_bounty = 0;
        beta_revision_before = beta_client.server_prize_state_revision;
        let dead = guard
            .clients
            .get_mut(&dead_teammate)
            .expect("dead teammate");
        dead.player.frequency = alpha_frequency;
        dead.alive = false;
        let spectator = guard
            .clients
            .get_mut(&spectator_teammate)
            .expect("spectator teammate");
        spectator.player.frequency = alpha_frequency;
        spectator.player.ship = 8;
        let outsider = guard
            .clients
            .get_mut(&other_frequency)
            .expect("other frequency");
        outsider.player.frequency = alpha_frequency.wrapping_add(1);
        excluded_revisions_before = [
            guard.clients[&dead_teammate].server_prize_state_revision,
            guard.clients[&spectator_teammate].server_prize_state_revision,
            guard.clients[&other_frequency].server_prize_state_revision,
        ];
    }

    seed_collectible_at_tick(&state, 51, 512, 640, 18, tick);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(51, 512, 640, 18),
        tick,
    );
    let guard = state.lock().expect("state");
    let alpha_client = &guard.clients[&alpha];
    let beta_client = &guard.clients[&beta];
    let expected_deadline = crate::continuum::continuum_tick_add(
        tick,
        crate::arena_settings::arena_ship_shield_lifetime_ticks(beta_client.player.ship),
    );
    assert_eq!(
        alpha_client.server_shield_until_tick,
        Some(expected_deadline)
    );
    assert_eq!(
        beta_client.server_shield_until_tick,
        Some(expected_deadline)
    );
    assert_eq!(beta_client.server_bounty, 1);
    assert_ne!(
        beta_client.server_prize_state_revision,
        beta_revision_before
    );
    assert_eq!(
        [
            guard.clients[&dead_teammate].server_prize_state_revision,
            guard.clients[&spectator_teammate].server_prize_state_revision,
            guard.clients[&other_frequency].server_prize_state_revision,
        ],
        excluded_revisions_before,
        "dead, spectating, and other-frequency clients are never recipients"
    );
    assert_eq!(guard.clients[&dead_teammate].server_shield_until_tick, None);
    assert_eq!(
        guard.clients[&spectator_teammate].server_shield_until_tick,
        None
    );
    assert_eq!(
        guard.clients[&other_frequency].server_shield_until_tick,
        None
    );
    drop(guard);

    // PrizeShareLimit is checked before applying each public collection. At
    // the exact cap the remote teammate receives neither mutation nor a
    // private revision, while the collector remains authoritative.
    let beta_revision_at_cap = {
        let mut guard = state.lock().expect("state");
        let beta_client = guard.clients.get_mut(&beta).expect("beta");
        beta_client.server_bounty =
            crate::arena_settings::arena_ship_prize_share_limit(beta_client.player.ship);
        beta_client.server_prize_state_revision
    };
    seed_collectible_at_tick(&state, 52, 512, 640, 1, tick + 1);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(52, 512, 640, 1),
        tick + 1,
    );
    let guard = state.lock().expect("state");
    assert_eq!(
        guard.clients[&beta].server_prize_state_revision,
        beta_revision_at_cap
    );
    drop(guard);

    // Numeric ids are reusable, but sharing is calculated from the current
    // session roster under the claim lock. A replacement session at beta's
    // old id is eligible from its own fresh life/state and cannot inherit or
    // suppress the removed teammate's revision.
    disconnect_player(&state, beta);
    let (replacement_tx, replacement_rx) = mpsc::channel();
    state.lock().expect("state").next_player_id = beta;
    let replacement = register_connection(&state, replacement_tx);
    assert_eq!(replacement, beta, "test must exercise player-id reuse");
    join_arena(&state, replacement, 0);
    drain_available(&replacement_rx);
    let (replacement_life, replacement_revision) = {
        let mut guard = state.lock().expect("state");
        let collector_frequency = guard.clients[&alpha].player.frequency;
        let replacement = guard.clients.get_mut(&replacement).expect("replacement");
        replacement.player.frequency = collector_frequency;
        replacement.server_bounty = 0;
        (
            replacement.life_seq,
            replacement.server_prize_state_revision,
        )
    };
    seed_collectible_at_tick(&state, 53, 512, 640, 18, tick + 2);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(53, 512, 640, 18),
        tick + 2,
    );
    let guard = state.lock().expect("state");
    let replacement = &guard.clients[&replacement];
    assert_eq!(replacement.life_seq, replacement_life);
    assert_ne!(
        replacement.server_prize_state_revision,
        replacement_revision
    );
    assert_eq!(
        replacement.server_shield_until_tick,
        Some(crate::continuum::continuum_tick_add(
            tick + 2,
            crate::arena_settings::arena_ship_shield_lifetime_ticks(replacement.player.ship),
        ))
    );
}

#[test]
fn simultaneous_claimers_have_one_atomic_winner_and_one_loser() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position(&state, alpha, &client_position_packet(1, 512, 640));
    broadcast_position(&state, beta, &client_position_packet(1, 512, 640));
    drain_available(&rx_a);
    drain_available(&rx_b);

    let collectible_id = 77;
    seed_collectible(&state, collectible_id, 512, 640, 15);
    let request = Arc::new(take_prize_request_payload(
        collectible_id,
        65_535,
        65_535,
        -28,
    ));
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for player_id in [alpha, beta] {
        let state = Arc::clone(&state);
        let request = Arc::clone(&request);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            relay_prize_pickup_at_tick(&state, player_id, &request, 500);
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().expect("claim worker");
    }

    let payload_a = read_reliable_payload_from_channel(&rx_a);
    let payload_b = read_reliable_payload_from_channel(&rx_b);
    let mut packet_types = [payload_a[0], payload_b[0]];
    packet_types.sort_unstable();
    assert_eq!(packet_types, [0x08, 0x61]);

    let (winner, alpha_bounty, beta_bounty) = {
        let state = state.lock().expect("state");
        assert!(state.active_hidden_prizes.is_empty());
        assert_eq!(state.claimed_prizes.len(), 1);
        let receipt = state.claimed_prizes[&collectible_id];
        assert_eq!(receipt.claim.x, 512);
        assert_eq!(receipt.claim.y, 640);
        assert_eq!(receipt.claim.prize_id, 15);
        (
            receipt.winner.player_id(),
            state.clients[&alpha].server_bounty,
            state.clients[&beta].server_bounty,
        )
    };
    let initial_bounty = crate::arena_settings::arena_ship_initial_bounty(0);
    assert_eq!(
        alpha_bounty as u32 + beta_bounty as u32,
        initial_bounty as u32 * 2 + 1
    );

    relay_prize_pickup_at_tick(&state, winner, &request, 501);
    let retry = if winner == alpha {
        read_reliable_payload_from_channel(&rx_a)
    } else {
        read_reliable_payload_from_channel(&rx_b)
    };
    assert_eq!(assert_prize_ack(retry, 512, 640, 15), collectible_id);
    if winner == alpha {
        assert!(rx_b.try_recv().is_err());
    } else {
        assert!(rx_a.try_recv().is_err());
    }
}

#[test]
fn hidden_and_death_collectibles_at_same_coordinates_keep_distinct_identities() {
    let tick = 1_000;
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(1, 512, 640));
    drain_available(&rx);

    seed_collectible_at_tick(&state, 40, 512, 640, 15, tick);
    let death_id = {
        let mut state = state.lock().expect("state");
        crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 21, tick, 300)
            .expect("death collectible")
    };
    assert_eq!(death_id, 41);

    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(40, 1, 2, -28),
        tick,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx), 512, 640, 15),
        40
    );
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(death_id, 3, 4, -28),
        tick,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx), 512, 640, 21),
        death_id
    );

    let state = state.lock().expect("state");
    assert!(state.active_hidden_prizes.is_empty());
    assert_eq!(state.claimed_prizes.len(), 2);
}

#[test]
fn expired_death_id_cannot_claim_a_new_green_at_the_same_coordinates() {
    let tick = 10_000;
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(1, 512, 640));
    drain_available(&rx);

    let (expired_id, replacement_id) = {
        let mut state = state.lock().expect("state");
        let expired_id =
            crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 15, tick, 2)
                .expect("first death collectible");
        assert!(
            crate::hidden_prizes::active_collectible_claim(&mut state, expired_id, tick + 1,)
                .is_some()
        );
        assert!(
            crate::hidden_prizes::active_collectible_claim(&mut state, expired_id, tick + 2,)
                .is_none()
        );
        let replacement_id =
            crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 21, tick + 2, 300)
                .expect("replacement death collectible");
        (expired_id, replacement_id)
    };
    assert_ne!(expired_id, replacement_id);

    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(expired_id, 512, 640, 15),
        tick + 2,
    );
    assert!(rx.try_recv().is_err());
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(replacement_id, 0, 0, -28),
        tick + 2,
    );
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx), 512, 640, 21),
        replacement_id
    );
}

#[test]
fn winner_receipt_is_scoped_to_session_and_life_generation() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    broadcast_position(&state, alpha, &client_position_packet(1, 512, 640));
    drain_available(&rx_a);

    seed_collectible(&state, 1, 512, 640, 15);
    let old_life_request = take_prize_request_payload(1, 512, 640, 15);
    relay_prize_pickup(&state, alpha, &old_life_request);
    assert_eq!(
        assert_prize_ack(read_reliable_payload_from_channel(&rx_a), 512, 640, 15),
        1
    );

    update_ship(&state, alpha, &ship_change_request(1));
    drain_available(&rx_a);
    relay_prize_pickup(&state, alpha, &old_life_request);
    assert!(
        rx_a.try_recv().is_err(),
        "a new life must not inherit retry authority"
    );

    // The newly selected ship has a different hull. Use the canonical spawn
    // rather than carrying the old ship's arbitrary fixture coordinate across
    // the life boundary; hull admission is intentionally authoritative.
    let (spawn_x, spawn_y) = {
        let locked = state.lock().expect("state");
        crate::zone::server_spawn_coordinates(locked.zone_map())
    };
    seed_collectible(&state, 2, spawn_x, spawn_y, 16);
    let mut reset_position = client_position_packet(2, spawn_x, spawn_y);
    reset_position[1] = 0;
    let (bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&alpha];
        (client.server_bounty, client.server_energy)
    };
    reset_position[16..18].copy_from_slice(&bounty.to_le_bytes());
    reset_position[18..20].copy_from_slice(&energy.to_le_bytes());
    broadcast_position(&state, alpha, &reset_position);
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&alpha];
        assert!(
            client.last_position.is_some(),
            "new-life reset position accepted"
        );
        assert_eq!(
            client.last_position_sequence,
            client.life_position_sequence_base
        );
    }
    drain_available(&rx_a);
    let old_session_request = take_prize_request_payload(2, 512, 640, 16);
    relay_prize_pickup(&state, alpha, &old_session_request);
    assert_eq!(
        assert_prize_ack(
            read_reliable_payload_from_channel(&rx_a),
            spawn_x,
            spawn_y,
            16,
        ),
        2
    );
    disconnect_player(&state, alpha);

    let (tx_replacement, rx_replacement) = mpsc::channel();
    {
        state.lock().expect("state").next_player_id = alpha;
    }
    let replacement = register_connection(&state, tx_replacement);
    assert_eq!(
        replacement, alpha,
        "test must exercise numeric player-id reuse"
    );
    join_arena(&state, replacement, 0);
    drain_available(&rx_replacement);
    broadcast_position(&state, replacement, &client_position_packet(3, 512, 640));
    drain_available(&rx_replacement);
    relay_prize_pickup(&state, replacement, &old_session_request);
    assert!(
        rx_replacement.try_recv().is_err(),
        "a replacement session must not inherit retry authority"
    );
}

#[test]
fn arena_reset_invalidates_old_ids_without_reusing_them() {
    let tick = 20_000;
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(1, 512, 640));
    drain_available(&rx);
    seed_collectible_at_tick(&state, 900, 512, 640, 15, tick);

    assert!(reset_arena_at_tick(&state, tick).starts_with("arena reset:"));
    drain_available(&rx);
    broadcast_position(&state, alpha, &client_position_packet(2, 512, 640));
    drain_available(&rx);
    relay_prize_pickup_at_tick(
        &state,
        alpha,
        &take_prize_request_payload(900, 512, 640, 15),
        tick + 1,
    );
    assert!(rx.try_recv().is_err());

    let next_id = {
        let mut state = state.lock().expect("state");
        crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 21, tick + 1, 300)
            .expect("post-reset collectible")
    };
    assert!(
        next_id > 900,
        "arena reset must not rewind collectible identity"
    );
}

#[test]
fn collectible_id_exhaustion_capacity_and_receipt_retention_are_bounded() {
    let tick = 30_000;
    let mut state = ServerState::default();
    state.next_collectible_id = u32::MAX - 1;
    assert_eq!(state.allocate_collectible_id(), Some(u32::MAX));
    assert_eq!(state.allocate_collectible_id(), None);
    assert_eq!(state.allocate_collectible_id(), None);

    state.next_collectible_id = 0;
    for index in 0..crate::hidden_prizes::SERVER_MAX_ACTIVE_COLLECTIBLES {
        assert!(
            crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 15, tick, 100,)
                .is_some(),
            "collectible {index} should fit"
        );
    }
    assert!(
        crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 15, tick, 100)
            .is_none(),
        "active collectible storage must be bounded"
    );
    assert!(
        crate::hidden_prizes::create_death_collectible(&mut state, 512, 640, 15, tick + 100, 100)
            .is_some(),
        "expiry must release active capacity"
    );

    let winner = crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            7,
            crate::authority_identity::PlayerSessionId::new(9),
        ),
        crate::authority_identity::LifeIdentity::new(7, 11),
    );
    for index in 0..crate::state::MAX_PRIZE_CLAIM_RECEIPTS + 17 {
        let id = index as u32 + 1;
        let claim = ServerPrizeClaim {
            collectible_id: id,
            x: 512,
            y: 640,
            prize_id: 15,
        };
        state.record_prize_claim_receipt(
            id,
            ServerPrizeReceipt {
                claim,
                winner,
                claimed_at_tick: tick + index as u32,
                authoritative_energy: 1234,
                prize_state_revision: 1,
                engine_shutdown_until_tick: None,
                engine_shutdown_application_required: false,
            },
            tick + index as u32,
        );
    }
    assert_eq!(
        state.claimed_prizes.len(),
        crate::state::MAX_PRIZE_CLAIM_RECEIPTS
    );
    assert!(!state.claimed_prizes.contains_key(&1));
    assert!(
        state
            .claimed_prizes
            .contains_key(&(crate::state::MAX_PRIZE_CLAIM_RECEIPTS as u32 + 17))
    );
    let last_tick = tick + crate::state::MAX_PRIZE_CLAIM_RECEIPTS as u32 + 17;
    assert_eq!(
        state.expire_prize_claims_older_than(last_tick + 1, 0),
        crate::state::MAX_PRIZE_CLAIM_RECEIPTS
    );
    assert!(state.claimed_prizes.is_empty());
}

#[test]
fn server_tracks_bounty_from_accepted_positive_prize_claims() {
    let state = Arc::new(Mutex::new(ServerState::default()));
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
        &client_position_packet(0x1122_3300, 512, 640),
    );
    drain_available(&rx_b);

    seed_collectible(&state, 0x1122_3344, 512, 640, 15);
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3344, 512, 640, 15),
    );
    drain_available(&rx_b);
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert_eq!(
            state.clients.get(&alpha).map(|client| client.server_bounty),
            Some(crate::arena_settings::arena_ship_initial_bounty(0) + 1)
        );
    }

    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3345, 512, 640, 15),
    );
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert_eq!(
            state.clients.get(&alpha).map(|client| client.server_bounty),
            Some(crate::arena_settings::arena_ship_initial_bounty(0) + 1)
        );
    }

    seed_collectible(&state, 0x1122_3346, 524, 640, -15);
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3346, 524, 640, -15),
    );
    drain_available(&rx_b);
    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state.clients.get(&alpha).map(|client| client.server_bounty),
        Some(crate::arena_settings::arena_ship_initial_bounty(0) + 1)
    );
}

#[test]
fn server_replays_initial_bounty_prizes_on_arena_join() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        maxed_spawn_loadout: false,
        ..ServerConfig::default()
    })));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);

    let weights = crate::arena_settings::arena_prize_weights();
    let weight_total = crate::arena_settings::arena_prize_weight_total();
    let initial_bounty = crate::arena_settings::arena_ship_initial_bounty(0);
    let replayed_prizes = crate::authority_prizes::initial_bounty_prize_ids(
        &weights,
        weight_total,
        initial_bounty as u32,
        1,
    );
    assert_eq!(replayed_prizes.len(), initial_bounty as usize);
    assert_eq!(initial_bounty, 30);
    assert!(!replayed_prizes.contains(&16));

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_bounty, initial_bounty);
    assert_live_stat(
        client.server_thrust_acceleration,
        replayed_live_stat(
            &replayed_prizes,
            11,
            crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(0),
            crate::arena_settings::arena_ship_thrust_upgrade_pixels_per_second_squared(0),
            crate::arena_settings::arena_ship_max_thrust_pixels_per_second_squared(0),
        ),
    );
    assert_live_stat(
        client.server_top_speed,
        replayed_live_stat(
            &replayed_prizes,
            12,
            crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(0),
            crate::arena_settings::arena_ship_top_speed_upgrade_pixels_per_second(0),
            crate::arena_settings::arena_ship_max_speed_pixels_per_second(0),
        ),
    );
    assert_live_stat(
        client.server_rotation_per_second,
        replayed_live_stat(
            &replayed_prizes,
            3,
            crate::arena_settings::arena_ship_initial_rotation_per_second(0),
            crate::arena_settings::arena_ship_rotation_upgrade_per_second(0),
            crate::arena_settings::arena_ship_max_rotation_per_second(0),
        ),
    );
    assert_live_stat(
        client.server_recharge_per_second,
        replayed_live_stat(
            &replayed_prizes,
            1,
            crate::arena_settings::arena_ship_initial_recharge_per_second(0),
            crate::arena_settings::arena_ship_recharge_upgrade_per_second(0),
            crate::arena_settings::arena_ship_max_recharge_per_second(0),
        ),
    );
    assert_eq!(
        client.server_max_energy,
        crate::arena_settings::arena_ship_max_energy(0)
    );
    assert_eq!(
        client.server_energy,
        crate::arena_settings::arena_ship_max_energy(0)
    );
    assert!(!client.server_weapons.proximity_bombs);
}

#[test]
fn server_applies_energy_and_full_charge_prizes_to_server_energy_state() {
    let state = Arc::new(Mutex::new(ServerState::default()));
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
        &client_position_packet(0x1122_3300, 512, 640),
    );
    drain_available(&rx_b);
    // Manually pin max + current below the arena cap so the Energy
    // upgrade prize (id=2) has headroom to grow. Chaos arena ships
    // initial = maximum = 1700, which would make the prize a no-op
    // here — that's intentional zone tuning (Option A from the
    // InitialEnergy decision) but it'd hide the upgrade-prize
    // semantics we're trying to exercise.
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.reset_server_loadout_for_ship();
        client.server_max_energy = 1_000;
        client.server_energy = 1_000;
    }

    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(client.server_max_energy, 1_000);
        assert_eq!(client.server_energy, 1_000);
    }

    seed_collectible(&state, 0x1122_3344, 512, 640, 2);
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3344, 512, 640, 2),
    );
    drain_available(&rx_b);
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(client.server_max_energy, 1_100);
        assert_eq!(client.server_energy, 1_000);
    }

    seed_collectible(&state, 0x1122_3345, 516, 640, 13);
    seed_collectible(&state, 0x1122_3346, 520, 640, -13);
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3345, 516, 640, 13),
    );
    drain_available(&rx_b);
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(client.server_max_energy, 1_100);
        assert_eq!(client.server_energy, 1_100);
    }

    seed_collectible(&state, 0x1122_3347, 524, 640, -2);
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3346, 520, 640, -13),
    );
    drain_available(&rx_b);
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(client.server_max_energy, 1_100);
        assert_eq!(client.server_energy, 1);
    }

    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3347, 524, 640, -2),
    );
    drain_available(&rx_b);
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    // Negative Energy prize subtracts UpgradeEnergy, but the result
    // is floored at `arena_ship_initial_energy(0)` = 1700 in chaos.
    // The manual setup above pinned max=1000, but the prize handler
    // raises it back to the configured initial-energy floor — so
    // even one negative Energy hit yanks max up to 1700.
    assert_eq!(client.server_max_energy, 1_700);
    assert_eq!(client.server_energy, 1);
}

#[test]
fn server_applies_accepted_weapon_prizes_to_server_weapon_state() {
    let state = Arc::new(Mutex::new(ServerState::default()));
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
        &client_position_packet(0x1122_3300, 512, 640),
    );
    drain_available(&rx_b);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.reset_server_loadout_for_ship();
    }

    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert_eq!(client.server_weapons.bomb_level, 0);
        assert_eq!(client.server_weapons.shrapnel, 0);
        assert!(!client.server_weapons.multifire_capable);
        assert!(!client.server_weapons.proximity_bombs);
        assert_eq!(client.server_weapons.repels, 0);
        assert_eq!(client.server_weapons.bricks, 0);
    }

    for (index, prize_id) in [9, 19, 15, 16, 21, 26].into_iter().enumerate() {
        seed_collectible(
            &state,
            0x1122_3344 + index as u32,
            512 + index as u16 * 2,
            640,
            prize_id,
        );
        relay_prize_pickup(
            &state,
            alpha,
            &take_prize_request_payload(
                0x1122_3344 + index as u32,
                512 + index as u16 * 2,
                640,
                prize_id,
            ),
        );
        drain_available(&rx_b);
    }

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_weapons.bomb_level, 1);
    assert_eq!(client.server_weapons.shrapnel, 2);
    assert!(client.server_weapons.multifire_capable);
    assert!(client.server_weapons.proximity_bombs);
    assert_eq!(client.server_weapons.repels, 1);
    assert_eq!(client.server_weapons.bricks, 1);
}

#[test]
fn server_applies_accepted_status_prizes_to_server_status_state() {
    let state = Arc::new(Mutex::new(ServerState::default()));
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
        &client_position_packet(0x1122_3300, 512, 640),
    );
    drain_available(&rx_b);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.reset_server_loadout_for_ship();
    }

    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert!(!client.server_status.stealth_capable);
        assert!(!client.server_status.cloak_capable);
        assert!(!client.server_status.xradar_capable);
        assert!(!client.server_status.antiwarp_capable);
    }

    for (index, prize_id) in [4, 5, 6, 20].into_iter().enumerate() {
        seed_collectible(
            &state,
            0x1122_3400 + index as u32,
            512 + index as u16 * 2,
            640,
            prize_id,
        );
        relay_prize_pickup(
            &state,
            alpha,
            &take_prize_request_payload(
                0x1122_3400 + index as u32,
                512 + index as u16 * 2,
                640,
                prize_id,
            ),
        );
        drain_available(&rx_b);
    }

    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert!(client.server_status.stealth_capable);
        assert!(!client.server_status.cloak_capable);
        assert!(client.server_status.xradar_capable);
        assert!(client.server_status.antiwarp_capable);
    }

    seed_collectible(&state, 0x1122_3404, 520, 640, -4);
    relay_prize_pickup(
        &state,
        alpha,
        &take_prize_request_payload(0x1122_3404, 520, 640, -4),
    );
    drain_available(&rx_b);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert!(!client.server_status.stealth_capable);
}

#[test]
fn canonical_prize_edges_match_reference_game_fallback_inventory_and_timers() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut guard = state.lock().expect("state");
    let client = guard.clients.get_mut(&player_id).expect("client");
    client.reset_server_loadout_for_ship();

    // Chaos Warbird Cloak is unavailable. Positive and incapable-negative
    // prizes fall back to the same-sign FullCharge; a negative prize removes
    // an already-owned capability without touching current energy.
    assert_eq!(crate::arena_settings::arena_ship_cloak_status(0), 0);
    client.server_energy = 123;
    client.apply_server_prize_effect(5);
    assert_eq!(client.server_energy, client.server_max_energy);
    assert!(!client.server_status.cloak_capable);
    client.server_energy = 456;
    client.apply_server_prize_effect(-5);
    assert_eq!(client.server_energy, 1);
    client.server_status.cloak_capable = true;
    client.server_energy = 456;
    client.apply_server_prize_effect(-5);
    assert_eq!(client.server_energy, 456);
    assert!(!client.server_status.cloak_capable);

    client.server_energy = 456;
    client.apply_server_prize_effect(-7);
    assert_eq!(
        client.server_energy, 1,
        "negative Warp is negative FullCharge"
    );

    client.server_weapons.shrapnel = 1;
    client.apply_server_prize_effect(-19);
    assert_eq!(client.server_weapons.shrapnel, 1);

    for _ in 0..u8::MAX {
        client.apply_server_prize_effect(28);
    }
    assert_eq!(
        client.server_weapons.portals,
        crate::arena_settings::arena_ship_max_portal_count(0)
    );
    client.apply_server_prize_effect(-28);
    assert_eq!(
        client.server_weapons.portals,
        crate::arena_settings::arena_ship_max_portal_count(0).saturating_sub(1)
    );

    client.server_rocket_until_tick = Some(9_000);
    client.apply_server_prize_effect_at_tick(14, 1_000);
    assert_eq!(client.server_rocket_until_tick, Some(9_000));

    client.reset_server_loadout_for_ship();
    assert_eq!(
        client.server_weapons.portals,
        crate::arena_settings::arena_ship_initial_portal_count(0)
    );
}

// Server-side Multiprize sub-prize replay must produce a stable sequence
// from the shared seed (1) so client and server stay in lockstep without
// requiring per-claim coordination over the wire. A divergence here would
// re-introduce the silent weapon/status desync that this fix closes.
#[test]
fn server_multiprize_replay_excludes_disallowed_sub_prizes() {
    let weights = crate::arena_settings::arena_prize_weights();
    let weight_total = crate::arena_settings::arena_prize_weight_total();
    let multiprize_count = crate::arena_settings::arena_multiprize_count();
    let prize_seed = 1_i32;

    let server_protocol_ids: Vec<i16> = crate::authority_prizes::multiprize_sub_prize_ids(
        &weights,
        weight_total,
        multiprize_count,
        prize_seed,
    );

    assert_eq!(server_protocol_ids.len() as u16, multiprize_count);

    // Client-side `multiprize_prize_allowed` excludes Warp(7),
    // EngineShutdown(14), Super(17), Shield(18), Multiprize(25), Brick(26).
    for &id in &server_protocol_ids {
        assert!(
            !matches!(id, 7 | 14 | 17 | 18 | 25 | 26),
            "disallowed sub-prize id {id} leaked into the multiprize replay",
        );
        assert!(
            (1..=28).contains(&id),
            "sub-prize id {id} outside Continuum prize protocol range",
        );
    }
}

#[test]
fn server_multiprize_replay_allows_full_charge_like_reference_game() {
    let mut weights = [0_u8; crate::arena_settings::CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(13 - 1) as usize] = 1;

    assert_eq!(
        crate::authority_prizes::multiprize_sub_prize_ids(&weights, 1, 1, 1),
        vec![13]
    );
}

#[test]
fn server_movement_prizes_mutate_live_values_for_every_ship() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut state = state.lock().expect("server state");
    let client = state.clients.get_mut(&player_id).expect("client present");
    for ship in 0..8 {
        client.player.ship = ship;
        client.reset_server_loadout_for_ship();
        let initial_thrust =
            crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(ship);
        let initial_speed =
            crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(ship);
        let initial_rotation = crate::arena_settings::arena_ship_initial_rotation_per_second(ship);
        let initial_recharge = crate::arena_settings::arena_ship_initial_recharge_per_second(ship);
        assert_live_stat(client.server_thrust_acceleration, initial_thrust);
        assert_live_stat(client.server_top_speed, initial_speed);
        assert_live_stat(client.server_rotation_per_second, initial_rotation);
        assert_live_stat(client.server_recharge_per_second, initial_recharge);

        // Prize ids: 1 = Recharge, 3 = Rotation, 11 = Thrust, 12 = Speed.
        for _ in 0..u8::MAX {
            client.apply_server_prize_effect(1);
            client.apply_server_prize_effect(3);
            client.apply_server_prize_effect(11);
            client.apply_server_prize_effect(12);
        }
        let max_thrust =
            crate::arena_settings::arena_ship_max_thrust_pixels_per_second_squared(ship);
        let max_speed = crate::arena_settings::arena_ship_max_speed_pixels_per_second(ship);
        let max_rotation = crate::arena_settings::arena_ship_max_rotation_per_second(ship);
        let max_recharge = crate::arena_settings::arena_ship_max_recharge_per_second(ship);
        assert_live_stat(client.server_thrust_acceleration, max_thrust);
        assert_live_stat(client.server_top_speed, max_speed);
        assert_live_stat(client.server_rotation_per_second, max_rotation);
        assert_live_stat(client.server_recharge_per_second, max_recharge);

        for prize_id in [-11, -12, -3, -1] {
            client.apply_server_prize_effect(prize_id);
        }
        assert_live_stat(
            client.server_thrust_acceleration,
            (max_thrust
                - crate::arena_settings::arena_ship_thrust_upgrade_pixels_per_second_squared(ship))
            .max(initial_thrust),
        );
        assert_live_stat(
            client.server_top_speed,
            (max_speed
                - crate::arena_settings::arena_ship_top_speed_upgrade_pixels_per_second(ship))
            .max(initial_speed),
        );
        assert_live_stat(
            client.server_rotation_per_second,
            (max_rotation - crate::arena_settings::arena_ship_rotation_upgrade_per_second(ship))
                .max(initial_rotation),
        );
        assert_live_stat(
            client.server_recharge_per_second,
            (max_recharge - crate::arena_settings::arena_ship_recharge_upgrade_per_second(ship))
                .max(initial_recharge),
        );

        for prize_id in [11, 12, 3, 1] {
            client.apply_server_prize_effect(prize_id);
        }
        assert_live_stat(client.server_thrust_acceleration, max_thrust);
        assert_live_stat(client.server_top_speed, max_speed);
        assert_live_stat(client.server_rotation_per_second, max_rotation);
        assert_live_stat(client.server_recharge_per_second, max_recharge);

        for _ in 0..u8::MAX {
            for prize_id in [-11, -12, -3, -1] {
                client.apply_server_prize_effect(prize_id);
            }
        }
        assert_live_stat(client.server_thrust_acceleration, initial_thrust);
        assert_live_stat(client.server_top_speed, initial_speed);
        assert_live_stat(client.server_rotation_per_second, initial_rotation);
        assert_live_stat(client.server_recharge_per_second, initial_recharge);
    }
}

#[test]
fn chaos_warbird_partial_caps_reverse_from_the_live_value() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut state = state.lock().expect("server state");
    let client = state.clients.get_mut(&player_id).expect("client present");
    client.reset_server_loadout_for_ship();
    for _ in 0..u8::MAX {
        for prize_id in [11, 12, 3, 1] {
            client.apply_server_prize_effect(prize_id);
        }
    }
    for prize_id in [-11, -12, -3, -1] {
        client.apply_server_prize_effect(prize_id);
    }
    assert_live_stat(client.server_thrust_acceleration, 170.0);
    assert_live_stat(client.server_top_speed, 300.0);
    assert_live_stat(client.server_rotation_per_second, 0.625);
    assert_live_stat(client.server_recharge_per_second, 98.4);
}

#[test]
fn stat_upgrades_reset_when_loadout_resets_for_ship() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut state = state.lock().expect("server state");
    let client = state.clients.get_mut(&player_id).expect("client present");
    client.apply_server_prize_effect(11);
    client.apply_server_prize_effect(12);
    client.apply_server_prize_effect(3);
    client.apply_server_prize_effect(1);
    let before_reset = (
        client.server_thrust_acceleration,
        client.server_top_speed,
        client.server_rotation_per_second,
        client.server_recharge_per_second,
    );
    client.reset_server_loadout_for_ship();
    assert_ne!(
        before_reset,
        (
            client.server_thrust_acceleration,
            client.server_top_speed,
            client.server_rotation_per_second,
            client.server_recharge_per_second,
        )
    );
    assert_live_stat(
        client.server_thrust_acceleration,
        crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(0),
    );
    assert_live_stat(
        client.server_top_speed,
        crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(0),
    );
    assert_live_stat(
        client.server_rotation_per_second,
        crate::arena_settings::arena_ship_initial_rotation_per_second(0),
    );
    assert_live_stat(
        client.server_recharge_per_second,
        crate::arena_settings::arena_ship_initial_recharge_per_second(0),
    );
}

#[test]
fn timed_prizes_use_exact_deadlines_wrap_stacking_expiry_and_reset() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("server state");
    let client = state.clients.get_mut(&player_id).expect("client present");
    client.clear_server_timed_protections();
    client.server_prize_effect_rng_seed = crate::authority_prizes::SECURITY_PRIZE_SEED;

    let tick = u32::MAX - 5;
    let max_super = crate::arena_settings::arena_ship_super_lifetime_ticks(0);
    let mut expected_seed = crate::authority_prizes::SECURITY_PRIZE_SEED;
    let first_duration = crate::vie::vie_rng_next(&mut expected_seed) % max_super;
    client.apply_server_prize_effect_at_tick(17, tick);
    assert_eq!(
        client.server_super_until_tick,
        (first_duration > 0).then(|| crate::continuum::continuum_tick_add(tick, first_duration))
    );
    let first_deadline = client.server_super_until_tick;
    client.apply_server_prize_effect_at_tick(17, tick);
    let second_duration = crate::vie::vie_rng_next(&mut expected_seed) % max_super;
    let expected_duration = first_duration.max(second_duration);
    assert_eq!(
        client.server_super_until_tick,
        (expected_duration > 0)
            .then(|| crate::continuum::continuum_tick_add(tick, expected_duration))
    );
    assert!(client.server_super_until_tick != first_deadline || second_duration <= first_duration);

    client.apply_server_prize_effect_at_tick(18, tick);
    assert_eq!(
        client.server_shield_until_tick,
        Some(crate::continuum::continuum_tick_add(
            tick,
            crate::arena_settings::arena_ship_shield_lifetime_ticks(0)
        ))
    );
    client.apply_server_prize_effect_at_tick(-14, tick);
    let shutdown_ticks = crate::arena_settings::arena_engine_shutdown_ticks().saturating_mul(3);
    let shutdown_deadline = crate::continuum::continuum_tick_add(tick, shutdown_ticks);
    assert_eq!(
        client.server_engine_shutdown_until_tick,
        Some(shutdown_deadline)
    );
    assert!(!client.server_engine_shutdown_active(tick));
    client.server_engine_shutdown_start_sequence = Some(0);
    assert!(
        client.server_engine_shutdown_active(crate::continuum::continuum_tick_sub(
            shutdown_deadline,
            1
        ))
    );
    client.maintain_short_tick_state(shutdown_deadline, 0, 0, 0, 0);
    assert_eq!(client.server_engine_shutdown_until_tick, None);

    client.reset_server_loadout_for_ship_at_tick(shutdown_deadline);
    assert_eq!(client.server_super_until_tick, None);
    assert_eq!(client.server_shield_until_tick, None);
    assert_eq!(client.server_engine_shutdown_until_tick, None);
}

#[test]
fn engine_shutdown_and_emp_expire_independently_and_never_clear_rocket() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("server state");
    let client = state.clients.get_mut(&player_id).expect("client present");
    client.clear_server_timed_protections();

    let tick = crate::continuum::CONTINUUM_TICK_MASK - 20;
    let short = crate::continuum::continuum_tick_add(tick, 40);
    let long = crate::continuum::continuum_tick_add(tick, 90);
    let rocket = crate::continuum::continuum_tick_add(tick, 120);
    client.server_rocket_until_tick = Some(rocket);

    client.last_input_seq = Some(u16::MAX);
    client.apply_server_prize_effect_at_tick(14, tick);
    assert_eq!(client.server_engine_shutdown_start_sequence, None);
    assert!(!client.server_engine_shutdown_active_for_sequence(tick, u16::MAX));
    client.server_engine_shutdown_start_sequence = Some(0);
    assert!(client.server_engine_shutdown_active_for_sequence(tick, 0));
    client.clear_server_timed_protections();
    client.server_rocket_until_tick = Some(rocket);

    // EMP expires first; the independent prize deadline still shuts down movement.
    client.server_engine_shutdown_start_sequence = Some(0);
    client.server_engine_shutdown_until_tick = Some(long);
    client.server_emp_start_sequence = Some(0);
    client.server_emp_until_tick = Some(short);
    client.maintain_short_tick_state(short, 0, 0, 0, 0);
    assert_eq!(client.server_emp_until_tick, None);
    assert_eq!(client.server_engine_shutdown_until_tick, Some(long));
    assert!(client.server_engine_shutdown_active(short));
    assert_eq!(client.server_rocket_until_tick, Some(rocket));

    // Prize Shutdown expires first; EMP still pauses recharge. Applying a new
    // Shutdown replaces only its own source and must not erase EMP or Rocket.
    client.server_engine_shutdown_start_sequence = Some(0);
    client.server_engine_shutdown_until_tick = Some(short);
    client.server_emp_start_sequence = Some(0);
    client.server_emp_until_tick = Some(long);
    client.maintain_short_tick_state(short, 0, 0, 0, 0);
    assert_eq!(client.server_engine_shutdown_until_tick, None);
    assert_eq!(client.server_emp_until_tick, Some(long));
    assert!(client.server_emp_active(short));
    assert!(!client.server_engine_shutdown_active(short));
    client.apply_server_prize_effect_at_tick(14, short);
    client.server_engine_shutdown_start_sequence = Some(0);
    assert_eq!(client.server_emp_until_tick, Some(long));
    assert_eq!(client.server_rocket_until_tick, Some(rocket));

    client.maintain_short_tick_state(long, 0, 0, 0, 0);
    assert_eq!(client.server_emp_until_tick, None);
    // The replacement prize deadline may extend past EMP; whichever source
    // is later remains authoritative without deadline overwrite.
    assert_eq!(
        client.server_engine_shutdown_active(long),
        client
            .server_engine_shutdown_until_tick
            .is_some_and(|deadline| { !crate::continuum::continuum_tick_reached(long, deadline) })
    );
    let replacement_expiry = client
        .server_engine_shutdown_until_tick
        .expect("replacement prize deadline");
    client.maintain_short_tick_state(replacement_expiry, 0, 0, 0, 0);
    assert_eq!(client.server_engine_shutdown_until_tick, None);
    assert!(!client.server_engine_shutdown_active(replacement_expiry));
}

#[test]
fn engine_shutdown_replaces_by_source_and_preserves_zero_deadline_presence() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("server state");
    let client = state.clients.get_mut(&player_id).expect("client present");
    client.clear_server_timed_protections();

    let tick = 1_000;
    client.replace_server_engine_shutdown_at_tick(300, tick);
    client.server_engine_shutdown_start_sequence = Some(10);
    client.replace_server_engine_shutdown_at_tick(100, tick);
    assert_eq!(
        client.server_engine_shutdown_until_tick,
        Some(crate::continuum::continuum_tick_add(tick, 100)),
        "positive replacement shortens an active negative interval",
    );
    assert_eq!(client.server_engine_shutdown_start_sequence, Some(10));
    client.replace_server_engine_shutdown_at_tick(300, tick);
    assert_eq!(
        client.server_engine_shutdown_until_tick,
        Some(crate::continuum::continuum_tick_add(tick, 300)),
        "negative replacement lengthens an active positive interval",
    );

    let wrap_tick = crate::continuum::CONTINUUM_TICK_MASK - 13;
    client.replace_server_engine_shutdown_at_tick(14, wrap_tick);
    assert_eq!(client.server_engine_shutdown_until_tick, Some(0));
    assert_eq!(
        client.server_engine_shutdown_deadline_for_delivery(wrap_tick),
        Some(0),
        "a legitimate tick-zero deadline remains present",
    );

    client.pending_prize_effect_applications.push_back(
        crate::state::PendingPrizeEffectApplication {
            collectible_id: 91,
            prize_id: 14,
            life_seq: client.life_seq,
            prize_state_revision: client.server_prize_state_revision,
            minimum_application_sequence: Some(11),
            maximum_application_sequence: Some(11),
            latest_application_sequence: None,
            deferred_application: None,
            movement_tuning_before_bits: None,
        },
    );
    client.replace_server_engine_shutdown_at_tick(0, wrap_tick);
    assert_eq!(client.server_engine_shutdown_until_tick, None);
    assert_eq!(client.server_engine_shutdown_start_sequence, None);
    assert!(client.pending_prize_effect_applications.is_empty());

    let claim = ServerPrizeClaim {
        collectible_id: 92,
        x: 512,
        y: 640,
        prize_id: 14,
    };
    let ack = prize_pickup_ack_from_claim(
        claim,
        crate::player_events::PrizeRecipientAck {
            player_id,
            arena_stream_id: client.arena_stream_id,
            authoritative_energy: client.server_energy,
            life_seq: client.life_seq,
            prize_state_revision: client.server_prize_state_revision.max(1),
            engine_shutdown_until_tick: Some(0),
            engine_shutdown_application_required: true,
        },
    );
    assert_eq!(ack.len(), 44);
    assert_eq!(u32::from_le_bytes(ack[21..25].try_into().unwrap()), 0);
    assert_eq!(
        ack[25], 0b11,
        "deadline presence and barrier are distinct bits"
    );
}

#[test]
fn repeated_shutdown_rebases_are_exact_marker_tombstones_across_wrap() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let a = u16::MAX - 1;
    let b = u16::MAX;
    let c = 0;
    let application_a = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.replace_server_engine_shutdown_at_tick(500, 1_000);
        let application = crate::prize_application::PrizeEffectApplied {
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq: client.life_seq,
            collectible_id: 0x1122_3344,
            prize_id: 14,
            prize_state_revision: client.server_prize_state_revision,
            application_sequence: a,
        };
        client.pending_prize_effect_applications.push_back(
            crate::state::PendingPrizeEffectApplication {
                collectible_id: application.collectible_id,
                prize_id: application.prize_id,
                life_seq: application.life_seq,
                prize_state_revision: application.prize_state_revision,
                minimum_application_sequence: Some(b),
                maximum_application_sequence: Some(b),
                latest_application_sequence: None,
                deferred_application: None,
                movement_tuning_before_bits: None,
            },
        );
        application
    };
    let terminal_ab =
        crate::player_events::apply_prize_effect_application(&state, player_id, application_a)
            .expect("A rebases to B");
    assert_eq!(terminal_ab.requested_application_sequence, a);
    assert_eq!(terminal_ab.application_sequence, b);
    assert_eq!(
        terminal_ab.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE,
    );

    {
        let mut state = state.lock().expect("state");
        state
            .clients
            .get_mut(&player_id)
            .expect("client")
            .observe_prize_effect_application_boundary(b);
    }
    let application_b = crate::prize_application::PrizeEffectApplied {
        application_sequence: b,
        ..application_a
    };
    let terminal_bc =
        crate::player_events::apply_prize_effect_application(&state, player_id, application_b)
            .expect("B rebases to C");
    assert_eq!(terminal_bc.requested_application_sequence, b);
    assert_eq!(terminal_bc.application_sequence, c);
    assert_eq!(
        terminal_bc.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE,
    );
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, application_a),
        Some(terminal_ab),
        "lost A terminal retries only A's cached A-to-B result",
    );
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, application_b),
        Some(terminal_bc),
        "lost B terminal retries only B's cached B-to-C result",
    );

    let application_c = crate::prize_application::PrizeEffectApplied {
        application_sequence: c,
        ..application_a
    };
    let applied =
        crate::player_events::apply_prize_effect_application(&state, player_id, application_c)
            .expect("C applies");
    assert_eq!(applied.requested_application_sequence, c);
    assert_eq!(applied.application_sequence, c);
    assert_eq!(
        applied.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED,
    );
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, application_c),
        Some(applied),
        "duplicate C receives only C's applied terminal",
    );
}

#[test]
fn shutdown_claim_barrier_uses_active_at_claim_not_stale_start_presence() {
    let prior_deadline = 1_001;
    for (claim_tick, run_maintenance, expected_required) in [
        (1_000, false, false),
        (1_001, false, true),
        (1_002, true, true),
    ] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        drain_available(&rx);
        broadcast_position(&state, player_id, &client_position_packet(55, 512, 640));
        drain_available(&rx);
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.last_input_seq = Some(55);
            client.server_engine_shutdown_start_sequence = Some(40);
            client.server_engine_shutdown_until_tick = Some(prior_deadline);
            if claim_tick >= prior_deadline {
                client.pending_prize_effect_applications.push_back(
                    crate::state::PendingPrizeEffectApplication {
                        collectible_id: 700,
                        prize_id: -14,
                        life_seq: client.life_seq,
                        prize_state_revision: client.server_prize_state_revision,
                        minimum_application_sequence: Some(56),
                        maximum_application_sequence: Some(56),
                        latest_application_sequence: Some(56),
                        deferred_application: None,
                        movement_tuning_before_bits: None,
                    },
                );
            }
            if run_maintenance {
                client.maintain_short_tick_state(prior_deadline, 0, 0, 0, 0);
            }
        }
        drain_available(&rx);
        let collectible_id = 701 + claim_tick;
        seed_collectible_at_tick(&state, collectible_id, 512, 640, 14, claim_tick);
        relay_prize_pickup_at_tick(
            &state,
            player_id,
            &take_prize_request_payload(collectible_id, 512, 640, 14),
            claim_tick,
        );
        let state = state.lock().expect("state");
        let receipt = state
            .claimed_prizes
            .get(&collectible_id)
            .expect("claim receipt");
        assert_eq!(
            receipt.engine_shutdown_application_required, expected_required,
            "claim tick {claim_tick} maintenance={run_maintenance}",
        );
        let client = &state.clients[&player_id];
        assert!(
            client
                .pending_prize_effect_applications
                .iter()
                .all(|pending| pending.collectible_id != 700),
            "an expired predecessor cannot block the new-life FIFO head",
        );
        assert_eq!(
            client
                .pending_prize_effect_applications
                .iter()
                .any(|pending| pending.collectible_id == collectible_id),
            expected_required,
        );
    }
}

#[test]
fn rotating_and_thrusting_preclaim_history_switches_only_at_exact_receipted_sequence() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let application = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.last_input_seq = Some(u16::MAX - 1);
        client.recent_input_records.extend([
            crate::state::SequencedInputObservation {
                sequence: u16::MAX - 2,
                input: crate::state::ClientInputObservation {
                    bits: 0b0000_1001,
                    direction: 39,
                },
                door_seed: 7,
                door_phase_tick: 999,
            },
            crate::state::SequencedInputObservation {
                sequence: u16::MAX - 1,
                input: crate::state::ClientInputObservation {
                    bits: 0b0000_0101,
                    direction: 0,
                },
                door_seed: 7,
                door_phase_tick: 1_000,
            },
        ]);
        client.apply_server_prize_effect_at_tick(-14, 1_000);
        client.pending_prize_effect_applications.push_back(
            crate::state::PendingPrizeEffectApplication {
                collectible_id: 77,
                prize_id: -14,
                life_seq: client.life_seq,
                prize_state_revision: client.server_prize_state_revision,
                minimum_application_sequence: Some(u16::MAX),
                maximum_application_sequence: None,
                latest_application_sequence: None,
                deferred_application: None,
                movement_tuning_before_bits: None,
            },
        );
        client.observe_prize_effect_application_boundary(u16::MAX);
        client.observe_prize_effect_application_boundary(0);
        client.observe_prize_effect_application_boundary(1);
        assert_eq!(
            client.pending_prize_effect_applications[0].maximum_application_sequence,
            Some(2),
            "every newer old-semantics tail advances the wrapped successor"
        );
        client.pending_prize_effect_applications.push_back(
            crate::state::PendingPrizeEffectApplication {
                collectible_id: 78,
                prize_id: -14,
                life_seq: client.life_seq,
                prize_state_revision: client.server_prize_state_revision.wrapping_add(1),
                minimum_application_sequence: Some(0),
                maximum_application_sequence: Some(2),
                latest_application_sequence: None,
                deferred_application: None,
                movement_tuning_before_bits: None,
            },
        );
        assert!(client.server_engine_shutdown_active_for_sequence(1_001, 0) == false);
        assert_eq!(client.recent_input_records[0].input.bits, 0b0000_1001);
        assert_eq!(client.recent_input_records[1].input.bits, 0b0000_0101);
        crate::prize_application::PrizeEffectApplied {
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq: client.life_seq,
            collectible_id: 77,
            prize_id: -14,
            prize_state_revision: client.server_prize_state_revision,
            application_sequence: 2,
        }
    };
    let reordered = crate::prize_application::PrizeEffectApplied {
        collectible_id: 78,
        prize_state_revision: application.prize_state_revision.wrapping_add(1),
        application_sequence: 1,
        ..application
    };
    let reordered_terminal = crate::prize_application::PrizeEffectAccepted {
        outcome: crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE,
        application_sequence: 2,
        ..crate::prize_application::PrizeEffectAccepted::from(reordered)
    };
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, reordered),
        Some(reordered_terminal),
        "a later claim receives the durable head boundary"
    );
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, reordered),
        Some(reordered_terminal),
        "loss of the invalid application's rebase terminal retries the same durable outcome"
    );
    let accepted =
        crate::player_events::apply_prize_effect_application(&state, player_id, application)
            .expect("exact application");
    assert_eq!(accepted.application_sequence, 2);
    {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert!(!client.server_engine_shutdown_active_for_sequence(1_001, u16::MAX));
        assert!(!client.server_engine_shutdown_active_for_sequence(1_001, 0));
        assert!(!client.server_engine_shutdown_active_for_sequence(1_001, 1));
        assert!(client.server_engine_shutdown_active_for_sequence(1_001, 2));
    }
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, application),
        Some(accepted),
        "lost terminal retry is idempotent"
    );

    let reordered_rebased = crate::prize_application::PrizeEffectApplied {
        application_sequence: 2,
        ..reordered
    };
    let reordered_accepted =
        crate::player_events::apply_prize_effect_application(&state, player_id, reordered_rebased)
            .expect("reordered suffix applies after the missing prefix");
    assert_eq!(reordered_accepted.application_sequence, 2);
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, reordered_rebased),
        Some(reordered_accepted),
        "a lost suffix terminal is also idempotent"
    );

    let mut stale = application;
    stale.collectible_id = 80;
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, stale),
        Some(crate::prize_application::PrizeEffectAccepted {
            outcome: crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
            ..crate::prize_application::PrizeEffectAccepted::from(stale)
        })
    );
    for invalid in [
        crate::prize_application::PrizeEffectApplied {
            server_session_id: application.server_session_id.wrapping_add(1),
            ..application
        },
        crate::prize_application::PrizeEffectApplied {
            life_seq: application.life_seq.wrapping_add(1),
            ..application
        },
    ] {
        assert_eq!(
            crate::player_events::apply_prize_effect_application(&state, player_id, invalid),
            Some(crate::prize_application::PrizeEffectAccepted {
                outcome: crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
                ..crate::prize_application::PrizeEffectAccepted::from(invalid)
            }),
            "session/life cutover retires an old application identity"
        );
    }
    let mut old_sequence = application;
    old_sequence.collectible_id = 79;
    old_sequence.application_sequence = u16::MAX - 1;
    {
        let mut state = state.lock().expect("state");
        state
            .clients
            .get_mut(&player_id)
            .unwrap()
            .pending_prize_effect_applications
            .push_back(crate::state::PendingPrizeEffectApplication {
                collectible_id: 79,
                prize_id: -14,
                life_seq: application.life_seq,
                prize_state_revision: application.prize_state_revision,
                minimum_application_sequence: Some(u16::MAX),
                maximum_application_sequence: Some(1),
                latest_application_sequence: None,
                deferred_application: None,
                movement_tuning_before_bits: None,
            });
    }
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, old_sequence),
        Some(crate::prize_application::PrizeEffectAccepted {
            outcome: crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE,
            application_sequence: 1,
            ..crate::prize_application::PrizeEffectAccepted::from(old_sequence)
        }),
        "an application cannot reinterpret pre-claim rotating/thrusting history"
    );
}

#[test]
fn pending_shutdown_timeout_emits_a_durable_cancel_terminal_and_unpauses_fifo() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let deadline = 75_000_u32;
    let application = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.apply_server_prize_effect_at_tick(-14, deadline.saturating_sub(1));
        client.server_engine_shutdown_until_tick = Some(deadline);
        let application = crate::prize_application::PrizeEffectApplied {
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq: client.life_seq,
            collectible_id: 0xdead_beef,
            prize_id: -14,
            prize_state_revision: client.server_prize_state_revision,
            application_sequence: u16::MAX,
        };
        client.pending_prize_effect_applications.push_back(
            crate::state::PendingPrizeEffectApplication {
                collectible_id: application.collectible_id,
                prize_id: application.prize_id,
                life_seq: application.life_seq,
                prize_state_revision: application.prize_state_revision,
                minimum_application_sequence: Some(u16::MAX),
                maximum_application_sequence: Some(u16::MAX),
                latest_application_sequence: None,
                deferred_application: None,
                movement_tuning_before_bits: None,
            },
        );
        application
    };
    {
        let mut state = state.lock().expect("state");
        state
            .clients
            .get_mut(&player_id)
            .expect("client")
            .maintain_short_tick_state(deadline, 0, 0, 0, 0);
    }
    let terminal = crate::prize_application::PrizeEffectAccepted::parse(
        &read_reliable_payload_from_channel(&rx),
    )
    .expect("timeout cancellation terminal");
    assert_eq!(
        terminal.outcome,
        crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(terminal.application_sequence, u16::MAX);
    {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert!(client.pending_prize_effect_applications.is_empty());
        assert_eq!(client.server_engine_shutdown_until_tick, None);
        assert_eq!(client.server_engine_shutdown_start_sequence, None);
    }
    assert_eq!(
        crate::player_events::apply_prize_effect_application(&state, player_id, application),
        Some(terminal),
        "a retry after timeout receives the durable original cancellation"
    );
}

fn assert_prize_ack(packet: Vec<u8>, x: u16, y: u16, prize_id: i16) -> u32 {
    assert_eq!(packet.len(), 44);
    assert_eq!(packet[0], 0x61);
    let timestamp = u32::from_le_bytes(packet[1..5].try_into().unwrap());
    assert_eq!(u16::from_le_bytes(packet[5..7].try_into().unwrap()), x);
    assert_eq!(u16::from_le_bytes(packet[7..9].try_into().unwrap()), y);
    assert_eq!(
        i16::from_le_bytes(packet[9..11].try_into().unwrap()),
        prize_id
    );
    let _authoritative_energy = u16::from_le_bytes(packet[11..13].try_into().unwrap());
    assert_ne!(
        u32::from_le_bytes(packet[13..17].try_into().unwrap()),
        0,
        "prize ACK must bind the authoritative winner life"
    );
    assert_ne!(u32::from_le_bytes(packet[17..21].try_into().unwrap()), 0);
    assert_eq!(
        u64::from_le_bytes(packet[26..34].try_into().unwrap()),
        crate::wire::server_session_id(),
    );
    assert_ne!(
        u64::from_le_bytes(packet[34..42].try_into().unwrap()),
        0,
        "prize ACK must bind the current arena stream",
    );
    assert_ne!(
        u16::from_le_bytes(packet[42..44].try_into().unwrap()),
        u16::MAX,
    );
    timestamp
}

fn assert_player_prize(
    packet: Vec<u8>,
    player_id: u16,
    collectible_id: u32,
    x: u16,
    y: u16,
    prize_id: i16,
) {
    assert_eq!(
        packet,
        player_prize_from_claim(
            player_id,
            ServerPrizeClaim {
                collectible_id,
                x,
                y,
                prize_id,
            },
        )
    );
}
