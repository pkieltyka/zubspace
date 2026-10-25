use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::arena_settings::{arena_ship_attach_bounty, arena_ship_turret_limit};
use super::super::rate_limit::allow_short_burst;
use super::super::reliable::broadcast_reliable_to_joined;
use super::super::state::{ClientHandle, ServerState};
use super::ATTACH_REQUEST;
use super::packets::{create_turret_link, destroy_turret_link};

const ATTACH_RATE_WINDOW: Duration = Duration::from_millis(250);
const ATTACH_BURST_LIMIT: u8 = 3;

#[cfg(test)]
pub(crate) fn relay_attach_request(
    state: &Arc<Mutex<ServerState>>,
    requester_id: u16,
    request: &[u8],
) {
    relay_attach_request_for_runtime(state, None, requester_id, request);
}

pub(crate) fn relay_attach_request_for_runtime(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<super::super::authority_identity::PlayerRuntimeIdentity>,
    requester_id: u16,
    request: &[u8],
) {
    let Some(packet) = create_turret_from_request(state, expected_identity, requester_id, request)
    else {
        return;
    };
    broadcast_reliable_to_joined(state, packet);
}

#[cfg(test)]
pub(crate) fn relay_drop_attached(state: &Arc<Mutex<ServerState>>, parent_id: u16) {
    relay_drop_attached_for_runtime(state, None, parent_id);
}

pub(crate) fn relay_drop_attached_for_runtime(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<super::super::authority_identity::PlayerRuntimeIdentity>,
    parent_id: u16,
) {
    let had_children = {
        let mut state = state.lock().expect("server state should not be poisoned");
        if expected_identity.is_some()
            && state.active_player_runtime_identity(parent_id) != expected_identity
        {
            return;
        }
        if !state
            .clients
            .get(&parent_id)
            .is_some_and(|client| client.joined)
        {
            return;
        }
        let before = state.turret_links.len();
        state
            .turret_links
            .retain(|_, linked_parent| *linked_parent != parent_id);
        state.turret_links.len() != before
    };
    if had_children {
        broadcast_reliable_to_joined(state, destroy_turret_link(parent_id));
    }
}

#[cfg(test)]
pub(crate) fn current_turret_packets(state: &Arc<Mutex<ServerState>>) -> Vec<Vec<u8>> {
    let state = state.lock().expect("server state should not be poisoned");
    current_turret_packets_for_state(&state)
}

pub(crate) fn current_turret_packets_for_state(state: &ServerState) -> Vec<Vec<u8>> {
    state
        .turret_links
        .iter()
        .map(|(&requester_id, &destination_id)| create_turret_link(requester_id, destination_id))
        .collect()
}

pub(crate) fn clear_player_turret_links(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let _ = remove_player_turret_link_packets(&mut state, player_id);
}

pub(crate) fn remove_player_turret_link_packets(
    state: &mut ServerState,
    player_id: u16,
) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    if state.turret_links.remove(&player_id).is_some() {
        packets.push(create_turret_link(player_id, u16::MAX));
    }
    let before = state.turret_links.len();
    state
        .turret_links
        .retain(|_, parent_id| *parent_id != player_id);
    if state.turret_links.len() != before {
        packets.push(destroy_turret_link(player_id));
    }
    packets
}

fn create_turret_from_request(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<super::super::authority_identity::PlayerRuntimeIdentity>,
    requester_id: u16,
    request: &[u8],
) -> Option<Vec<u8>> {
    if request.len() < 3 || request[0] != ATTACH_REQUEST {
        return None;
    }

    let destination_id = u16::from_le_bytes(request[1..3].try_into().ok()?);
    if destination_id == u16::MAX {
        let mut state = state.lock().expect("server state should not be poisoned");
        if expected_identity.is_some()
            && state.active_player_runtime_identity(requester_id) != expected_identity
        {
            return None;
        }
        let client = state.clients.get(&requester_id)?;
        if !client.joined {
            return None;
        }
        if !state.turret_links.contains_key(&requester_id) {
            return None;
        }
        let client = state.clients.get_mut(&requester_id)?;
        if !allow_attach_request_burst(client, Instant::now()) {
            return None;
        }
        state.turret_links.remove(&requester_id);
        return Some(create_turret_link(requester_id, u16::MAX));
    }

    let mut state = state.lock().expect("server state should not be poisoned");
    if expected_identity.is_some()
        && state.active_player_runtime_identity(requester_id) != expected_identity
    {
        return None;
    }
    let (requester_joined, requester_alive, requester_ship, requester_frequency, requester_bounty) = {
        let requester = state.clients.get(&requester_id)?;
        (
            requester.joined,
            requester.alive,
            requester.player.ship,
            requester.player.frequency,
            requester.server_bounty,
        )
    };
    let (destination_joined, destination_alive, destination_ship, destination_frequency) = {
        let destination = state.clients.get(&destination_id)?;
        (
            destination.joined,
            destination.alive,
            destination.player.ship,
            destination.player.frequency,
        )
    };
    if !requester_joined
        || !requester_alive
        || !destination_joined
        || !destination_alive
        || requester_id == destination_id
        || requester_ship >= 8
        || destination_ship >= 8
        || requester_frequency != destination_frequency
    {
        return None;
    }

    if state.turret_links.get(&requester_id) == Some(&destination_id) {
        return None;
    }

    // The reference game's `PlayerManager:1314-1345` rejects attaches when:
    //   * requester bounty is below the requester ship's AttachBounty,
    //   * destination ship's TurretLimit is 0 or already at capacity, or
    //   * requester already has children attached (no chaining).
    let attach_bounty = arena_ship_attach_bounty(requester_ship);
    if (requester_bounty as u32) < attach_bounty as u32 {
        return None;
    }
    let turret_limit = arena_ship_turret_limit(destination_ship);
    if turret_limit == 0 {
        return None;
    }
    let existing_riders = state
        .turret_links
        .values()
        .filter(|&&parent| parent == destination_id)
        .count();
    if existing_riders >= turret_limit as usize {
        return None;
    }
    let requester_has_children = state
        .turret_links
        .values()
        .any(|&parent| parent == requester_id);
    if requester_has_children {
        return None;
    }
    let requester = state.clients.get_mut(&requester_id)?;
    if !allow_attach_request_burst(requester, Instant::now()) {
        return None;
    }
    state.turret_links.insert(requester_id, destination_id);
    Some(create_turret_link(requester_id, destination_id))
}

fn allow_attach_request_burst(client: &mut ClientHandle, now: Instant) -> bool {
    allow_short_burst(
        &mut client.last_attach_request_at,
        &mut client.attach_request_burst_count,
        now,
        ATTACH_RATE_WINDOW,
        ATTACH_BURST_LIMIT,
    )
}
