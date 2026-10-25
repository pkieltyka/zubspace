use crate::authority::POSITION_STATUS_ANTIWARP;
use crate::portal_placement::{PortalPlacementAccepted, PortalPlacementRequest};
use crate::state::{PendingPortalPlacement, ServerPortal, ServerState};

const PORTAL_POSITION_RECOVERY_TICKS: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortalPlacementOutcome {
    Deferred,
    Stale,
    Terminal(PortalPlacementAccepted),
}

pub(super) fn enemy_antiwarp_covers_witness(
    state: &ServerState,
    player_id: u16,
    x: f32,
    y: f32,
) -> bool {
    let Some(source) = state.clients.get(&player_id) else {
        return false;
    };
    let radius = crate::arena_settings::arena_antiwarp_radius_pixels();
    if radius <= 0.0 {
        return false;
    }
    let radius_squared = radius * radius;
    state.clients.iter().any(|(&other_id, other)| {
        if other_id == player_id
            || !other.joined
            || !other.alive
            || other.player.ship >= 8
            || other.player.frequency == source.player.frequency
        {
            return false;
        }
        let Some(position) = other.last_position.as_deref() else {
            return false;
        };
        let Some((other_x, other_y)) = crate::arena::large_position_coordinates(position) else {
            return false;
        };
        if position.get(13).copied().unwrap_or_default() & POSITION_STATUS_ANTIWARP == 0 {
            return false;
        }
        let dx = f32::from(other_x) - x;
        let dy = f32::from(other_y) - y;
        dx * dx + dy * dy <= radius_squared
    })
}

fn terminal_for(
    request: PortalPlacementRequest,
    revision: u32,
    portals: u8,
    outcome: u8,
    position_x: f32,
    position_y: f32,
    placed_at_tick: u32,
    expires_at_tick: u32,
) -> PortalPlacementAccepted {
    PortalPlacementAccepted {
        server_session_id: request.server_session_id,
        arena_stream_id: request.arena_stream_id,
        player_id: request.player_id,
        life_seq: request.life_seq,
        action_id: request.action_id,
        position_sequence: request.position_sequence,
        prize_state_revision: revision.max(1),
        portals,
        outcome,
        position_x_bits: position_x.to_bits(),
        position_y_bits: position_y.to_bits(),
        placed_at_tick,
        expires_at_tick,
    }
}

/// Commit one Portal inventory transaction only after its exact post-movement
/// canonical witness exists. Capacity is reserved before decrement/revision;
/// a full private reliable queue leaves the immutable request deferred.
pub(crate) fn apply_portal_placement_at_tick(
    state: &mut ServerState,
    pending: PendingPortalPlacement,
    server_tick: u32,
) -> PortalPlacementOutcome {
    if !state.player_runtime_matches(pending.identity) {
        return PortalPlacementOutcome::Stale;
    }
    let player_id = pending.identity.player_id();
    let request = pending.request;
    if let Some(terminal) = state.clients[&player_id]
        .portal_placement_terminals
        .iter()
        .find(|terminal| terminal.action_id == request.action_id)
        .copied()
    {
        if !crate::reliable::can_retain_application_payload(
            &state.clients[&player_id],
            PortalPlacementAccepted::PACKET_LEN,
        ) {
            return PortalPlacementOutcome::Deferred;
        }
        crate::reliable::send_reliable_payload(
            state
                .clients
                .get_mut(&player_id)
                .expect("active Portal client"),
            &terminal.encode(),
        );
        return PortalPlacementOutcome::Terminal(terminal);
    }
    if state.clients[&player_id].portal_action_seen(request.action_id) {
        if !crate::reliable::can_retain_application_payload(
            &state.clients[&player_id],
            PortalPlacementAccepted::PACKET_LEN,
        ) {
            return PortalPlacementOutcome::Deferred;
        }
        let client = state
            .clients
            .get_mut(&player_id)
            .expect("matching duplicate Portal runtime");
        let terminal = terminal_for(
            request,
            client.server_prize_state_revision,
            client.server_weapons.portals,
            PortalPlacementAccepted::OUTCOME_REJECTED,
            0.0,
            0.0,
            server_tick,
            server_tick,
        );
        crate::reliable::send_reliable_payload(client, &terminal.encode());
        return PortalPlacementOutcome::Terminal(terminal);
    }

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
        if request_still_forward
            && crate::continuum::continuum_tick_elapsed(server_tick, pending.queued_at_tick)
                <= PORTAL_POSITION_RECOVERY_TICKS
        {
            return PortalPlacementOutcome::Deferred;
        }
    }
    if !crate::reliable::can_retain_application_payload(
        &state.clients[&player_id],
        PortalPlacementAccepted::PACKET_LEN,
    ) {
        return PortalPlacementOutcome::Deferred;
    }

    let accepted = witness.is_some_and(|witness| {
        let client = &state.clients[&player_id];
        client.alive
            && client.player.ship < 8
            && client.server_weapons.portals > 0
            && client
                .server_warp_settle_until_tick
                .is_none_or(|until| crate::continuum::continuum_tick_reached(server_tick, until))
            && !enemy_antiwarp_covers_witness(
                state,
                player_id,
                witness.state.position_x,
                witness.state.position_y,
            )
    });
    let (position_x, position_y) = witness
        .map(|witness| (witness.state.position_x, witness.state.position_y))
        .unwrap_or((0.0, 0.0));
    let expires_at_tick = crate::continuum::continuum_tick_add(
        server_tick,
        crate::arena_settings::arena_portal_lifetime_ticks(),
    );
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("matching Portal runtime must remain active");
    if accepted {
        client.server_weapons.portals -= 1;
        client.advance_server_prize_state_revision();
        client.active_portal = Some(ServerPortal {
            position_x,
            position_y,
            placed_at_tick: server_tick,
            expires_at_tick,
            placement_action_id: request.action_id,
        });
    }
    let terminal = terminal_for(
        request,
        client.server_prize_state_revision,
        client.server_weapons.portals,
        if accepted {
            PortalPlacementAccepted::OUTCOME_APPLIED
        } else {
            PortalPlacementAccepted::OUTCOME_REJECTED
        },
        position_x,
        position_y,
        server_tick,
        expires_at_tick,
    );
    client.remember_portal_action(terminal);
    tracing::info!(
        "zubspace_event {{\"type\":\"server_portal_placement_terminal\",\"player_id\":{},\"life_seq\":{},\"action_id\":{},\"position_sequence\":{},\"server_tick\":{},\"prize_state_revision\":{},\"portals_remaining\":{},\"position_x\":{},\"position_y\":{},\"expires_at_tick\":{},\"outcome\":{}}}",
        player_id,
        request.life_seq,
        request.action_id,
        request.position_sequence,
        server_tick,
        terminal.prize_state_revision,
        terminal.portals,
        position_x,
        position_y,
        expires_at_tick,
        terminal.outcome,
    );
    crate::reliable::send_reliable_payload(client, &terminal.encode());
    PortalPlacementOutcome::Terminal(terminal)
}
