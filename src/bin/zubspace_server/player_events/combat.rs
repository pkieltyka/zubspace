use super::super::arena_settings::{
    arena_prize_negative_factor, arena_prize_weight_total, arena_prize_weights,
};
#[cfg(test)]
use super::super::hidden_prizes::create_death_collectible;
#[cfg(test)]
use super::super::identity::persist_stats_for_client;
#[cfg(test)]
use super::super::objectives::{
    ObjectiveLifecycleReason, objective_player, player_lifecycle_packets_for_context,
    remove_player_turret_link_packets,
};
#[cfg(test)]
use super::super::reliable::send_reliable_payload;
#[cfg(test)]
use super::super::roster::score_update;
use super::super::state::ServerState;
#[cfg(test)]
use super::super::zone::large_position_packet_in_safe_tile_for_map;
use crate::authority_prizes::generate_weighted_prize_id;
#[cfg(test)]
use std::sync::{Arc, Mutex};

const DEATH_PRIZE_INITIAL_RNG_SEED: i32 = 0x6770_656e_u32 as i32;

pub(crate) const DEATH_REQUEST: u8 = 0x05;
#[cfg(test)]
pub(crate) const PLAYER_DEATH: u8 = 0x06;
pub(crate) const TOGGLE_SEND_DAMAGE: u8 = 0x37;
#[cfg(test)]
pub(crate) fn relay_death(state: &Arc<Mutex<ServerState>>, killed_id: u16, payload: &[u8]) {
    if payload.len() < 5 {
        return;
    }
    let killer_id = u16::from_le_bytes([payload[1], payload[2]]);
    let tick = crate::wire::current_tick();
    let mut state = state.lock().expect("server state should not be poisoned");
    let map = state.zone_map();
    let mut score_packets = Vec::new();
    let mut powerball_packets = Vec::new();
    let mut turret_packets = Vec::new();
    let (bounty, death_motion) = {
        let Some(killed) = state.clients.get(&killed_id) else {
            return;
        };
        if !killed.joined || !killed.alive {
            return;
        }
        let Some(last_position) = killed.last_position.as_ref() else {
            return;
        };
        if large_position_packet_in_safe_tile_for_map(map, last_position) {
            return;
        }
        if !death_request_killer_valid(&state, killed_id, killer_id) {
            return;
        }
        if killer_id != killed_id {
            return;
        }
        (
            killed.server_bounty,
            crate::arena::large_position_coordinates_and_velocity(last_position),
        )
    };
    let rolled_green_id = roll_death_green_prize_id(&mut state);
    let (green_id, green_collectible_id) = death_motion
        .filter(|(_, (velocity_x, velocity_y))| {
            rolled_green_id > 0 && (*velocity_x != 0 || *velocity_y != 0)
        })
        .and_then(|((x, y), _)| {
            create_death_collectible(
                &mut state,
                x,
                y,
                rolled_green_id as i16,
                tick,
                crate::arena_settings::arena_death_prize_lifetime_ticks(),
            )
            .map(|collectible_id| (rolled_green_id, collectible_id))
        })
        .unwrap_or((-1, 0));
    if let Some(objective_player) = objective_player(&state, killed_id) {
        let Ok(packets) = player_lifecycle_packets_for_context(
            &mut state,
            objective_player,
            Some(killer_id),
            ObjectiveLifecycleReason::Death { killer: None },
            tick,
        ) else {
            tracing::error!(
                killed_id,
                "player death retained: objective cleanup could not commit"
            );
            return;
        };
        powerball_packets.extend(packets);
    }
    let respawn_at_tick;
    {
        let Some(killed) = state.clients.get_mut(&killed_id) else {
            return;
        };
        if !killed.joined || !killed.alive {
            return;
        }
        killed.alive = false;
        killed.respawn_at_tick = Some(crate::continuum::continuum_tick_add(
            crate::wire::current_tick(),
            crate::arena_settings::arena_respawn_delay_ticks(),
        ));
        respawn_at_tick = killed
            .respawn_at_tick
            .expect("death always installs a respawn deadline");
        killed.player.losses = killed.player.losses.saturating_add(1);
        killed.player.flags = 0;
        killed.clear_server_runtime();
        score_packets.push(score_update(&killed.player));
    }
    let Some(life) = state.advance_client_life(killed_id) else {
        return;
    };
    let packet = player_death(
        killer_id,
        killed_id,
        bounty,
        0,
        green_id,
        green_collectible_id,
        life.life_seq,
        respawn_at_tick,
    );
    state.clear_client_position_preserving_aoi_observer(killed_id);
    state.remove_projectiles_for_player(killed_id);
    turret_packets.extend(remove_player_turret_link_packets(&mut state, killed_id));
    persist_stats_for_client(&mut state, killed_id);

    if killer_id != killed_id {
        if let Some(killer) = state.clients.get_mut(&killer_id) {
            killer.player.wins = killer.player.wins.saturating_add(1);
            killer.player.kill_points = killer.player.kill_points.saturating_add(u32::from(bounty));
            score_packets.push(score_update(&killer.player));
        }
        persist_stats_for_client(&mut state, killer_id);
    }

    for client in state.clients.values_mut().filter(|client| client.joined) {
        send_reliable_payload(client, &packet);
        for powerball_packet in &powerball_packets {
            send_reliable_payload(client, powerball_packet);
        }
        for turret_packet in &turret_packets {
            send_reliable_payload(client, turret_packet);
        }
        for score_packet in &score_packets {
            send_reliable_payload(client, score_packet);
        }
    }
}

#[cfg(test)]
pub(crate) fn player_death(
    killer_id: u16,
    killed_id: u16,
    bounty: u16,
    flag_transfer: u16,
    green_id: i8,
    green_collectible_id: u32,
    life_seq: u32,
    respawn_at_tick: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(22);
    payload.push(PLAYER_DEATH);
    payload.push(green_id as u8);
    payload.extend(killer_id.to_le_bytes());
    payload.extend(killed_id.to_le_bytes());
    payload.extend(bounty.to_le_bytes());
    payload.extend(flag_transfer.to_le_bytes());
    payload.extend(green_collectible_id.to_le_bytes());
    payload.extend(life_seq.to_le_bytes());
    payload.extend(respawn_at_tick.to_le_bytes());
    payload
}

/// Roll a positive prize id from the arena's weighted prize table for a
/// death green. Returns -1 (no green) if the prize table is empty. Mirrors
/// the reference game's `OnPlayerDeathPkt`/`SpawnDeathGreen` expectation that the
/// PLAYER_DEATH packet's green_id byte is a positive prize id when a green
/// should drop, or -1 (0xff) when none should.
pub(crate) fn roll_death_green_prize_id(state: &mut ServerState) -> i8 {
    if state.death_prize_rng_seed == 0 {
        state.death_prize_rng_seed = DEATH_PRIZE_INITIAL_RNG_SEED;
    }
    let weights = arena_prize_weights();
    let weight_total = arena_prize_weight_total();
    match generate_weighted_prize_id(
        &weights,
        weight_total,
        arena_prize_negative_factor(),
        false,
        &mut state.death_prize_rng_seed,
    ) {
        Some((prize_id, _)) => prize_id.clamp(1, i8::MAX as i16) as i8,
        None => -1,
    }
}

pub(crate) fn toggle_send_damage(enabled: bool) -> Vec<u8> {
    vec![TOGGLE_SEND_DAMAGE, u8::from(enabled)]
}

#[cfg(test)]
fn death_request_killer_valid(state: &ServerState, killed_id: u16, killer_id: u16) -> bool {
    if killer_id == killed_id {
        return true;
    }
    state
        .clients
        .get(&killer_id)
        .is_some_and(|client| client.joined && client.alive && client.player.ship < 8)
}
