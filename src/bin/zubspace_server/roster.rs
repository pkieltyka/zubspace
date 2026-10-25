use std::sync::{Arc, Mutex};

use super::reliable::send_reliable_payload;
use super::state::{PLAYER_BANNER_BYTES, PlayerSummary, ServerState};
use super::wire::copy_fixed;
use super::zone::server_zone_maps;

pub(crate) const SET_BANNER_REQUEST: u8 = 0x19;
pub(crate) const PERSONAL_BANNER_CHANGE: u8 = 0x1e;
pub(crate) const SHIP_FREQUENCY_CHANGE: u8 = 0x1d;
pub(crate) const FREQUENCY_CUTOVER_ACK: u8 = 0x74;
pub(crate) const LIFECYCLE_BASELINE_ACCEPTED: u8 = 0x75;
pub(crate) const WARP_RELOCATION_ACCEPTED: u8 = 0x77;
pub(crate) const REPEL_APPLICATION_ACCEPTED: u8 = 0x7a;
pub(crate) const PLAYER_BANNER_CHANGE: u8 = 0x1f;
pub(crate) const ARENA_DIRECTORY: u8 = 0x2f;
pub(crate) const SCORE_UPDATE: u8 = 0x09;

pub(crate) fn player_id_assignment(player_id: u16) -> Vec<u8> {
    let mut payload = vec![0x01];
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(crate) fn player_leaving(player_id: u16) -> Vec<u8> {
    let mut payload = vec![0x04];
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(crate) fn ship_frequency_change(player: &PlayerSummary, life_seq: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(10);
    payload.push(SHIP_FREQUENCY_CHANGE);
    payload.push(player.ship);
    payload.extend(player.id.to_le_bytes());
    payload.extend(player.frequency.to_le_bytes());
    payload.extend(life_seq.to_le_bytes());
    payload
}

pub(crate) fn frequency_cutover_ack(
    frequency: u16,
    life_seq: u32,
    baseline: crate::continuum_position::ClientPositionSnapshot,
) -> Vec<u8> {
    let mut payload =
        Vec::with_capacity(23 + crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_LEN);
    payload.push(FREQUENCY_CUTOVER_ACK);
    payload.extend(frequency.to_le_bytes());
    payload.extend(
        baseline
            .client_sequence
            .expect("frequency baseline always has a sequence")
            .to_le_bytes(),
    );
    payload.extend(life_seq.to_le_bytes());
    payload.push(baseline.direction);
    payload.extend(baseline.x.to_le_bytes());
    payload.extend(baseline.y.to_le_bytes());
    payload.extend(baseline.velocity_x.to_le_bytes());
    payload.extend(baseline.velocity_y.to_le_bytes());
    payload.push(baseline.togglables);
    payload.extend(baseline.bounty.to_le_bytes());
    payload.extend(baseline.energy.to_le_bytes());
    baseline.canonical_state.encode(&mut payload);
    payload
}

pub(crate) fn lifecycle_baseline_accepted(
    client_reliable_delivery_id: u32,
    life_seq: u32,
    position_sequence: u16,
    accepted_server_tick: u32,
    accepted_position: (u16, u16),
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(20);
    payload.push(LIFECYCLE_BASELINE_ACCEPTED);
    payload.push(3);
    payload.extend(client_reliable_delivery_id.to_le_bytes());
    payload.extend(life_seq.to_le_bytes());
    payload.extend(position_sequence.to_le_bytes());
    payload.extend(accepted_server_tick.to_le_bytes());
    payload.extend(accepted_position.0.to_le_bytes());
    payload.extend(accepted_position.1.to_le_bytes());
    payload
}

pub(crate) fn warp_relocation_accepted(
    collectible_id: u32,
    life_seq: u32,
    position_sequence: u16,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(11);
    payload.push(WARP_RELOCATION_ACCEPTED);
    payload.extend(collectible_id.to_le_bytes());
    payload.extend(life_seq.to_le_bytes());
    payload.extend(position_sequence.to_le_bytes());
    payload
}

pub(crate) fn repel_application_accepted(
    server_session_id: u64,
    life_seq: u32,
    delivery_generation: u32,
    snapshot_id: u16,
    application_sequence: u16,
    accepted_end_sequence: u16,
    active_steps: u16,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(26);
    payload.push(REPEL_APPLICATION_ACCEPTED);
    payload.push(3);
    payload.extend(server_session_id.to_le_bytes());
    payload.extend(life_seq.to_le_bytes());
    payload.extend(delivery_generation.to_le_bytes());
    payload.extend(snapshot_id.to_le_bytes());
    payload.extend(application_sequence.to_le_bytes());
    payload.extend(accepted_end_sequence.to_le_bytes());
    payload.extend(active_steps.to_le_bytes());
    payload
}

pub(crate) fn score_update(player: &PlayerSummary) -> Vec<u8> {
    let mut payload = Vec::with_capacity(20);
    payload.push(SCORE_UPDATE);
    payload.extend(player.id.to_le_bytes());
    payload.extend(player.kill_points.to_le_bytes());
    payload.extend(player.flag_points.to_le_bytes());
    payload.extend(player.wins.to_le_bytes());
    payload.extend(player.losses.to_le_bytes());
    payload.push(1); // zubspace assist extension; the classic 15-byte prefix stays intact.
    payload.extend(player.assists.to_le_bytes());
    payload
}

#[cfg(test)]
pub(crate) fn arena_directory_listing(state: &Arc<Mutex<ServerState>>) -> Vec<u8> {
    let state = state.lock().expect("server state should not be poisoned");
    arena_directory_listing_for_state(&state)
}

pub(crate) fn arena_directory_listing_for_state(state: &ServerState) -> Vec<u8> {
    let active_zone_id = state.zone_map().zone_id.as_ref();
    let population = state
        .clients
        .values()
        .filter(|client| client.joined)
        .count()
        .min(u16::MAX as usize) as u16;
    let mut payload = Vec::with_capacity(1 + server_zone_maps().len() * 18);
    payload.push(ARENA_DIRECTORY);

    for map in server_zone_maps() {
        if map.zone_id == active_zone_id {
            append_arena_directory_entry(&mut payload, map.display_name.as_ref(), population);
        }
    }
    for map in server_zone_maps() {
        if map.zone_id != active_zone_id {
            append_arena_directory_entry(&mut payload, map.display_name.as_ref(), 0);
        }
    }

    payload
}

fn append_arena_directory_entry(payload: &mut Vec<u8>, name: &str, population: u16) {
    payload.extend(name.as_bytes());
    payload.push(0);
    payload.extend(population.to_le_bytes());
}

pub(crate) fn player_entering(player: &PlayerSummary) -> Vec<u8> {
    let mut payload = vec![0; 64];
    payload[0] = 0x03;
    payload[1] = player.ship;
    copy_fixed(&mut payload[3..23], player.name.as_bytes());
    copy_fixed(&mut payload[23..43], b"zubspace");
    payload[43..47].copy_from_slice(&player.kill_points.to_le_bytes());
    payload[47..51].copy_from_slice(&player.flag_points.to_le_bytes());
    payload[51..53].copy_from_slice(&player.id.to_le_bytes());
    payload[53..55].copy_from_slice(&player.frequency.to_le_bytes());
    payload[55..57].copy_from_slice(&player.wins.to_le_bytes());
    payload[57..59].copy_from_slice(&player.losses.to_le_bytes());
    payload[59..61].copy_from_slice(&0xffff_u16.to_le_bytes());
    payload[61..63].copy_from_slice(&player.flags.to_le_bytes());
    if player.assists != 0 {
        // Existing roster-tail framing lets late joiners receive assists too,
        // while keeping real kill points in the classic 64-byte roster.
        payload.extend(score_update(player));
    }
    payload
}

pub(crate) fn update_banner(state: &Arc<Mutex<ServerState>>, player_id: u16, payload: &[u8]) {
    let Some(banner) = parse_banner_request(payload) else {
        return;
    };

    let personal_change = personal_banner_change(&banner);
    let remote_change = player_banner_change(player_id, &banner);
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(client) = state.clients.get_mut(&player_id) else {
        return;
    };
    if !client.joined {
        return;
    }

    client.player.banner = banner;
    send_reliable_payload(client, &personal_change);

    for (&id, client) in &mut state.clients {
        if id == player_id || !client.joined {
            continue;
        }
        send_reliable_payload(client, &remote_change);
    }
}

fn parse_banner_request(payload: &[u8]) -> Option<[u8; PLAYER_BANNER_BYTES]> {
    if payload.len() < 1 + PLAYER_BANNER_BYTES || payload.first().copied()? != SET_BANNER_REQUEST {
        return None;
    }

    let mut banner = [0; PLAYER_BANNER_BYTES];
    banner.copy_from_slice(&payload[1..1 + PLAYER_BANNER_BYTES]);
    Some(banner)
}

pub(crate) fn personal_banner_change(banner: &[u8; PLAYER_BANNER_BYTES]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + PLAYER_BANNER_BYTES);
    payload.push(PERSONAL_BANNER_CHANGE);
    payload.extend_from_slice(banner);
    payload
}

pub(crate) fn player_banner_change(player_id: u16, banner: &[u8; PLAYER_BANNER_BYTES]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3 + PLAYER_BANNER_BYTES);
    payload.push(PLAYER_BANNER_CHANGE);
    payload.extend(player_id.to_le_bytes());
    payload.extend_from_slice(banner);
    payload
}

pub(crate) fn has_banner(banner: &[u8; PLAYER_BANNER_BYTES]) -> bool {
    banner.iter().any(|&pixel| pixel != 0)
}
