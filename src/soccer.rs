use macroquad::prelude::Vec2;

use crate::math::canonical_discrete_heading;
use crate::{ArenaMap, ArenaTuning, BotShip, TILE_SIZE};

pub fn continuum_powerball_friction(raw_friction: u16) -> f32 {
    (1.0 - raw_friction as f32 / 100_000.0).clamp(0.0, 1.0)
}

pub(crate) fn bot_powerball_kick(
    ball_pos: Vec2,
    bots: &[BotShip],
    map: &ArenaMap,
    tuning: &ArenaTuning,
    disable_through_walls: bool,
    pass_cooldown: f32,
) -> Option<(usize, Vec2, Vec2, f32)> {
    let bot = bots
        .iter()
        .filter(|bot| bot.respawn_timer <= 0.0)
        .filter(|bot| {
            let radius = tuning.ship(bot.ship_type).soccer_ball_proximity;
            bot.pos.distance_squared(ball_pos) <= radius * radius
        })
        .filter(|bot| {
            can_touch_powerball(map, bot.pos, ball_pos, disable_through_walls, pass_cooldown)
        })
        .min_by(|left, right| {
            left.pos
                .distance_squared(ball_pos)
                .total_cmp(&right.pos.distance_squared(ball_pos))
        })?;

    let delta = ball_pos - bot.pos;
    let direction = if delta.length_squared() > 0.001 {
        delta.normalize()
    } else {
        canonical_discrete_heading(bot.orientation)
    };
    let bot_tuning = tuning.ship(bot.ship_type);
    Some((
        bot.id,
        bot.pos + direction * (bot_tuning.soccer_ball_proximity + 4.0),
        bot.vel + direction * bot_tuning.soccer_ball_speed,
        bot_tuning.soccer_ball_friction,
    ))
}

pub(crate) fn can_touch_powerball(
    map: &ArenaMap,
    picker_pos: Vec2,
    ball_pos: Vec2,
    disable_through_walls: bool,
    pass_cooldown: f32,
) -> bool {
    pass_cooldown <= 0.0 && can_pickup_powerball(map, picker_pos, ball_pos, disable_through_walls)
}

pub(crate) fn local_player_can_attempt_powerball_pickup(
    spectator_mode: bool,
    ship_type: usize,
    attached_to_turret: bool,
    already_carrying_ball: bool,
    alive: bool,
) -> bool {
    alive && !spectator_mode && ship_type < 8 && !attached_to_turret && !already_carrying_ball
}

pub(crate) fn powerball_pickup_weapon_cooldowns_after(
    current_bullet_cooldown: f32,
    current_bomb_cooldown: f32,
    bomb_fire_delay: f32,
) -> (f32, f32) {
    let delay = bomb_fire_delay.max(0.0);
    (
        current_bullet_cooldown.max(delay),
        current_bomb_cooldown.max(delay),
    )
}

pub(crate) fn manual_powerball_fire_velocity(
    ship_velocity: Vec2,
    heading: Vec2,
    speed: f32,
) -> Vec2 {
    ship_velocity + heading * speed
}

pub(crate) fn manual_powerball_fire_request_position(ship_position: Vec2) -> Vec2 {
    ship_position
}

pub(crate) fn timed_powerball_release_velocity(
    ship_velocity: Vec2,
    heading: Vec2,
    speed: f32,
) -> Vec2 {
    ship_velocity - heading * speed
}

pub(crate) fn can_pickup_powerball(
    map: &ArenaMap,
    picker_pos: Vec2,
    ball_pos: Vec2,
    disable_through_walls: bool,
) -> bool {
    !disable_through_walls || !solid_between_world_points(map, picker_pos, ball_pos)
}

pub(crate) fn solid_between_world_points(map: &ArenaMap, from: Vec2, to: Vec2) -> bool {
    let delta = to - from;
    let distance = delta.length();
    if distance <= 0.0 {
        return false;
    }

    let steps = (distance / (TILE_SIZE * 0.25)).ceil().max(1.0) as i32;
    for index in 1..steps {
        let pos = from + delta * (index as f32 / steps as f32);
        if map.is_solid_world(pos) {
            return true;
        }
    }
    false
}

pub fn soccer_goal_counts(
    world_pos: Vec2,
    tile_size: f32,
    frequency: u16,
    soccer_mode: u8,
) -> bool {
    let tile_pos = world_pos / tile_size;
    !soccer_team_goal(tile_pos, frequency as u32, soccer_mode)
}

pub(crate) fn soccer_team_goal(pos: Vec2, frequency: u32, soccer_mode: u8) -> bool {
    match soccer_mode {
        0 => false,
        1 => {
            if frequency & 1 != 0 {
                pos.x >= 512.0
            } else {
                pos.x < 512.0
            }
        }
        2 => {
            if frequency & 1 != 0 {
                pos.y >= 512.0
            } else {
                pos.y < 512.0
            }
        }
        3 => soccer_mode3_team_goal(pos, frequency),
        4 => !soccer_mode3_team_goal(pos, frequency),
        5 => soccer_mode5_team_goal(pos, frequency),
        6 => !soccer_mode5_team_goal(pos, frequency),
        _ => true,
    }
}

fn soccer_mode3_team_goal(pos: Vec2, frequency: u32) -> bool {
    match frequency % 4 {
        0 => pos.x < 512.0 && pos.y < 512.0,
        1 => pos.x >= 512.0 && pos.y < 512.0,
        2 => pos.x < 512.0 && pos.y >= 512.0,
        3 => pos.x >= 512.0 && pos.y >= 512.0,
        _ => false,
    }
}

fn soccer_mode5_team_goal(pos: Vec2, frequency: u32) -> bool {
    match frequency % 4 {
        0 => {
            if pos.y < 512.0 {
                pos.x < pos.y
            } else {
                pos.x + pos.y < 1024.0
            }
        }
        1 => {
            if pos.x < 512.0 {
                pos.x + pos.y >= 1024.0
            } else {
                pos.x < pos.y
            }
        }
        2 => {
            if pos.x < 512.0 {
                pos.x >= pos.y
            } else {
                pos.x + pos.y < 1024.0
            }
        }
        3 => {
            if pos.y <= 512.0 {
                pos.x + pos.y >= 1024.0
            } else {
                pos.x >= pos.y
            }
        }
        _ => false,
    }
}
