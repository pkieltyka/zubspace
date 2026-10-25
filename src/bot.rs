use macroquad::prelude::Vec2;

use crate::{
    ArenaMap, ArenaTuning, BOT_DECOY_TARGET_RADIUS, BOT_PRIZE_SEEK_RADIUS, BOT_ROTATION_PER_SECOND,
    BotShip, Decoy, ENGINE_SHUTDOWN_ROTATION_PER_SECOND, FLAG_PICKUP_RADIUS, PrizePickup,
    ShipTuning, WEAPON_LEVEL_MIN, combat::leveled_weapon_damage,
};

pub(crate) fn bot_can_fire(
    distance_to_target: f32,
    aim_alignment: f32,
    fire_cooldown: f32,
    jammed: bool,
    engine_shutdown: bool,
    weapon_range: f32,
) -> bool {
    distance_to_target < weapon_range.max(0.0)
        && aim_alignment > 0.93
        && fire_cooldown <= 0.0
        && !jammed
        && !engine_shutdown
}

pub(crate) fn bot_bullet_level(tuning: ShipTuning) -> u8 {
    tuning
        .initial_gun_level
        .min(tuning.max_gun_level.max(WEAPON_LEVEL_MIN))
        .max(WEAPON_LEVEL_MIN)
}

pub(crate) fn bot_bullet_damage(arena: ArenaTuning, ship_type: usize) -> f32 {
    let level = bot_bullet_level(arena.ship(ship_type));
    leveled_weapon_damage(arena.bullet_damage, arena.bullet_damage_upgrade, level)
}

pub(crate) fn bot_bullet_range(arena: ArenaTuning, ship_type: usize) -> f32 {
    arena.ship(ship_type).bullet_speed * arena.bullet_lifetime
}

pub(crate) fn bot_spawn_energy(tuning: ShipTuning) -> f32 {
    tuning.energy_initial.min(tuning.energy_max)
}

pub(crate) fn bot_prize_seek_energy(max_energy: f32) -> f32 {
    max_energy * 0.65
}

pub(crate) fn bot_initial_fire_cooldown(base_cooldown: f32, id: usize) -> f32 {
    base_cooldown.max(0.0) * (0.5 + id as f32 * 0.2)
}

pub(crate) fn bot_fire_cooldown_after_shot(base_cooldown: f32, id: usize) -> f32 {
    base_cooldown.max(0.0) + id as f32 * 0.12
}

pub(crate) fn bot_flag_objective_target(
    carrying_flag: bool,
    dropped_flag_pos: Option<Vec2>,
    source_flag_pos: Option<Vec2>,
    goal_pos: Option<Vec2>,
    flag_source_hidden: bool,
) -> Option<Vec2> {
    if carrying_flag {
        goal_pos
    } else {
        dropped_flag_pos.or_else(|| (!flag_source_hidden).then_some(source_flag_pos).flatten())
    }
}

pub(crate) fn bot_jammed_by_antiwarp(
    player_pos: Vec2,
    antiwarp_active: bool,
    bot_pos: Vec2,
    radius: f32,
) -> bool {
    antiwarp_active && player_pos.distance_squared(bot_pos) <= radius * radius
}

pub(crate) fn bot_rotation_per_second(engine_shutdown: bool) -> f32 {
    if engine_shutdown {
        ENGINE_SHUTDOWN_ROTATION_PER_SECOND
    } else {
        BOT_ROTATION_PER_SECOND
    }
}

pub(crate) fn bot_spawn_orientation(id: usize) -> f32 {
    (id as f32 * 0.25).rem_euclid(1.0)
}

pub(crate) fn bot_touching_pos(bots: &[BotShip], pos: Vec2, radius: f32) -> Option<usize> {
    bots.iter()
        .filter(|bot| bot.respawn_timer <= 0.0)
        .filter(|bot| bot.pos.distance_squared(pos) <= radius * radius)
        .min_by(|left, right| {
            left.pos
                .distance_squared(pos)
                .total_cmp(&right.pos.distance_squared(pos))
        })
        .map(|bot| bot.id)
}

pub(crate) fn bot_touches_map_object(pos: Vec2, map: &ArenaMap, object_id: u8) -> bool {
    map.is_touching_object(pos, object_id, FLAG_PICKUP_RADIUS)
        || map.tile_at_world(pos) == object_id
}

pub(crate) fn bot_touching_map_object(
    bots: &[BotShip],
    map: &ArenaMap,
    object_id: u8,
) -> Option<usize> {
    bots.iter()
        .filter(|bot| bot.respawn_timer <= 0.0)
        .find(|bot| bot_touches_map_object(bot.pos, map, object_id))
        .map(|bot| bot.id)
}

pub(crate) fn bot_prize_target_position(
    bot_pos: Vec2,
    bot_energy: f32,
    bot_max_energy: f32,
    prizes: &[PrizePickup],
) -> Option<Vec2> {
    if bot_energy >= bot_prize_seek_energy(bot_max_energy) {
        return None;
    }

    prizes
        .iter()
        .filter(|prize| prize.ttl > 0.0)
        .filter(|prize| {
            bot_pos.distance_squared(prize.pos) <= BOT_PRIZE_SEEK_RADIUS * BOT_PRIZE_SEEK_RADIUS
        })
        .min_by(|left, right| {
            bot_pos
                .distance_squared(left.pos)
                .total_cmp(&bot_pos.distance_squared(right.pos))
        })
        .map(|prize| prize.pos)
}

pub(crate) fn bot_target_position(
    bot_pos: Vec2,
    player_pos: Vec2,
    player_hidden: bool,
    decoys: &[Decoy],
) -> Option<Vec2> {
    let decoy_target = decoys
        .iter()
        .filter(|decoy| decoy.ttl > 0.0)
        .filter_map(|decoy| {
            let distance_squared = bot_pos.distance_squared(decoy.pos);
            if distance_squared <= BOT_DECOY_TARGET_RADIUS * BOT_DECOY_TARGET_RADIUS {
                Some((decoy.pos, distance_squared))
            } else {
                None
            }
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(pos, _)| pos);

    if decoy_target.is_some() {
        decoy_target
    } else if player_hidden {
        None
    } else {
        Some(player_pos)
    }
}
