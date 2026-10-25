use macroquad::prelude::*;

use crate::authority_prizes::{
    SECURITY_PRIZE_SEED, generate_weighted_prize_id, initial_bounty_prize_ids,
    multiprize_sub_prize_ids,
};
#[cfg(test)]
use crate::authority_prizes::{initial_bounty_prize_allowed_id, multiprize_sub_prize_allowed_id};
use crate::tile::tile_center;
use crate::vie::vie_rng_next;
use crate::{
    ArenaMap, CONTINUUM_MAX_GREEN_COUNT, CONTINUUM_PRIZE_WEIGHT_COUNT, NetworkPlayer, NetworkWorld,
    PlayerDeath, PrizeKind, PrizePickup, TILE_SIZE,
};

pub(crate) const DAMAGE_PRIZE_LOSS_REROLL_LIMIT: usize = 9999;

pub(crate) fn max_hidden_prize_count(prize_factor: u16, player_count: usize) -> usize {
    ((prize_factor as usize).saturating_mul(player_count) / 1000).min(CONTINUUM_MAX_GREEN_COUNT)
}

pub(crate) fn damage_can_remove_prize(
    bounty: u32,
    energy: f32,
    energy_max: f32,
    damage_factor: u16,
) -> bool {
    bounty > 0 && energy < energy_max && damage_factor != 0
}

pub(crate) fn damage_prize_loss_chance(damage_factor: u16, damage: f32) -> u32 {
    if damage <= 0.0 {
        return u32::MAX;
    }
    (((damage_factor as f32 * 200_000.0) / (damage * 1000.0)) as u32).saturating_add(1)
}

pub(crate) fn damage_prize_loss_id_allowed(prize_id: i16) -> bool {
    crate::gameplay_rules::prize_eligible(
        crate::gameplay_rules::PrizeEligibilityContext::DamageLoss,
        prize_id,
    )
}

#[allow(dead_code)]
pub(crate) fn remove_prize_at_protocol_position(
    prizes: &mut Vec<PrizePickup>,
    x: u16,
    y: u16,
) -> bool {
    let Some(index) = prizes
        .iter()
        .position(|prize| prize.pos.x as u16 == x && prize.pos.y as u16 == y)
    else {
        return false;
    };

    prizes.swap_remove(index);
    true
}

pub(crate) fn ship_overlaps_prize(
    ship_pos: Vec2,
    ship_radius_pixels: f32,
    prize_center: Vec2,
) -> bool {
    let radius = ship_radius_pixels.max(0.0);
    let ship_min = ship_pos - vec2(radius, radius);
    let ship_max = ship_pos + vec2(radius, radius);
    let half_prize = TILE_SIZE * 0.5;
    let prize_min = prize_center - vec2(half_prize, half_prize);
    let prize_max = prize_center + vec2(half_prize, half_prize);

    ship_max.x >= prize_min.x
        && ship_min.x < prize_max.x
        && ship_max.y >= prize_min.y
        && ship_min.y < prize_max.y
}

pub(crate) fn local_player_can_collect_prizes(spectator: bool, respawn_timer: f32) -> bool {
    !spectator && respawn_timer <= 0.0
}

pub(crate) fn network_player_overlaps_prize(
    player: &NetworkPlayer,
    ship_radius_pixels: f32,
    prize_center: Vec2,
) -> bool {
    player
        .position
        .is_some_and(|pos| ship_overlaps_prize(pos, ship_radius_pixels, prize_center))
}

pub(crate) fn can_spawn_prize_at(
    map: &ArenaMap,
    prizes_len: usize,
    max_prizes: usize,
    pos: Vec2,
) -> bool {
    prizes_len < max_prizes && map.tile_at_world(pos) == 0
}

pub(crate) fn collected_prize_bounty_gain(prize_id: i16, count: u16) -> u32 {
    if prize_id >= 0 { count as u32 } else { 0 }
}

pub(crate) fn death_prize_from_player_death(
    network_world: &NetworkWorld,
    death: PlayerDeath,
) -> Option<(u32, Vec2, PrizeKind)> {
    if death.green_id <= 0 || death.green_collectible_id == 0 {
        return None;
    }
    network_world.players.get(&death.killer_id)?;
    let killed = network_world.players.get(&death.killed_id)?;
    if killed.velocity == Vec2::ZERO {
        return None;
    }

    let pos = killed.position?;
    let (kind, negative) = PrizeKind::from_protocol_id(death.green_id as i16)?;
    if negative {
        return None;
    }
    Some((death.green_collectible_id, pos, kind))
}

pub(crate) fn remove_prize_by_collectible_id(prizes: &mut Vec<PrizePickup>, collectible_id: u32) {
    if collectible_id != 0 {
        prizes.retain(|prize| prize.collectible_id != collectible_id);
    }
}

pub(crate) fn ship_explosion_position_from_player_death(
    network_world: &NetworkWorld,
    death: PlayerDeath,
) -> Option<Vec2> {
    network_world.players.get(&death.killed_id)?.position
}

pub(crate) fn energy_capacity_after_prize(
    current_capacity: f32,
    upgrade_step: f32,
    initial_energy: f32,
    energy_max: f32,
    negative: bool,
) -> f32 {
    let step = upgrade_step.max(0.0);
    let initial = initial_energy.min(energy_max);
    if negative {
        (current_capacity - step).max(initial)
    } else {
        (current_capacity + step).min(energy_max)
    }
}

pub(crate) fn current_energy_after_capacity_prize(current_energy: f32, _new_capacity: f32) -> f32 {
    current_energy
}

/// The reference game mutates the current live stat by one prize step and then clamps
/// that operation. This is observably different from reconstructing a value
/// as `initial + count * step` when the maximum is not step-aligned: after a
/// positive prize reaches a partial cap, a negative prize subtracts a full
/// step from the cap itself.
pub(crate) fn stat_value_after_prize(
    current: f32,
    initial: f32,
    step: f32,
    max_value: f32,
    negative: bool,
) -> f32 {
    crate::gameplay_rules::stat_value_after_prize(current, initial, step, max_value, negative)
}

pub(crate) fn full_charge_energy_after(energy_max: f32, negative: bool) -> f32 {
    if negative { 1.0 } else { energy_max }
}

pub(crate) fn super_prize_lifetime_after(current_timer: f32, max_lifetime: f32, roll: u32) -> f32 {
    let max_ticks = crate::continuum::seconds_to_ticks_u32(max_lifetime);
    if max_ticks == 0 {
        return current_timer.max(0.0);
    }

    current_timer.max(crate::continuum::ticks_to_seconds_u32(roll % max_ticks))
}

pub(crate) fn shield_prize_lifetime_after(max_lifetime: f32) -> f32 {
    max_lifetime.max(0.0)
}

pub(crate) fn engine_shutdown_lifetime_after(max_lifetime: f32, negative: bool) -> f32 {
    let multiplier = if negative { 3.0 } else { 1.0 };
    (max_lifetime * multiplier).max(0.0)
}

pub(crate) fn warp_prize_energy_after(current_energy: f32, energy_max: f32, negative: bool) -> f32 {
    if negative {
        full_charge_energy_after(energy_max, true)
    } else {
        current_energy
    }
}

pub(crate) fn warp_prize_notification_after(negative: bool) -> (PrizeKind, bool) {
    if negative {
        (PrizeKind::FullCharge, true)
    } else {
        (PrizeKind::Warp, false)
    }
}

pub(crate) fn status_prize_capability_after(
    available: bool,
    was_capable: bool,
    negative: bool,
) -> Option<bool> {
    if available || (negative && was_capable) {
        Some(!negative)
    } else {
        None
    }
}

pub(crate) fn capability_prize_notification(
    kind: PrizeKind,
    was_capable: bool,
    negative: bool,
) -> Option<(PrizeKind, bool)> {
    if was_capable == negative {
        Some((kind, negative))
    } else {
        None
    }
}

pub(crate) fn status_prize_notification(
    kind: PrizeKind,
    available: bool,
    was_capable: bool,
    negative: bool,
) -> Option<(PrizeKind, bool)> {
    if available {
        capability_prize_notification(kind, was_capable, negative)
    } else if negative && was_capable {
        Some((kind, negative))
    } else {
        Some((PrizeKind::FullCharge, negative))
    }
}

pub(crate) fn apply_status_prize(
    capable: &mut bool,
    active: &mut bool,
    energy: &mut f32,
    energy_max: f32,
    available: bool,
    negative: bool,
) {
    if let Some(next_capable) = status_prize_capability_after(available, *capable, negative) {
        *capable = next_capable;
        if !next_capable {
            *active = false;
        }
    } else {
        *energy = full_charge_energy_after(energy_max, negative);
    }
}

pub(crate) fn tick_timed_status(timer: &mut f32, dt: f32) {
    *timer = (*timer - dt).max(0.0);
}

pub(crate) fn drain_status_energy(
    active: &mut bool,
    energy: &mut f32,
    cost_per_second: f32,
    dt: f32,
) {
    if !*active {
        return;
    }

    let cost = cost_per_second * dt;
    if *energy <= cost {
        *active = false;
    } else {
        *energy -= cost;
    }
}

pub(crate) fn drain_ship_status_energy(
    stealth: &mut bool,
    cloak: &mut bool,
    xradar: &mut bool,
    antiwarp: &mut bool,
    energy: &mut f32,
    stealth_cost_per_second: f32,
    cloak_cost_per_second: f32,
    xradar_cost_per_second: f32,
    antiwarp_cost_per_second: f32,
    dt: f32,
) {
    drain_status_energy(xradar, energy, xradar_cost_per_second, dt);
    drain_status_energy(stealth, energy, stealth_cost_per_second, dt);
    drain_status_energy(cloak, energy, cloak_cost_per_second, dt);
    drain_status_energy(antiwarp, energy, antiwarp_cost_per_second, dt);
}

pub(crate) fn shrapnel_prize_amount_after(
    current: u8,
    rate: u8,
    max_amount: u8,
    negative: bool,
) -> u8 {
    crate::gameplay_rules::shrapnel_prize_amount_after(current, rate, max_amount, negative)
}

pub(crate) fn generate_weighted_prize(
    weights: &[u8; CONTINUUM_PRIZE_WEIGHT_COUNT],
    weight_total: u16,
    negative_factor: u16,
    negative_allowed: bool,
    rng_seed: &mut i32,
) -> Option<(PrizeKind, bool)> {
    generate_weighted_prize_id(
        weights,
        weight_total,
        negative_factor,
        negative_allowed,
        rng_seed,
    )
    .and_then(|(prize_id, negative)| {
        PrizeKind::from_protocol_id(prize_id).map(|(kind, _)| (kind, negative))
    })
}

pub(crate) fn initial_bounty_prizes(
    weights: &[u8; CONTINUUM_PRIZE_WEIGHT_COUNT],
    weight_total: u16,
    initial_bounty: u32,
    prize_seed: i32,
) -> Vec<PrizeKind> {
    initial_bounty_prize_ids(weights, weight_total, initial_bounty, prize_seed)
        .into_iter()
        .filter_map(|prize_id| PrizeKind::from_protocol_id(prize_id).map(|(kind, _)| kind))
        .collect()
}

#[cfg(test)]
pub(crate) fn initial_bounty_prize_allowed(kind: PrizeKind) -> bool {
    initial_bounty_prize_allowed_id(kind.protocol_id())
}

pub(crate) fn multiprize_prizes(
    weights: &[u8; CONTINUUM_PRIZE_WEIGHT_COUNT],
    weight_total: u16,
    count: u16,
    prize_seed: i32,
) -> Vec<PrizeKind> {
    multiprize_sub_prize_ids(weights, weight_total, count, prize_seed)
        .into_iter()
        .filter_map(|prize_id| PrizeKind::from_protocol_id(prize_id).map(|(kind, _)| kind))
        .collect()
}

pub(crate) fn authoritative_multiprize_prizes(
    weights: &[u8; CONTINUUM_PRIZE_WEIGHT_COUNT],
    weight_total: u16,
    count: u16,
) -> Vec<PrizeKind> {
    multiprize_prizes(weights, weight_total, count, SECURITY_PRIZE_SEED)
}

#[cfg(test)]
pub(crate) fn multiprize_prize_allowed(kind: PrizeKind) -> bool {
    multiprize_sub_prize_allowed_id(kind.protocol_id())
}

pub(crate) fn none_prize_replacement_prize(
    weights: &[u8; CONTINUUM_PRIZE_WEIGHT_COUNT],
    weight_total: u16,
    prize_seed: i32,
) -> Option<PrizeKind> {
    if weight_total == 0 {
        return None;
    }

    let mut seed = prize_seed;
    for _ in 0..9999 {
        let Some((kind, _)) = generate_weighted_prize(weights, weight_total, 0, false, &mut seed)
        else {
            break;
        };

        if none_prize_replacement_prize_allowed(kind) {
            return Some(kind);
        }
    }

    None
}

pub(crate) fn none_prize_replacement_prize_allowed(kind: PrizeKind) -> bool {
    !matches!(
        kind,
        PrizeKind::EngineShutdown
            | PrizeKind::Shield
            | PrizeKind::Super
            | PrizeKind::Multiprize
            | PrizeKind::Warp
    )
}

pub(crate) fn continuum_prize_spawn_position(
    map_width: usize,
    map_height: usize,
    minimum_virtual: u16,
    upgrade_virtual: u16,
    player_count: usize,
    rng_seed: &mut i32,
) -> Option<Vec2> {
    let map_tiles = map_width.min(map_height).max(1) as u32;
    let mut spawn_extent = minimum_virtual as u32
        + upgrade_virtual as u32 * player_count.min(u16::MAX as usize) as u32;
    spawn_extent = spawn_extent.clamp(3, map_tiles);

    let range = spawn_extent - 2;
    let offset = (map_tiles - spawn_extent) / 2;
    let x = (vie_rng_next(rng_seed) % range) + 1 + offset;
    let y = (vie_rng_next(rng_seed) % range) + 1 + offset;

    if x >= map_width as u32 || y >= map_height as u32 {
        return None;
    }

    Some(tile_center(ivec2(x as i32, y as i32)))
}

pub(crate) fn prize_lifetime_seconds(
    min_seconds: f32,
    max_seconds: f32,
    rng_seed: &mut i32,
) -> f32 {
    let min_ticks = crate::continuum::seconds_to_ticks_u32(min_seconds);
    let max_ticks = crate::continuum::seconds_to_ticks_u32(max_seconds.max(min_seconds));
    if max_ticks <= min_ticks {
        return crate::continuum::ticks_to_seconds_u32(min_ticks);
    }

    let roll = vie_rng_next(rng_seed) % (max_ticks - min_ticks + 1);
    crate::continuum::ticks_to_seconds_u32(min_ticks + roll)
}
