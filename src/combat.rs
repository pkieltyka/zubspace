use macroquad::prelude::*;

use crate::continuum_weapon::{
    continuum_proximity_bomb_trigger_radius, continuum_proximity_sensor_overlaps_ship,
    continuum_weapon_contact_overlaps_ship,
};
use crate::{
    PROXIMITY_BOMB_TRIGGER_SCALE, REPEL_REUSE_COOLDOWN, SHRAPNEL_INACTIVE_DAMAGE_TIME,
    WEAPON_LEVEL_MIN,
};

pub(crate) fn bomb_flash_position(ship_pos: Vec2, heading: Vec2, ship_radius: f32) -> Vec2 {
    ship_pos + heading * ship_radius
}

pub(crate) fn radial_damage(distance: f32, radius: f32, damage: f32) -> f32 {
    radial_damage_raw(distance, radius, damage).trunc()
}

fn radial_damage_raw(distance: f32, radius: f32, damage: f32) -> f32 {
    if distance >= radius {
        0.0
    } else {
        damage * (1.0 - distance / radius).clamp(0.0, 1.0)
    }
}

pub(crate) fn radial_damage_with_shooter_proximity_reduction(
    target_distance: f32,
    shooter_distance: Option<f32>,
    radius: f32,
    damage: f32,
) -> f32 {
    let target_damage = radial_damage(target_distance, radius, damage);
    let Some(shooter_distance) = shooter_distance else {
        return target_damage;
    };
    if shooter_distance >= radius {
        return target_damage;
    }

    let shooter_reduction = (radial_damage_raw(shooter_distance, radius, damage) * 0.5).trunc();
    (target_damage - shooter_reduction).max(0.0)
}

pub(crate) fn explosion_bot_damage_active(damages_bots: bool, age: f32) -> bool {
    damages_bots && age <= 0.05
}

pub(crate) fn thor_damage(base_bomb_damage: f32, thor_level: u8) -> f32 {
    base_bomb_damage + base_bomb_damage * thor_level as f32 * thor_level as f32
}

pub(crate) fn thor_damage_for_ship(
    base_bomb_damage: f32,
    bouncing_multiplier: f32,
    bounces: u8,
    thor_level: u8,
) -> f32 {
    bomb_damage_for_ship(
        thor_damage(base_bomb_damage, thor_level),
        bouncing_multiplier,
        1.0,
        bounces,
        false,
    )
}

pub(crate) fn bomb_damage_for_ship(
    base_damage: f32,
    bouncing_multiplier: f32,
    emp_multiplier: f32,
    bounces: u8,
    emp_bomb: bool,
) -> f32 {
    let damage = if emp_bomb {
        (base_damage * emp_multiplier).trunc()
    } else {
        base_damage
    };
    if bounces > 0 {
        (damage * bouncing_multiplier).trunc()
    } else {
        damage
    }
}

pub(crate) fn proximity_bomb_trigger_radius(proximity_distance: f32, bomb_level: u8) -> f32 {
    continuum_proximity_bomb_trigger_radius(proximity_distance, bomb_level)
}

pub(crate) fn thor_proximity_trigger_radius(proximity_distance: f32, thor_level: u8) -> f32 {
    (proximity_distance + thor_level as f32 + 3.0) * PROXIMITY_BOMB_TRIGGER_SCALE
}

pub(crate) fn bomb_explosion_radius(base_pixels: f32, bomb_level: u8) -> f32 {
    base_pixels * bomb_level.max(WEAPON_LEVEL_MIN) as f32
}

pub(crate) fn emp_bomb_shutdown_time(emp_bomb: bool, shutdown_time: f32) -> f32 {
    if emp_bomb { shutdown_time } else { 0.0 }
}

pub(crate) fn leveled_weapon_damage(base_damage: f32, upgrade_damage: f32, level: u8) -> f32 {
    base_damage + upgrade_damage * level.saturating_sub(WEAPON_LEVEL_MIN) as f32
}

pub(crate) fn leveled_weapon_energy_cost(base_cost: f32, upgrade_cost: f32, level: u8) -> f32 {
    base_cost + upgrade_cost * level.saturating_sub(WEAPON_LEVEL_MIN) as f32
}

pub(crate) fn leveled_gun_energy_cost(base_cost: f32, level: u8) -> f32 {
    base_cost * level.max(WEAPON_LEVEL_MIN) as f32
}

#[cfg(test)]
pub(crate) fn mine_limit(ship_max_mines: usize, team_max_mines: usize) -> usize {
    ship_max_mines.min(team_max_mines)
}

pub(crate) fn mine_drop_allowed(
    player_mines: usize,
    team_mines: usize,
    ship_max_mines: usize,
    team_max_mines: usize,
    overlaps_existing_mine: bool,
) -> bool {
    !overlaps_existing_mine && player_mines < ship_max_mines && team_mines < team_max_mines
}

pub(crate) fn mine_positions_overlap(a: Vec2, b: Vec2) -> bool {
    a == b
}

pub(crate) fn rocket_available(rocket_timer: f32, bomb_cooldown: f32) -> bool {
    rocket_timer <= 0.0 && bomb_cooldown <= 0.0
}

pub(crate) fn bomb_recoil_velocity(velocity: Vec2, heading: Vec2, recoil_speed: f32) -> Vec2 {
    velocity - heading * recoil_speed
}

pub(crate) fn repel_cooldown_after_weapon(current: f32) -> f32 {
    current.max(REPEL_REUSE_COOLDOWN)
}

pub(crate) fn fast_weapon_available(afterburning: bool, disable_fast_shooting: bool) -> bool {
    !afterburning || !disable_fast_shooting
}

pub(crate) fn bullet_or_burst_damage_after_exactness(exact_damage: bool, damage: f32) -> f32 {
    if exact_damage || damage <= 0.0 {
        return damage;
    }

    let max_roll = (damage * damage).floor().max(0.0) as i32 + 1;
    let roll = macroquad::rand::gen_range(0, max_roll);
    inexact_weapon_damage_from_roll(damage, roll as u32)
}

pub(crate) fn player_damage_is_fatal(energy: f32, damage: f32) -> bool {
    energy < damage
}

pub(crate) fn local_bomb_self_damage_energy_after(energy: f32, damage: f32) -> f32 {
    if energy <= damage {
        1.0
    } else {
        energy - damage
    }
}

pub(crate) fn local_visual_damage_applies(networked_play: bool, damage: f32) -> bool {
    damage > 0.0 && !networked_play
}

pub(crate) fn safe_zone_weapon_hit_drops_flag(
    in_safe: bool,
    carrying_flag: bool,
    hit: bool,
) -> bool {
    in_safe && carrying_flag && hit
}

pub(crate) fn inexact_weapon_damage_from_roll(damage: f32, roll: u32) -> f32 {
    if damage <= 0.0 {
        return 0.0;
    }

    let max_roll = (damage * damage).floor().max(0.0) as u32 + 1;
    ((roll % max_roll) as f32).sqrt().floor()
}

pub(crate) fn ball_carrier_should_kick(carrying_ball: bool, allow_carrier_weapon: bool) -> bool {
    carrying_ball && !allow_carrier_weapon
}

pub(crate) fn ball_carrier_warp_should_kick(carrying_ball: bool) -> bool {
    carrying_ball
}

pub(crate) fn shrapnel_count(amount: u8, max_amount: u8) -> usize {
    amount.min(max_amount) as usize
}

pub(crate) fn explosive_shrapnel_level(parent_bomb_level: u8) -> u8 {
    // Project override: fragments match their parent bomb, independently of guns.
    parent_bomb_level
}

pub(crate) fn shrapnel_projectile_position(explosion_pos: Vec2) -> Vec2 {
    explosion_pos
}

pub(crate) fn shrapnel_rng_seed(
    pos: Vec2,
    vel: Vec2,
    shrapnel_count: usize,
    weapon_level: u8,
    frequency: u16,
) -> u32 {
    crate::gameplay_rules::shrapnel_rng_seed(
        crate::gameplay_rules::RulesVector::new(pos.x, pos.y),
        crate::gameplay_rules::RulesVector::new(vel.x, vel.y),
        shrapnel_count,
        weapon_level,
        frequency,
    )
}

pub(crate) fn shrapnel_direction(index: usize, count: usize, random: bool, seed: u32) -> Vec2 {
    let direction = crate::gameplay_rules::shrapnel_direction(index, count, random, seed);
    vec2(direction.x, direction.y)
}

pub(crate) fn inactive_shrapnel_damage(inactive_damage: f32, level: u8) -> f32 {
    inactive_damage.trunc() * level.saturating_sub(WEAPON_LEVEL_MIN) as f32
}

pub(crate) fn active_shrapnel_damage(
    base_damage: f32,
    upgrade_damage: f32,
    multiplier: f32,
    level: u8,
) -> f32 {
    (leveled_weapon_damage(base_damage, upgrade_damage, level) * multiplier).trunc()
}

pub(crate) fn shrapnel_damage(age: f32, inactive_damage: f32, active_damage: f32) -> f32 {
    if age <= SHRAPNEL_INACTIVE_DAMAGE_TIME {
        inactive_damage
    } else {
        active_damage
    }
}

pub(crate) fn decoy_mirrored_orientation(initial_orientation: f32, owner_orientation: f32) -> f32 {
    (initial_orientation * 2.0 - owner_orientation).rem_euclid(1.0)
}

pub(crate) fn projectile_cleanup_creates_impact(solid_hit: bool, timed_out: bool) -> bool {
    solid_hit && !timed_out
}

pub(crate) fn explosive_cleanup_creates_explosion(triggered: bool, timed_out: bool) -> bool {
    triggered && !timed_out
}

pub(crate) fn shrapnel_cleanup_creates_impact(solid_hit: bool, timed_out: bool, age: f32) -> bool {
    projectile_cleanup_creates_impact(solid_hit, timed_out) && age > SHRAPNEL_INACTIVE_DAMAGE_TIME
}

pub(crate) fn early_shrapnel_wall_bounce_times_out(age: f32) -> bool {
    age <= SHRAPNEL_INACTIVE_DAMAGE_TIME
}

pub(crate) fn damage_after_timed_protection(
    damage: f32,
    super_timer: f32,
    shield_timer: f32,
    shield_lifetime: f32,
) -> f32 {
    if damage <= 0.0 || super_timer > 0.0 {
        return 0.0;
    }

    if shield_timer <= 0.0 || shield_lifetime <= 0.0 {
        return damage;
    }

    let blocked_percent = (shield_timer / shield_lifetime).clamp(0.0, 1.0);
    damage - (damage * blocked_percent).trunc()
}

pub(crate) fn bot_bullet_hits_ship(bullet_pos: Vec2, ship_pos: Vec2, ship_radius: f32) -> bool {
    bullet_contact_overlaps_ship(bullet_pos, ship_pos, ship_radius)
}

pub(crate) fn bullet_contact_overlaps_ship(
    bullet_pos: Vec2,
    ship_pos: Vec2,
    ship_radius: f32,
) -> bool {
    continuum_weapon_contact_overlaps_ship(
        bullet_pos.x,
        bullet_pos.y,
        ship_pos.x,
        ship_pos.y,
        ship_radius,
    )
}

#[cfg(test)]
pub(crate) fn explosive_contact_radius(ship_radius: f32) -> f32 {
    ship_radius + crate::continuum_weapon::CONTINUUM_WEAPON_CONTACT_HALF_EXTENT
}

pub(crate) fn explosive_contact_overlaps_ship(
    weapon_pos: Vec2,
    ship_pos: Vec2,
    ship_radius: f32,
) -> bool {
    continuum_weapon_contact_overlaps_ship(
        weapon_pos.x,
        weapon_pos.y,
        ship_pos.x,
        ship_pos.y,
        ship_radius,
    )
}

pub(crate) fn proximity_sensor_overlaps_ship(
    weapon_pos: Vec2,
    ship_pos: Vec2,
    sensor_radius: f32,
    ship_radius: f32,
) -> bool {
    continuum_proximity_sensor_overlaps_ship(
        weapon_pos.x,
        weapon_pos.y,
        ship_pos.x,
        ship_pos.y,
        sensor_radius,
        ship_radius,
    )
}

pub(crate) fn proximity_bomb_safety_blocks_ship(
    shooter_pos: Vec2,
    target_pos: Vec2,
    safety_radius: f32,
) -> bool {
    shooter_pos.distance_squared(target_pos) <= safety_radius * safety_radius
}

pub(crate) fn repel_velocity(
    origin: Vec2,
    pos: &mut Vec2,
    vel: &mut Vec2,
    speed: f32,
    radius: f32,
) -> bool {
    let Some(repelled) = crate::gameplay_rules::repel_velocity_after(
        crate::gameplay_rules::RulesVector::new(origin.x, origin.y),
        crate::gameplay_rules::RulesVector::new(pos.x, pos.y),
        speed,
        radius,
    ) else {
        return false;
    };
    *vel = vec2(repelled.x, repelled.y);
    true
}

pub(crate) fn repel_can_affect_frequency(source_frequency: u16, target_frequency: u16) -> bool {
    source_frequency != target_frequency
}
