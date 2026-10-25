use super::*;
use crate::arena::{PortalPlacementOutcome, apply_portal_placement_at_tick};
use crate::portal_placement::{PortalPlacementAccepted, PortalPlacementRequest};
use crate::state::{CanonicalPositionWitness, PendingPortalPlacement, ServerSimMovement};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};

fn joined_portal_player() -> (Arc<Mutex<ServerState>>, u16) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = crate::arena::register_connection(&state, tx);
    crate::arena::join_arena_at_tick(&state, player_id, 0, 10);
    (state, player_id)
}

fn witness(sequence: u16, life_seq: u32, x: f32, y: f32) -> CanonicalPositionWitness {
    CanonicalPositionWitness {
        sequence,
        life_seq,
        state: ServerSimMovement {
            position_x: x,
            position_y: y,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_000.0,
            max_energy: 1_000.0,
        },
        wire_x: x as u16,
        wire_y: y as u16,
        wire_velocity_x: 0,
        wire_velocity_y: 0,
        direction: 0,
        door_seed: 0,
        door_phase_tick: 10,
    }
}

fn pending_for(
    state: &ServerState,
    player_id: u16,
    action_id: u32,
    position_sequence: u16,
    queued_at_tick: u32,
) -> PendingPortalPlacement {
    let client = &state.clients[&player_id];
    PendingPortalPlacement {
        identity: state
            .active_player_runtime_identity(player_id)
            .expect("joined player has an exact runtime"),
        request: PortalPlacementRequest {
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: client.arena_stream_id,
            player_id,
            life_seq: client.life_seq,
            action_id,
            position_sequence,
        },
        queued_at_tick,
    }
}

#[test]
fn portal_placement_is_exactly_once_across_duplicate_and_action_wrap() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let life_seq = state.clients[&player_id].life_seq;
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_weapons.portals = 2;
    client.server_prize_state_revision = u32::MAX;
    client.canonical_position_witnesses = VecDeque::from([
        witness(u16::MAX, life_seq, 1_000.0, 1_000.0),
        witness(0, life_seq, 1_001.0, 1_000.0),
    ]);
    client.last_position_sequence = Some(0);

    let first = pending_for(&state, player_id, u32::MAX, u16::MAX, 100);
    let terminal = apply_portal_placement_at_tick(&mut state, first.clone(), 100);
    assert!(matches!(
        terminal,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            portals: 1,
            prize_state_revision: 1,
            ..
        })
    ));
    assert_eq!(state.clients[&player_id].server_weapons.portals, 1);
    let active = state.clients[&player_id]
        .active_portal
        .expect("applied placement creates server endpoint");
    assert_eq!((active.position_x, active.position_y), (1_000.0, 1_000.0));
    assert_eq!(active.placed_at_tick, 100);
    assert_eq!(active.placement_action_id, u32::MAX);
    assert_eq!(
        active.expires_at_tick,
        crate::continuum::continuum_tick_add(
            100,
            crate::arena_settings::arena_portal_lifetime_ticks(),
        )
    );

    let duplicate = apply_portal_placement_at_tick(&mut state, first, 101);
    assert!(matches!(
        duplicate,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            portals: 1,
            prize_state_revision: 1,
            ..
        })
    ));
    assert_eq!(state.clients[&player_id].server_weapons.portals, 1);

    let wrapped = pending_for(&state, player_id, 1, 0, 102);
    let terminal = apply_portal_placement_at_tick(&mut state, wrapped, 102);
    assert!(matches!(
        terminal,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            portals: 0,
            prize_state_revision: 2,
            ..
        })
    ));
    assert_eq!(state.clients[&player_id].server_weapons.portals, 0);
    assert_eq!(
        state.clients[&player_id].active_portal.map(|portal| (
            portal.position_x,
            portal.position_y,
            portal.placement_action_id
        )),
        Some((1_001.0, 1_000.0, 1))
    );
}

#[test]
fn active_portal_expires_and_clears_at_lifecycle_boundary() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let life_seq = state.clients[&player_id].life_seq;
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_weapons.portals = 1;
    client
        .canonical_position_witnesses
        .push_back(witness(1, life_seq, 700.5, 701.25));
    client.last_position_sequence = Some(1);
    let pending = pending_for(&state, player_id, 1, 1, 10);
    assert!(matches!(
        apply_portal_placement_at_tick(&mut state, pending, 10),
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            ..
        })
    ));
    let expiry = state.clients[&player_id]
        .active_portal
        .unwrap()
        .expires_at_tick;
    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .maintain_short_tick_state(expiry, 0, 0, 0, 0);
    assert!(state.clients[&player_id].active_portal.is_none());

    state.clients.get_mut(&player_id).unwrap().active_portal = Some(crate::state::ServerPortal {
        position_x: 1.0,
        position_y: 2.0,
        placed_at_tick: 20,
        expires_at_tick: 30,
        placement_action_id: 2,
    });
    state
        .advance_client_life_at_tick(player_id, 21)
        .expect("life advances");
    assert!(state.clients[&player_id].active_portal.is_none());
}

#[test]
fn portal_placement_waits_for_reordered_position_then_commits() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let life_seq = state.clients[&player_id].life_seq;
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_weapons.portals = 1;
    client.last_position_sequence = Some(40);
    client
        .canonical_position_witnesses
        .push_back(witness(40, life_seq, 500.0, 500.0));
    let pending = pending_for(&state, player_id, 7, 41, 200);

    assert_eq!(
        apply_portal_placement_at_tick(&mut state, pending.clone(), 200),
        PortalPlacementOutcome::Deferred
    );
    assert_eq!(state.clients[&player_id].server_weapons.portals, 1);

    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .canonical_position_witnesses
        .push_back(witness(41, life_seq, 501.0, 500.0));
    let terminal = apply_portal_placement_at_tick(&mut state, pending, 201);
    assert!(matches!(
        terminal,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            portals: 0,
            ..
        })
    ));
}

#[test]
fn portal_placement_loss_window_closes_with_one_rejection() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_weapons.portals = 1;
    client.last_position_sequence = Some(50);
    let pending = pending_for(&state, player_id, 8, 51, 300);

    assert_eq!(
        apply_portal_placement_at_tick(&mut state, pending.clone(), 312),
        PortalPlacementOutcome::Deferred
    );
    let terminal = apply_portal_placement_at_tick(&mut state, pending.clone(), 313);
    assert!(matches!(
        terminal,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_REJECTED,
            portals: 1,
            ..
        })
    ));
    assert_eq!(state.clients[&player_id].server_weapons.portals, 1);

    let duplicate = apply_portal_placement_at_tick(&mut state, pending, 314);
    assert_eq!(duplicate, terminal);
    assert_eq!(state.clients[&player_id].server_weapons.portals, 1);
}

#[test]
fn portal_placement_cannot_cross_a_life_cutover() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let old_life = state.clients[&player_id].life_seq;
    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .canonical_position_witnesses
        .push_back(witness(1, old_life, 800.0, 800.0));
    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .server_weapons
        .portals = 1;
    let pending = pending_for(&state, player_id, 9, 1, 400);

    state
        .advance_client_life_at_tick(player_id, 401)
        .expect("active player life advances");
    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .server_weapons
        .portals = 1;
    assert_eq!(
        apply_portal_placement_at_tick(&mut state, pending, 401),
        PortalPlacementOutcome::Stale
    );
    assert_eq!(state.clients[&player_id].server_weapons.portals, 1);
}

#[test]
fn portal_placement_queue_is_bounded_and_collapses_exact_duplicates() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let identity = state.active_player_runtime_identity(player_id).unwrap();
    for action_id in 1..=crate::state::MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME as u32 {
        let pending = pending_for(&state, player_id, action_id, action_id as u16, 500);
        assert!(state.queue_portal_placement_for_runtime(
            identity,
            pending.request,
            pending.queued_at_tick,
        ));
    }
    let duplicate = pending_for(&state, player_id, 1, 999, 501);
    assert!(state.queue_portal_placement_for_runtime(
        identity,
        duplicate.request,
        duplicate.queued_at_tick,
    ));
    assert_eq!(
        state.pending_portal_placements.len(),
        crate::state::MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME
    );

    let overflow = pending_for(&state, player_id, 65, 65, 501);
    assert!(!state.queue_portal_placement_for_runtime(
        identity,
        overflow.request,
        overflow.queued_at_tick,
    ));
    assert_eq!(
        state.pending_portal_placements.len(),
        crate::state::MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME
    );
}

#[test]
fn portal_placement_waits_for_terminal_capacity_before_mutating_inventory() {
    let (state, player_id) = joined_portal_player();
    let mut state = state.lock().unwrap();
    let life_seq = state.clients[&player_id].life_seq;
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_weapons.portals = 1;
    client.server_prize_state_revision = 9;
    client
        .canonical_position_witnesses
        .push_back(witness(1, life_seq, 1_000.0, 1_000.0));
    client.last_position_sequence = Some(1);
    while crate::reliable::can_retain_application_payload(
        client,
        PortalPlacementAccepted::PACKET_LEN,
    ) {
        crate::reliable::send_reliable_payload(client, b"capacity");
    }
    let pending = pending_for(&state, player_id, 10, 1, 550);

    assert_eq!(
        apply_portal_placement_at_tick(&mut state, pending, 550),
        PortalPlacementOutcome::Deferred
    );
    let client = &state.clients[&player_id];
    assert_eq!(client.server_weapons.portals, 1);
    assert_eq!(client.server_prize_state_revision, 9);
    assert!(!client.portal_action_seen(10));
    assert!(client.portal_placement_terminals.is_empty());
}

#[test]
fn portal_placement_respects_server_owned_enemy_antiwarp() {
    let (state, player_id) = joined_portal_player();
    let (enemy_tx, _enemy_rx) = mpsc::channel();
    let enemy_id = crate::arena::register_connection(&state, enemy_tx);
    crate::arena::join_arena_at_tick(&state, enemy_id, 0, 10);

    let mut state = state.lock().unwrap();
    let life_seq = state.clients[&player_id].life_seq;
    let client = state.clients.get_mut(&player_id).unwrap();
    client.server_weapons.portals = 1;
    client
        .canonical_position_witnesses
        .push_back(witness(1, life_seq, 1_000.0, 1_000.0));
    client.last_position_sequence = Some(1);
    let mut enemy_position = vec![0; 23];
    enemy_position[0] = crate::arena::LARGE_POSITION_PACKET;
    enemy_position[4..6].copy_from_slice(&1_000_u16.to_le_bytes());
    enemy_position[13] = POSITION_STATUS_ANTIWARP;
    enemy_position[15..17].copy_from_slice(&1_000_u16.to_le_bytes());
    state.clients.get_mut(&enemy_id).unwrap().last_position = Some(enemy_position);

    let blocked = pending_for(&state, player_id, 1, 1, 600);
    let terminal = apply_portal_placement_at_tick(&mut state, blocked, 600);
    assert!(matches!(
        terminal,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_REJECTED,
            portals: 1,
            ..
        })
    ));

    state.clients.get_mut(&enemy_id).unwrap().player.frequency =
        state.clients[&player_id].player.frequency;
    let allowed = pending_for(&state, player_id, 2, 1, 601);
    let terminal = apply_portal_placement_at_tick(&mut state, allowed, 601);
    assert!(matches!(
        terminal,
        PortalPlacementOutcome::Terminal(PortalPlacementAccepted {
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            portals: 0,
            ..
        })
    ));
}

#[test]
fn portal_placement_antiwarp_matches_reference_range_and_live_hostility_rules() {
    // The reference game's PlayerManager::IsAntiwarped uses inclusive squared range,
    // excludes teammates/spectators/enter delay, and requires active status.
    for (label, dx, dy, active, joined, alive, ship, teammate, position, blocked) in [
        ("overlap", 0, 0, true, true, true, 0, false, true, true),
        (
            "exact axial edge",
            640,
            0,
            true,
            true,
            true,
            0,
            false,
            true,
            true,
        ),
        (
            "outside axial edge",
            641,
            0,
            true,
            true,
            true,
            0,
            false,
            true,
            false,
        ),
        (
            "diagonal inside",
            384,
            512,
            true,
            true,
            true,
            0,
            false,
            true,
            true,
        ),
        (
            "diagonal outside",
            385,
            512,
            true,
            true,
            true,
            0,
            false,
            true,
            false,
        ),
        ("inactive", 0, 0, false, true, true, 0, false, true, false),
        ("unjoined", 0, 0, true, false, true, 0, false, true, false),
        (
            "dead/enter delay",
            0,
            0,
            true,
            true,
            false,
            0,
            false,
            true,
            false,
        ),
        ("spectator", 0, 0, true, true, true, 8, false, true, false),
        ("teammate", 0, 0, true, true, true, 0, true, true, false),
        (
            "missing position",
            0,
            0,
            true,
            true,
            true,
            0,
            false,
            false,
            false,
        ),
    ] {
        let (state, player_id) = joined_portal_player();
        let (enemy_tx, _enemy_rx) = mpsc::channel();
        let enemy_id = crate::arena::register_connection(&state, enemy_tx);
        crate::arena::join_arena_at_tick(&state, enemy_id, 0, 10);
        let mut state = state.lock().unwrap();
        assert_eq!(crate::arena_settings::arena_antiwarp_radius_pixels(), 640.0);
        let frequency = state.clients[&player_id].player.frequency;
        let client = state.clients.get_mut(&player_id).unwrap();
        client.server_weapons.portals = 1;
        client.canonical_position_witnesses.push_back(witness(
            1,
            client.life_seq,
            1_000.0,
            1_000.0,
        ));
        client.last_position_sequence = Some(1);
        let enemy = state.clients.get_mut(&enemy_id).unwrap();
        enemy.joined = joined;
        enemy.alive = alive;
        enemy.player.ship = ship;
        enemy.player.frequency = if teammate { frequency } else { frequency + 1 };
        let mut payload = vec![0; 23];
        payload[0] = crate::arena::LARGE_POSITION_PACKET;
        payload[4..6].copy_from_slice(&(1_000_u16 + dx).to_le_bytes());
        payload[15..17].copy_from_slice(&(1_000_u16 + dy).to_le_bytes());
        payload[13] = if active {
            POSITION_STATUS_ANTIWARP
        } else {
            POSITION_STATUS_STEALTH
        };
        enemy.last_position = position.then_some(payload);
        let pending = pending_for(&state, player_id, 1, 1, 600);
        let PortalPlacementOutcome::Terminal(terminal) =
            apply_portal_placement_at_tick(&mut state, pending, 600)
        else {
            panic!("{label}: placement must terminate");
        };
        assert_eq!(
            terminal.outcome == PortalPlacementAccepted::OUTCOME_REJECTED,
            blocked,
            "{label}"
        );
        assert_eq!(
            state.clients[&player_id].server_weapons.portals,
            u8::from(blocked),
            "{label}: rejected placement must preserve inventory"
        );
    }
}
