use std::sync::{Arc, Mutex};

use super::super::reliable::send_reliable_to;
use super::super::state::ServerState;

pub(crate) const SPECTATE_REQUEST: u8 = 0x08;
const SPECTATE_PACKET: u8 = 0x1c;

pub(crate) fn update_spectate(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    if matches!(
        request.len(),
        crate::spectator_view::SpectatorViewRequest::LEN
            | crate::spectator_view::SpectatorViewRequest::LEGACY_LEN
    ) {
        update_observer_view(state, player_id, request);
        return;
    }
    if request.len() != 3 || request[0] != SPECTATE_REQUEST {
        return;
    }
    let target_id = u16::from_le_bytes([request[1], request[2]]);
    if !spectate_request_allowed(state, player_id, target_id) {
        return;
    }
    send_reliable_to(state, player_id, spectate_packet(target_id != u16::MAX));
}

pub(crate) fn spectate_packet(extra_position_info: bool) -> Vec<u8> {
    vec![SPECTATE_PACKET, u8::from(extra_position_info)]
}

fn spectate_request_allowed(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    target_id: u16,
) -> bool {
    let state = state.lock().expect("server state should not be poisoned");
    if !state
        .clients
        .get(&player_id)
        .is_some_and(|client| client.joined)
    {
        return false;
    }
    if target_id == u16::MAX {
        return true;
    }
    if target_id == player_id {
        return false;
    }
    state
        .clients
        .get(&target_id)
        .is_some_and(|client| client.joined && client.alive && client.player.ship < 8)
}

fn update_observer_view(shared: &Arc<Mutex<ServerState>>, player_id: u16, bytes: &[u8]) {
    let Some(view) = crate::spectator_view::SpectatorViewRequest::decode(bytes) else {
        return;
    };
    let mut state = shared.lock().expect("server state should not be poisoned");
    let Some(viewer) = state.clients.get(&player_id) else {
        return;
    };
    let limit = (crate::zone::MAP_TILES as f32) * crate::zone::TILE_SIZE_PX;
    if !viewer.joined
        || !viewer.snapshot_ready
        || viewer.player.ship != 8
        || viewer.last_security_valid != Some(true)
        || view.server_session_id != crate::wire::server_session_id()
        || view.arena_stream_id != viewer.arena_stream_id
        || view.life_seq != viewer.life_seq
        || view.x < 0.0
        || view.x >= limit
        || view.y < 0.0
        || view.y >= limit
        || viewer.spectator_view_sequence.is_some_and(|old| {
            let delta = view.sequence.wrapping_sub(old);
            delta == 0 || delta >= 0x8000
        })
    {
        return;
    }
    let anchor = if view.target == u16::MAX {
        let mut anchor = vec![0; 23];
        anchor[0] = crate::arena::LARGE_POSITION_PACKET;
        anchor[4..6].copy_from_slice(&(view.x.floor() as u16).to_le_bytes());
        anchor[8..10].copy_from_slice(&player_id.to_le_bytes());
        anchor[15..17].copy_from_slice(&(view.y.floor() as u16).to_le_bytes());
        Some(anchor)
    } else if view.target == player_id {
        None
    } else if let Some(target) = state
        .clients
        .get(&view.target)
        .filter(|target| target.joined && target.player.ship < 8)
    {
        target
            .last_position
            .clone()
            .or_else(|| target.aoi_observer_position.clone())
    } else {
        state
            .bots
            .iter()
            .find(|target| target.player.id == view.target && target.player.ship < 8)
            .map(|bot| bot.large_position_packet())
    };
    let Some(anchor) = anchor else {
        return;
    };
    // This is exclusively an observer anchor. Never install last_position,
    // update the visibility index, revive a life, or touch combat witnesses.
    let viewer = state.clients.get_mut(&player_id).unwrap();
    viewer.aoi_observer_position = Some(anchor);
    viewer.spectator_view_sequence = Some(view.sequence);
    if let Some(sequence) = view.next_input_sequence {
        viewer.spectator_next_input_sequence = Some(sequence);
    }
}
