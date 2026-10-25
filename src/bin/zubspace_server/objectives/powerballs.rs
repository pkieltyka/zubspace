use std::sync::{Arc, Mutex};

use super::super::security::log_security_reject;
use super::super::state::ServerState;
use super::packets::{self, powerball_position_packet};
use super::{BALL_FIRE_REQUEST, BALL_GOAL_REQUEST, BALL_PICKUP_REQUEST, PowerballState};

pub(crate) fn relay_ball_pickup(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    let Some((ball_id, expected_version)) = parse_pickup_request(request) else {
        log_security_reject(player_id, "powerball", "malformed_pickup");
        return;
    };
    let state = state.lock().expect("server state should not be poisoned");
    let Some(ball) = state.objectives.powerball(ball_id) else {
        log_security_reject(player_id, "powerball", "unknown_pickup_id");
        return;
    };
    if ball.version.get() != expected_version {
        log_security_reject(player_id, "powerball", "stale_pickup_version");
        return;
    }

    // Pickup placement, range, safety, carry, and lifecycle semantics are all
    // deferred profile policy. Even an exact current version is evidence only
    // for an existing object, not authority to mutate it.
    log_security_reject(player_id, "powerball", "pickup_policy_disabled");
}

pub(crate) fn relay_ball_fire(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    let Some(fire) = parse_fire_request(request) else {
        log_security_reject(player_id, "powerball", "malformed_fire");
        return;
    };
    if fire.client_timestamp != 0 {
        log_security_reject(player_id, "powerball", "client_fire_timestamp_nonzero");
        return;
    }
    if fire.claimed_owner != player_id {
        log_security_reject(player_id, "powerball", "forged_fire_owner");
        return;
    }
    let state = state.lock().expect("server state should not be poisoned");
    let Some(ball) = state.objectives.powerball(fire.ball_id) else {
        log_security_reject(player_id, "powerball", "unknown_fire_id");
        return;
    };
    let Some(runtime) = state.active_player_runtime_identity(player_id) else {
        log_security_reject(player_id, "powerball", "inactive_fire_owner");
        return;
    };
    if !matches!(ball.state, PowerballState::Carried { carrier } if carrier == runtime) {
        log_security_reject(player_id, "powerball", "wrong_fire_owner");
        return;
    }

    // This legacy packet has no generation, object version, player life, or
    // action sequence. Client-authored placement and velocity therefore stay
    // inert until a profile and authenticated mutation contract define them.
    log_security_reject(player_id, "powerball", "fire_policy_disabled");
}

pub(crate) fn relay_ball_goal(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    let Some(ball_id) = parse_goal_request(request) else {
        log_security_reject(player_id, "powerball", "malformed_goal");
        return;
    };
    let state = state.lock().expect("server state should not be poisoned");
    let Some(ball) = state.objectives.powerball(ball_id) else {
        log_security_reject(player_id, "powerball", "unknown_goal_id");
        return;
    };
    let Some(runtime) = state.active_player_runtime_identity(player_id) else {
        log_security_reject(player_id, "powerball", "inactive_goal_owner");
        return;
    };
    if !matches!(ball.state, PowerballState::Carried { carrier } if carrier == runtime) {
        log_security_reject(player_id, "powerball", "wrong_goal_owner");
        return;
    }
    // Goal admission, reset, reward, and win semantics require an explicit
    // server profile policy. The profile-neutral foundation deliberately does
    // not mutate the registry or score merely because the client names a real
    // carried ball.
    log_security_reject(player_id, "powerball", "goal_policy_disabled");
}

pub(crate) fn current_powerball_packets_for_state(state: &ServerState) -> Vec<Vec<u8>> {
    state
        .objectives
        .replay()
        .powerballs
        .into_iter()
        .filter_map(|ball| match ball.state {
            PowerballState::Waiting { .. } => None,
            _ => Some(powerball_packet(ball)),
        })
        .collect()
}

pub(crate) fn player_carries_powerball(
    state: &ServerState,
    carrier: super::super::authority_identity::PlayerRuntimeIdentity,
) -> bool {
    !state.objectives.carried_powerball_ids(carrier).is_empty()
}

fn powerball_packet(ball: super::registry::PowerballSnapshot) -> Vec<u8> {
    match ball.state {
        PowerballState::Waiting { .. } => packets::powerball_removal_packet(ball.identity.id),
        PowerballState::OnMap {
            x,
            y,
            velocity_x,
            velocity_y,
            ..
        } => powerball_position_packet(
            ball.identity.id,
            x,
            y,
            velocity_x,
            velocity_y,
            None,
            ball.version.get(),
        ),
        PowerballState::Carried { carrier } => {
            powerball_position_packet(ball.identity.id, 0, 0, 0, 0, Some(carrier.player_id()), 0)
        }
    }
}

fn parse_pickup_request(request: &[u8]) -> Option<(u8, u32)> {
    if request.len() != 6 || request[0] != BALL_PICKUP_REQUEST {
        return None;
    }
    let version = u32::from_le_bytes(request[2..6].try_into().ok()?);
    if version == 0 || version > crate::continuum::CONTINUUM_TICK_MASK {
        return None;
    }
    Some((request[1], version))
}

#[derive(Clone, Copy)]
struct FireRequest {
    ball_id: u8,
    claimed_owner: u16,
    client_timestamp: u32,
}

fn parse_fire_request(request: &[u8]) -> Option<FireRequest> {
    if request.len() != 16 || request[0] != BALL_FIRE_REQUEST {
        return None;
    }
    Some(FireRequest {
        ball_id: request[1],
        claimed_owner: u16::from_le_bytes(request[10..12].try_into().ok()?),
        client_timestamp: u32::from_le_bytes(request[12..16].try_into().ok()?),
    })
}

fn parse_goal_request(request: &[u8]) -> Option<u8> {
    (request.len() == 6 && request[0] == BALL_GOAL_REQUEST).then_some(request[1])
}
