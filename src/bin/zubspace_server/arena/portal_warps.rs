use crate::portal_warp::{PortalWarpAccepted, PortalWarpRequest};
use crate::state::{PendingPortalWarp, ServerState};

const PORTAL_WARP_POSITION_RECOVERY_TICKS: u32 = 12;
const MANUAL_WARP_BOMB_LOCK_TICKS: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortalWarpOutcome {
    Deferred,
    Stale,
    Terminal(PortalWarpAccepted),
}

fn terminal_for(
    request: PortalWarpRequest,
    server_tick: u32,
    state: crate::state::ServerSimMovement,
    energy: u16,
    used_portal: bool,
    outcome: u8,
) -> PortalWarpAccepted {
    PortalWarpAccepted {
        server_session_id: request.server_session_id,
        arena_stream_id: request.arena_stream_id,
        player_id: request.player_id,
        life_seq: request.life_seq,
        action_id: request.action_id,
        position_sequence: request.position_sequence,
        server_tick,
        position_x_bits: state.position_x.to_bits(),
        position_y_bits: state.position_y.to_bits(),
        velocity_x_bits: state.velocity_x.to_bits(),
        velocity_y_bits: state.velocity_y.to_bits(),
        orientation_bits: state.orientation.to_bits(),
        energy,
        used_portal,
        outcome,
    }
}

fn bomb_cooldown_ready(client: &crate::state::ClientHandle, server_tick: u32) -> bool {
    let delay = crate::arena_settings::arena_ship_weapon_fire_delay_ticks(client.player.ship, 3);
    client
        .last_bomb_fire_tick
        .is_none_or(|last| crate::continuum::continuum_tick_elapsed(server_tick, last) >= delay)
        && client
            .server_warp_bomb_lock_until_tick
            .is_none_or(|until| crate::continuum::continuum_tick_reached(server_tick, until))
}

fn rewrite_public_position(
    client: &mut crate::state::ClientHandle,
    witness: crate::state::CanonicalPositionWitness,
    state: crate::state::ServerSimMovement,
    server_tick: u32,
) {
    let wire_x = state.position_x.round().clamp(0.0, u16::MAX as f32) as u16;
    let wire_y = state.position_y.round().clamp(0.0, u16::MAX as f32) as u16;
    let wire_velocity_x = (state.velocity_x * 10.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    let wire_velocity_y = (state.velocity_y * 10.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    if let Some(position) = client
        .last_position
        .as_mut()
        .filter(|position| position.len() >= 23)
    {
        position[4..6].copy_from_slice(&wire_x.to_le_bytes());
        position[6..8].copy_from_slice(&wire_velocity_y.to_le_bytes());
        position[10..12].copy_from_slice(&wire_velocity_x.to_le_bytes());
        position[15..17].copy_from_slice(&wire_y.to_le_bytes());
        position[21..23].copy_from_slice(&client.server_energy.to_le_bytes());
    }
    let rebased = crate::state::CanonicalPositionWitness {
        // Cross-channel delivery can leave a later neutral realtime tail
        // admitted already. Rebase that current logical tail onto the exact
        // requested Warp state so the next input advances from one coherent
        // sequence anchor.
        sequence: client.last_position_sequence.unwrap_or(witness.sequence),
        life_seq: witness.life_seq,
        state,
        wire_x,
        wire_y,
        wire_velocity_x,
        wire_velocity_y,
        direction: witness.direction,
        door_seed: witness.door_seed,
        door_phase_tick: witness.door_phase_tick,
    };
    client.reset_server_movement_tracking_for_discontinuity();
    client.position_history.clear();
    client.canonical_position_witnesses.clear();
    client.canonical_position_witnesses.push_back(rebased);
    client.canonical_movement_witness = Some(rebased);
    client.server_sim_state = Some(state);
    client.server_sim_anchor_tick = Some(server_tick);
    client.last_position_server_tick = Some(server_tick);
    let motion_append = client.append_actor_motion_witness(server_tick, rebased);
    debug_assert_eq!(
        motion_append,
        Ok(()),
        "an applied Portal/Warp must seed its exact destination segment"
    );
}

/// Apply one manual Warp only after the exact request witness is the current
/// frozen tail. Terminal capacity is reserved before Portal consumption,
/// energy drain, cooldown state, or canonical movement is changed.
pub(crate) fn apply_portal_warp_at_tick(
    state: &mut ServerState,
    pending: PendingPortalWarp,
    server_tick: u32,
) -> PortalWarpOutcome {
    if !state.player_runtime_matches(pending.identity) {
        return PortalWarpOutcome::Stale;
    }
    let player_id = pending.identity.player_id();
    let request = pending.request;
    if let Some(terminal) = state.clients[&player_id]
        .portal_warp_terminals
        .iter()
        .find(|terminal| terminal.action_id == request.action_id)
        .copied()
    {
        if !crate::reliable::can_retain_application_payload(
            &state.clients[&player_id],
            PortalWarpAccepted::PACKET_LEN,
        ) {
            return PortalWarpOutcome::Deferred;
        }
        crate::reliable::send_reliable_payload(
            &mut state.clients.get_mut(&player_id).unwrap(),
            &terminal.encode(),
        );
        return PortalWarpOutcome::Terminal(terminal);
    }

    // The reliable request can arrive after one or more already-emitted
    // realtime tails on another DataChannel. Bind to the exact retained
    // canonical witness named by the keyboard edge, as placement does; the
    // successful terminal rebases movement and discards any later pre-barrier
    // tails atomically.
    let witness = state.clients[&player_id]
        .canonical_position_witnesses
        .iter()
        .rev()
        .find(|witness| {
            witness.life_seq == request.life_seq && witness.sequence == request.position_sequence
        })
        .copied();
    if witness.is_none() {
        let last = state.clients[&player_id].last_position_sequence;
        let request_still_forward = last.is_none_or(|last| {
            let advance = request.position_sequence.wrapping_sub(last);
            advance != 0 && advance < (1 << 15)
        });
        if (request_still_forward
            || !state.clients[&player_id]
                .pending_repel_observations
                .is_empty())
            && crate::continuum::continuum_tick_elapsed(server_tick, pending.queued_at_tick)
                <= PORTAL_WARP_POSITION_RECOVERY_TICKS
        {
            return PortalWarpOutcome::Deferred;
        }
    }
    if !state.clients[&player_id]
        .pending_repel_observations
        .is_empty()
    {
        return PortalWarpOutcome::Deferred;
    }
    if !crate::reliable::can_retain_application_payload(
        &state.clients[&player_id],
        PortalWarpAccepted::PACKET_LEN,
    ) {
        return PortalWarpOutcome::Deferred;
    }

    let fallback_state = state.clients[&player_id]
        .canonical_movement_witness
        .map(|witness| witness.state)
        .unwrap_or(crate::state::ServerSimMovement {
            position_x: 0.0,
            position_y: 0.0,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: state.clients[&player_id].server_energy as f32,
            max_energy: state.clients[&player_id].server_max_energy as f32,
        });
    let already_seen = state.clients[&player_id].portal_action_seen(request.action_id);
    let active_portal = state.clients[&player_id].active_portal.filter(|portal| {
        !request.force_random
            && !crate::continuum::continuum_tick_reached(server_tick, portal.expires_at_tick)
    });
    let mut rejection_reason = if already_seen {
        Some("duplicate_action")
    } else if !state.clients[&player_id].alive || state.clients[&player_id].player.ship >= 8 {
        Some("invalid_life")
    } else if witness.is_none() {
        Some("missing_position_witness")
    } else if state.clients[&player_id]
        .server_warp_settle_until_tick
        .is_some_and(|until| !crate::continuum::continuum_tick_reached(server_tick, until))
    {
        Some("warp_settle")
    } else if witness.is_some_and(|witness| {
        super::portal_placements::enemy_antiwarp_covers_witness(
            state,
            player_id,
            witness.state.position_x,
            witness.state.position_y,
        )
    }) {
        Some("enemy_antiwarp")
    } else if active_portal.is_none()
        && state.clients[&player_id].server_energy < state.clients[&player_id].server_max_energy
    {
        Some("energy_not_full")
    } else if active_portal.is_none()
        && !bomb_cooldown_ready(&state.clients[&player_id], server_tick)
    {
        Some("bomb_cooldown")
    } else {
        None
    };
    // Resolve all placement constraints before mutating inventory, energy or
    // canonical movement. A failed bounded random search is a normal rejection.
    let random_destination = if request.force_random && rejection_reason.is_none() {
        let frequency = state.clients[&player_id].player.frequency;
        let blocked_tiles = state
            .active_bricks
            .iter()
            .filter(|brick| {
                !crate::continuum::continuum_tick_reached(server_tick, brick.expires_at_tick)
                    && brick.packet.get(9..11).is_some_and(|bytes| {
                        u16::from_le_bytes(bytes.try_into().unwrap()) != frequency
                    })
            })
            .flat_map(|brick| brick.tiles.iter().copied())
            .collect();
        let mut entropy = server_tick.wrapping_add(request.action_id).to_le_bytes();
        if let Err(error) = getrandom::fill(&mut entropy) {
            tracing::warn!(event = "warp_entropy_unavailable", %error);
        }
        let mut seed = i32::from_le_bytes(entropy);
        let destination = crate::zone::server_random_warp_coordinates(
            state.zone_map(),
            frequency,
            state.clients.len() + state.bots.len(),
            crate::arena_settings::arena_ship_radius_pixels(state.clients[&player_id].player.ship),
            if state.config.free_for_all {
                crate::zone::RandomWarpArea::CentralCombat
            } else {
                crate::zone::RandomWarpArea::ArenaSettings
            },
            &mut seed,
            &blocked_tiles,
        );
        if destination.is_none() {
            rejection_reason = Some("no_clear_warp_destination");
        }
        destination
    } else {
        None
    };
    let accepted = rejection_reason.is_none();

    let (terminal, applied_position) = if accepted {
        let witness = witness.unwrap();
        let used_portal = active_portal.is_some();
        let (destination_x, destination_y) = active_portal
            .map(|portal| (portal.position_x, portal.position_y))
            .unwrap_or_else(|| {
                let (x, y) = if request.force_random {
                    random_destination.expect("accepted random warp reserved a clear destination")
                } else {
                    crate::zone::server_spawn_coordinates(state.zone_map())
                };
                (f32::from(x), f32::from(y))
            });
        let moved = {
            let dx = witness.state.position_x - destination_x;
            let dy = witness.state.position_y - destination_y;
            dx * dx + dy * dy > 1.0
        };
        let energy = if !used_portal && moved {
            1
        } else {
            state.clients[&player_id].server_energy
        };
        let canonical = crate::state::ServerSimMovement {
            position_x: destination_x,
            position_y: destination_y,
            orientation: witness.state.orientation,
            velocity_x: if used_portal {
                witness.state.velocity_x
            } else {
                0.0
            },
            velocity_y: if used_portal {
                witness.state.velocity_y
            } else {
                0.0
            },
            energy: energy as f32,
            max_energy: state.clients[&player_id].server_max_energy as f32,
        };
        let settle_until_tick = crate::continuum::continuum_tick_add(
            server_tick,
            crate::arena_settings::arena_antiwarp_settle_ticks(),
        );
        let bomb_lock_until_tick =
            crate::continuum::continuum_tick_add(server_tick, MANUAL_WARP_BOMB_LOCK_TICKS);
        let client = state.clients.get_mut(&player_id).unwrap();
        client.server_energy = energy;
        client.server_energy_remainder = 0.0;
        client.last_reported_energy = 0;
        if used_portal {
            client.active_portal = None;
        }
        client.server_warp_settle_until_tick = Some(settle_until_tick);
        client.server_warp_bomb_lock_until_tick = Some(bomb_lock_until_tick);
        rewrite_public_position(client, witness, canonical, server_tick);
        (
            terminal_for(
                request,
                server_tick,
                canonical,
                energy,
                used_portal,
                PortalWarpAccepted::OUTCOME_APPLIED,
            ),
            Some((destination_x, destination_y)),
        )
    } else {
        (
            terminal_for(
                request,
                server_tick,
                fallback_state,
                state.clients[&player_id].server_energy,
                false,
                PortalWarpAccepted::OUTCOME_REJECTED,
            ),
            None,
        )
    };
    let client = state.clients.get_mut(&player_id).unwrap();
    client.remember_portal_warp_action(terminal);
    tracing::info!(
        "zubspace_event {{\"type\":\"server_portal_warp_terminal\",\"player_id\":{},\"life_seq\":{},\"action_id\":{},\"position_sequence\":{},\"server_tick\":{},\"used_portal\":{},\"position_x\":{},\"position_y\":{},\"energy\":{},\"outcome\":{},\"reason\":\"{}\"}}",
        player_id,
        request.life_seq,
        request.action_id,
        request.position_sequence,
        server_tick,
        terminal.used_portal,
        applied_position.map_or(0.0, |position| position.0),
        applied_position.map_or(0.0, |position| position.1),
        terminal.energy,
        terminal.outcome,
        rejection_reason.unwrap_or("applied"),
    );
    crate::reliable::send_reliable_payload(client, &terminal.encode());
    PortalWarpOutcome::Terminal(terminal)
}
