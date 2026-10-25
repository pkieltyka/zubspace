use super::*;
use crate::arena::{PortalWarpOutcome, apply_portal_warp_at_tick};
use crate::portal_warp::{PortalWarpAccepted, PortalWarpRequest};
use crate::state::{CanonicalPositionWitness, PendingPortalWarp, ServerPortal, ServerSimMovement};
use std::sync::{Arc, Mutex, mpsc};

fn fixture() -> (Arc<Mutex<ServerState>>, u16) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = crate::arena::register_connection(&state, tx);
    crate::arena::join_arena_at_tick(&state, player_id, 0, 10);
    {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        let sequence = u16::MAX;
        let canonical = ServerSimMovement {
            position_x: 4_000.25,
            position_y: 5_000.5,
            orientation: 0.375,
            velocity_x: 123.5,
            velocity_y: -45.25,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = CanonicalPositionWitness {
            sequence,
            life_seq: client.life_seq,
            state: canonical,
            wire_x: 4_000,
            wire_y: 5_001,
            wire_velocity_x: 1_235,
            wire_velocity_y: -453,
            direction: 15,
            door_seed: 7,
            door_phase_tick: 10,
        };
        let mut position = vec![0; 23];
        position[0] = crate::arena::LARGE_POSITION_PACKET;
        position[1] = witness.direction;
        position[4..6].copy_from_slice(&witness.wire_x.to_le_bytes());
        position[6..8].copy_from_slice(&witness.wire_velocity_y.to_le_bytes());
        position[10..12].copy_from_slice(&witness.wire_velocity_x.to_le_bytes());
        position[15..17].copy_from_slice(&witness.wire_y.to_le_bytes());
        position[21..23].copy_from_slice(&client.server_energy.to_le_bytes());
        client.last_position = Some(position);
        client.last_position_sequence = Some(sequence);
        client.canonical_movement_witness = Some(witness);
        client.canonical_position_witnesses.push_back(witness);
    }
    (state, player_id)
}

fn pending(state: &ServerState, player_id: u16, action_id: u32, tick: u32) -> PendingPortalWarp {
    let client = &state.clients[&player_id];
    PendingPortalWarp {
        identity: state.active_player_runtime_identity(player_id).unwrap(),
        request: PortalWarpRequest {
            force_random: false,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: client.arena_stream_id,
            player_id,
            life_seq: client.life_seq,
            action_id,
            position_sequence: u16::MAX,
        },
        queued_at_tick: tick,
    }
}

#[test]
fn portal_recall_consumes_endpoint_and_preserves_exact_velocity_energy_orientation() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let energy = state.clients[&player_id].server_energy;
    state.clients.get_mut(&player_id).unwrap().active_portal = Some(ServerPortal {
        position_x: 8_100.75,
        position_y: 8_200.125,
        placed_at_tick: 20,
        expires_at_tick: 2_000,
        placement_action_id: 1,
    });
    let request = pending(&state, player_id, 2, 100);
    let outcome = apply_portal_warp_at_tick(&mut state, request, 100);
    let PortalWarpOutcome::Terminal(terminal) = outcome else {
        panic!("recall must terminate");
    };
    assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_APPLIED);
    assert!(terminal.used_portal);
    assert_eq!(terminal.energy, energy);
    assert_eq!(
        terminal.canonical_state(),
        (8_100.75, 8_200.125, 123.5, -45.25, 0.375)
    );
    let client = &state.clients[&player_id];
    assert!(client.active_portal.is_none());
    assert_eq!(client.server_energy, energy);
    assert_eq!(client.last_position_sequence, Some(u16::MAX));
    assert_eq!(
        client.canonical_movement_witness.unwrap().state.position_x,
        8_100.75
    );
    assert_eq!(
        crate::arena::large_position_coordinates(client.last_position.as_deref().unwrap()),
        Some((8_101, 8_200))
    );
    let segment = client
        .actor_motion_history
        .current_segment()
        .expect("applied recall motion segment");
    let motion = client
        .actor_motion_history
        .at_or_before(segment, 100)
        .expect("applied recall seeds its exact motion destination");
    assert_eq!(motion.position(), (8_100.75, 8_200.125));
    assert_eq!(motion.velocity(), (123.5, -45.25));

    let duplicate = apply_portal_warp_at_tick(&mut state, request, 101);
    assert_eq!(duplicate, outcome);
    assert_eq!(state.clients[&player_id].server_energy, energy);
}

#[test]
fn enemy_antiwarp_rejects_recall_and_random_warp_without_spending_or_moving() {
    for force_random in [false, true] {
        let (state, player_id) = fixture();
        let (tx, _rx) = mpsc::channel();
        let enemy_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena_at_tick(&state, enemy_id, 0, 10);
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.server_energy = client.server_max_energy;
        client.active_portal = Some(ServerPortal {
            position_x: 8_100.75,
            position_y: 8_200.125,
            placed_at_tick: 20,
            expires_at_tick: 2_000,
            placement_action_id: 1,
        });
        let energy = client.server_energy;
        let before = client.canonical_movement_witness.unwrap();
        let mut enemy_position = vec![0; 23];
        enemy_position[0] = crate::arena::LARGE_POSITION_PACKET;
        enemy_position[4..6].copy_from_slice(&4_000_u16.to_le_bytes());
        enemy_position[15..17].copy_from_slice(&5_000_u16.to_le_bytes());
        enemy_position[13] = POSITION_STATUS_ANTIWARP;
        state.clients.get_mut(&enemy_id).unwrap().last_position = Some(enemy_position);
        let mut request = pending(&state, player_id, 2, 100);
        request.request.force_random = force_random;
        let PortalWarpOutcome::Terminal(terminal) =
            apply_portal_warp_at_tick(&mut state, request, 100)
        else {
            panic!("antiwarped request must terminate");
        };
        assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_REJECTED);
        let client = &state.clients[&player_id];
        assert_eq!(client.server_energy, energy);
        assert_eq!(
            client.canonical_movement_witness.unwrap().state,
            before.state
        );
        assert_eq!(client.active_portal.unwrap().placement_action_id, 1);
        assert!(client.server_warp_settle_until_tick.is_none());
        assert!(client.server_warp_bomb_lock_until_tick.is_none());

        // A current accepted status change releases the block; no reconnect or
        // stale retry of the rejected action is needed.
        state
            .clients
            .get_mut(&enemy_id)
            .unwrap()
            .last_position
            .as_mut()
            .unwrap()[13] = 0;
        let mut request = pending(&state, player_id, 3, 101);
        request.request.force_random = force_random;
        let PortalWarpOutcome::Terminal(terminal) =
            apply_portal_warp_at_tick(&mut state, request, 101)
        else {
            panic!("released request must terminate");
        };
        assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_APPLIED);
        assert_eq!(terminal.used_portal, !force_random);
        assert_eq!(
            state.clients[&player_id].active_portal.is_some(),
            force_random
        );
    }
}

#[test]
fn portal_recall_uses_exact_retained_witness_after_cross_channel_tail_advance() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    state.clients.get_mut(&player_id).unwrap().active_portal = Some(ServerPortal {
        position_x: 8_100.75,
        position_y: 8_200.125,
        placed_at_tick: 20,
        expires_at_tick: 2_000,
        placement_action_id: 1,
    });
    let request = pending(&state, player_id, 2, 100);
    let mut later = state.clients[&player_id]
        .canonical_movement_witness
        .expect("fixture witness");
    later.sequence = 0;
    later.state.position_x += 5.0;
    let client = state.clients.get_mut(&player_id).unwrap();
    client.last_position_sequence = Some(0);
    client.canonical_movement_witness = Some(later);
    client.canonical_position_witnesses.push_back(later);

    let PortalWarpOutcome::Terminal(terminal) = apply_portal_warp_at_tick(&mut state, request, 100)
    else {
        panic!("retained exact witness must terminate");
    };
    assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_APPLIED);
    assert_eq!(terminal.position_sequence, u16::MAX);
    assert_eq!(
        terminal.canonical_state(),
        (8_100.75, 8_200.125, 123.5, -45.25, 0.375),
    );
}

#[test]
fn no_portal_manual_warp_requires_full_energy_and_commits_spawn_stop_and_drain() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let request = pending(&state, player_id, 1, 100);
    let terminal = apply_portal_warp_at_tick(&mut state, request, 100);
    let PortalWarpOutcome::Terminal(terminal) = terminal else {
        panic!("spawn Warp terminates");
    };
    assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_APPLIED);
    assert!(!terminal.used_portal);
    assert_eq!(terminal.energy, 1);
    let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(state.zone_map());
    assert_eq!(
        terminal.canonical_state(),
        (f32::from(spawn_x), f32::from(spawn_y), 0.0, 0.0, 0.375)
    );
    assert_eq!(state.clients[&player_id].server_energy, 1);
    assert!(
        state.clients[&player_id]
            .server_warp_settle_until_tick
            .is_some()
    );
    assert!(
        state.clients[&player_id]
            .server_warp_bomb_lock_until_tick
            .is_some()
    );
}

#[test]
fn manual_warp_rejection_and_terminal_capacity_are_non_mutating() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_energy = client.server_max_energy.saturating_sub(1);
    let before = client.canonical_movement_witness;
    let request = pending(&state, player_id, 1, 100);
    let PortalWarpOutcome::Terminal(rejected) = apply_portal_warp_at_tick(&mut state, request, 100)
    else {
        panic!("invalid Warp receives terminal");
    };
    assert_eq!(rejected.outcome, PortalWarpAccepted::OUTCOME_REJECTED);
    assert_eq!(state.clients[&player_id].canonical_movement_witness, before);

    let (capacity_state, capacity_player) = fixture();
    let mut capacity_state = capacity_state.lock().unwrap();
    let client = capacity_state.clients.get_mut(&capacity_player).unwrap();
    client.active_portal = Some(ServerPortal {
        position_x: 9_000.0,
        position_y: 9_100.0,
        placed_at_tick: 1,
        expires_at_tick: 2_000,
        placement_action_id: 1,
    });
    while crate::reliable::can_retain_application_payload(client, PortalWarpAccepted::PACKET_LEN) {
        crate::reliable::send_reliable_payload(client, b"capacity");
    }
    let request = pending(&capacity_state, capacity_player, 2, 100);
    assert_eq!(
        apply_portal_warp_at_tick(&mut capacity_state, request, 100),
        PortalWarpOutcome::Deferred
    );
    assert!(
        capacity_state.clients[&capacity_player]
            .active_portal
            .is_some()
    );
}

#[test]
fn manual_warp_request_is_life_scoped_and_queue_bounded() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let stale = pending(&state, player_id, 1, 100);
    state.advance_client_life_at_tick(player_id, 101).unwrap();
    assert_eq!(
        apply_portal_warp_at_tick(&mut state, stale, 101),
        PortalWarpOutcome::Stale
    );

    let identity = state.active_player_runtime_identity(player_id).unwrap();
    for action_id in 1..=crate::state::MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME as u32 {
        let client = &state.clients[&player_id];
        let request = PortalWarpRequest {
            force_random: false,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: client.arena_stream_id,
            player_id,
            life_seq: client.life_seq,
            action_id,
            position_sequence: action_id as u16,
        };
        assert!(state.queue_portal_warp_for_runtime(identity, request, 200));
    }
    let client = &state.clients[&player_id];
    let overflow = PortalWarpRequest {
        force_random: false,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: client.arena_stream_id,
        player_id,
        life_seq: client.life_seq,
        action_id: 65,
        position_sequence: 65,
    };
    assert!(!state.queue_portal_warp_for_runtime(identity, overflow, 200));
}

#[test]
fn forced_spawn_warp_bypasses_and_preserves_live_portal() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let portal = ServerPortal {
        position_x: 8100.75,
        position_y: 8200.125,
        placed_at_tick: 20,
        expires_at_tick: 2000,
        placement_action_id: 1,
    };
    let client = state.clients.get_mut(&player_id).unwrap();
    client.active_portal = Some(portal);
    client.server_energy = client.server_max_energy;
    let mut request = pending(&state, player_id, 2, 100);
    request.request.force_random = true;
    let PortalWarpOutcome::Terminal(terminal) = apply_portal_warp_at_tick(&mut state, request, 100)
    else {
        panic!("forced warp must terminate");
    };
    assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_APPLIED);
    assert!(!terminal.used_portal);
    let x = f32::from_bits(terminal.position_x_bits);
    let y = f32::from_bits(terminal.position_y_bits);
    assert!((0.0..16384.0).contains(&x) && (0.0..16384.0).contains(&y));
    assert!(!crate::zone::solid_tile(crate::zone::tile_at_world(
        state.zone_map(),
        x as u16,
        y as u16
    )));
    assert_ne!((x, y), (portal.position_x, portal.position_y));
    assert_eq!(terminal.energy, 1);
    assert_eq!(
        state.clients[&player_id]
            .active_portal
            .unwrap()
            .placement_action_id,
        1
    );
    let replay = apply_portal_warp_at_tick(&mut state, request, 101);
    assert_eq!(
        replay,
        PortalWarpOutcome::Terminal(terminal),
        "retry must replay the same destination, never reroll or spend again"
    );
    assert_eq!(state.clients[&player_id].server_energy, 1);
}

#[test]
fn forced_spawn_warp_requires_full_energy_even_with_live_portal() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let client = state.clients.get_mut(&player_id).unwrap();
    client.active_portal = Some(ServerPortal {
        position_x: 8100.75,
        position_y: 8200.125,
        placed_at_tick: 20,
        expires_at_tick: 2000,
        placement_action_id: 1,
    });
    client.server_energy = client.server_max_energy - 1;
    let mut request = pending(&state, player_id, 2, 100);
    request.request.force_random = true;
    let PortalWarpOutcome::Terminal(terminal) = apply_portal_warp_at_tick(&mut state, request, 100)
    else {
        panic!("forced warp rejection must terminate");
    };
    assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_REJECTED);
    assert!(state.clients[&player_id].active_portal.is_some());
}

#[test]
fn blocked_random_warp_rejects_without_spending_energy_or_consuming_portal() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    assert_eq!(state.clients[&player_id].player.frequency, 0);
    // FFA overrides radar-mode corners with the central population-sized
    // area. This one-player fixture's area fits inside the middle half of
    // the map. Cover that entire area and the center fallback.
    let mut tiles = (256..=768)
        .flat_map(|x| (256..=768).map(move |y| (x, y)))
        .collect::<Vec<_>>();
    let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
    tiles.push((x / 16, y / 16));
    let mut packet = vec![0; 11];
    packet[9..11].copy_from_slice(&1_u16.to_le_bytes());
    state.active_bricks.push(crate::state::ServerBrick {
        tiles,
        packet,
        expires_at_tick: 1000,
    });
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_energy = client.server_max_energy;
    client.active_portal = Some(ServerPortal {
        position_x: 8100.75,
        position_y: 8200.125,
        placed_at_tick: 20,
        expires_at_tick: 2000,
        placement_action_id: 1,
    });
    let energy = client.server_energy;
    let prior = client.canonical_movement_witness.unwrap();
    let mut request = pending(&state, player_id, 2, 100);
    request.request.force_random = true;
    let PortalWarpOutcome::Terminal(terminal) = apply_portal_warp_at_tick(&mut state, request, 100)
    else {
        panic!("blocked random warp must terminate");
    };
    assert_eq!(terminal.outcome, PortalWarpAccepted::OUTCOME_REJECTED);
    assert_eq!(terminal.energy, energy);
    let client = &state.clients[&player_id];
    assert_eq!(client.server_energy, energy);
    assert_eq!(client.active_portal.unwrap().placement_action_id, 1);
    assert_eq!(
        client.canonical_movement_witness.unwrap().state,
        prior.state
    );
    assert!(client.server_warp_settle_until_tick.is_none());
    assert!(client.server_warp_bomb_lock_until_tick.is_none());
    // The same temporary wall becomes nonblocking at its authoritative expiry.
    state.active_bricks[0].expires_at_tick = 99;
    let mut retry = pending(&state, player_id, 3, 100);
    retry.request.force_random = true;
    let PortalWarpOutcome::Terminal(applied) = apply_portal_warp_at_tick(&mut state, retry, 100)
    else {
        panic!("expired brick must no longer prevent random warp");
    };
    assert_eq!(applied.outcome, PortalWarpAccepted::OUTCOME_APPLIED);
    assert!(state.clients[&player_id].active_portal.is_some());
}

fn overlap_brick_for_fixture(state: &mut ServerState, player_id: u16, team: u16, expires: u32) {
    let position = state.clients[&player_id]
        .canonical_movement_witness
        .unwrap()
        .state;
    let tile = (
        (position.position_x / 16.0).floor() as u16,
        (position.position_y / 16.0).floor() as u16,
    );
    let mut packet = vec![0; 11];
    packet[9..11].copy_from_slice(&team.to_le_bytes());
    state.active_bricks.push(crate::state::ServerBrick {
        tiles: vec![tile],
        packet,
        expires_at_tick: expires,
    });
}

#[test]
fn brick_overlap_queues_then_receipts_a_same_life_free_relocation_exactly_once() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let runtime = state.active_player_runtime_identity(player_id).unwrap();
    let before = state.clients[&player_id]
        .canonical_movement_witness
        .unwrap();
    let energy = state.clients[&player_id].server_energy;
    let weapons = state.clients[&player_id].server_weapons;
    let frequency = state.clients[&player_id].player.frequency;
    overlap_brick_for_fixture(&mut state, player_id, frequency.wrapping_add(1), 1000);
    crate::objectives::queue_enemy_brick_overlap_corrections(&mut state, 100);
    let client = &state.clients[&player_id];
    assert_eq!(
        client.canonical_movement_witness.unwrap(),
        before,
        "an offer must not teleport before receipt"
    );
    let proposal = client
        .pending_movement_correction
        .as_ref()
        .unwrap()
        .proposal;
    assert_eq!(
        proposal.reason,
        crate::movement_correction::MovementCorrection::REASON_BRICK_OVERLAP
    );
    crate::objectives::queue_enemy_brick_overlap_corrections(&mut state, 101);
    assert_eq!(
        state.clients[&player_id]
            .pending_movement_correction
            .as_ref()
            .unwrap()
            .proposal,
        proposal
    );
    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id,
        life_seq: runtime.life.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: 1,
    };
    let receipt = crate::arena::apply_movement_correction_application_at_tick(
        &mut state,
        runtime,
        application,
        102,
    )
    .unwrap();
    assert_eq!(
        receipt.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_APPLIED
    );
    let client = &state.clients[&player_id];
    assert_ne!(
        (receipt.position_x, receipt.position_y),
        (before.state.position_x, before.state.position_y)
    );
    assert_eq!((receipt.velocity_x, receipt.velocity_y), (0.0, 0.0));
    assert_eq!(client.life_seq, runtime.life.life_seq);
    assert_eq!(client.server_energy, energy);
    assert_eq!(client.server_weapons, weapons);
    assert_eq!(client.player.frequency, frequency);
    assert_eq!(client.last_position_sequence, Some(1));
    assert_eq!(
        state.movement_snapbacks_total, 0,
        "legitimate brick warp is not network drift"
    );
    assert_eq!(
        crate::arena::apply_movement_correction_application_at_tick(
            &mut state,
            runtime,
            application,
            103
        ),
        Some(receipt)
    );
}

#[test]
fn brick_overlap_ignores_friendly_expired_dead_and_spectator_hulls() {
    for case in 0..4 {
        let (state, player_id) = fixture();
        let mut state = state.lock().unwrap();
        let frequency = state.clients[&player_id].player.frequency;
        overlap_brick_for_fixture(
            &mut state,
            player_id,
            if case == 0 {
                frequency
            } else {
                frequency.wrapping_add(1)
            },
            if case == 1 { 99 } else { 1000 },
        );
        if case == 2 {
            state.clients.get_mut(&player_id).unwrap().alive = false;
        }
        if case == 3 {
            state.clients.get_mut(&player_id).unwrap().player.ship = 8;
        }
        crate::objectives::queue_enemy_brick_overlap_corrections(&mut state, 100);
        assert!(
            state.clients[&player_id]
                .pending_movement_correction
                .is_none(),
            "excluded case {case}"
        );
    }
}

#[test]
fn brick_overlap_relocates_only_hostile_bot_without_resetting_its_life_or_loadout() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    state.config.bot_count = 2;
    state.initialize_configured_bots();
    let position = state.clients[&player_id]
        .canonical_movement_witness
        .unwrap()
        .state;
    for bot in &mut state.bots {
        bot.x = position.position_x;
        bot.y = position.position_y;
        bot.velocity_x = 10.0;
        bot.velocity_y = -5.0;
    }
    let friendly_frequency = state.bots[1].player.frequency;
    overlap_brick_for_fixture(&mut state, player_id, friendly_frequency, 1000);
    let life = state.bots[0].life_seq;
    let weapons = state.bots[0].weapons;
    let energy = state.bots[0].energy;
    crate::objectives::queue_enemy_brick_overlap_corrections(&mut state, 100);
    let bot = &state.bots[0];
    assert_ne!((bot.x, bot.y), (position.position_x, position.position_y));
    assert_eq!((bot.velocity_x, bot.velocity_y), (0.0, 0.0));
    assert_eq!(bot.life_seq, life);
    assert_eq!(bot.weapons, weapons);
    assert_eq!(bot.energy, energy);
    assert!(bot.actor_motion_history.current_segment().is_some());
    assert_eq!(bot.actor_motion_history.current_samples().count(), 1);
    assert_eq!(
        (state.bots[1].x, state.bots[1].y),
        (position.position_x, position.position_y)
    );
    assert_eq!(
        (state.bots[1].velocity_x, state.bots[1].velocity_y),
        (10.0, -5.0)
    );
}

#[test]
fn brick_overlap_application_cancels_safely_when_new_walls_block_every_destination() {
    let (state, player_id) = fixture();
    let mut state = state.lock().unwrap();
    let runtime = state.active_player_runtime_identity(player_id).unwrap();
    let frequency = state.clients[&player_id].player.frequency;
    overlap_brick_for_fixture(&mut state, player_id, frequency.wrapping_add(1), 1000);
    crate::objectives::queue_enemy_brick_overlap_corrections(&mut state, 100);
    let before = state.clients[&player_id]
        .canonical_movement_witness
        .unwrap();
    let proposal = state.clients[&player_id]
        .pending_movement_correction
        .as_ref()
        .unwrap()
        .proposal;
    // Cover the entire one-player central area after the offer, so the
    // terminal must not blindly reuse its once-clear proposed location.
    state.active_bricks[0]
        .tiles
        .extend((256..=768).flat_map(|x| (256..=768).map(move |y| (x, y))));
    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id,
        life_seq: runtime.life.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: 1,
    };
    let receipt = crate::arena::apply_movement_correction_application_at_tick(
        &mut state,
        runtime,
        application,
        102,
    )
    .unwrap();
    assert_eq!(
        receipt.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(
        state.clients[&player_id]
            .canonical_movement_witness
            .unwrap(),
        before
    );
    assert_eq!(
        state.clients[&player_id].server_energy,
        before.state.energy as u16
    );
}
