use crate::{ArenaMap, SetCoordinates};
use macroquad::prelude::*;

pub(crate) fn apply_set_coordinates_to_ship(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: &mut Vec2,
    coordinates: SetCoordinates,
    radius: f32,
) -> Vec2 {
    *pos = coordinates.position();
    *vel = Vec2::ZERO;
    unstuck_ship_after_set_coordinates(map, pos, radius);
    *pos
}

fn unstuck_ship_after_set_coordinates(map: &ArenaMap, pos: &mut Vec2, radius: f32) {
    while !map.can_fit_world(*pos, radius) {
        *pos = vec2((pos.x - 1.0).floor(), (pos.y - 1.0).floor());

        if pos.x < 0.0 {
            pos.x = 0.0;
            break;
        }

        if pos.y < 0.0 {
            pos.y = 0.0;
            break;
        }
    }
}

pub(crate) fn warp_energy_after(
    current: f32,
    drain_without_portal: bool,
    used_portal: bool,
    from: Vec2,
    destination: Vec2,
) -> f32 {
    if drain_without_portal && !used_portal && from.distance_squared(destination) > 1.0 {
        1.0
    } else {
        current
    }
}

pub(crate) fn warp_velocity_after(current: Vec2, used_portal: bool) -> Vec2 {
    if used_portal { current } else { Vec2::ZERO }
}

pub(crate) fn warp_prize_destination(spawn_position: Vec2, _portal_position: Option<Vec2>) -> Vec2 {
    spawn_position
}

pub(crate) fn warp_prize_velocity_after(_current: Vec2) -> Vec2 {
    Vec2::ZERO
}

pub(crate) fn antiwarp_settle_active(timer: f32) -> bool {
    timer > 0.0
}

/// Mirrors the reference game's `PlayerManager::IsAntiwarped` — true if any hostile
/// player is broadcasting antiwarp status within `antiwarp_radius` of
/// `ship_pos`. Spectators and same-frequency players are excluded; positions
/// without a recent broadcast are skipped.
pub(crate) fn is_antiwarped_by_enemy(
    network_world: &crate::network::NetworkWorld,
    ship_pos: macroquad::prelude::Vec2,
    own_frequency: u16,
    antiwarp_radius: f32,
) -> bool {
    if antiwarp_radius <= 0.0 {
        return false;
    }
    let radius_sq = antiwarp_radius * antiwarp_radius;
    network_world.players.values().any(|player| {
        if player.ship >= 8 || player.frequency == own_frequency {
            return false;
        }
        if player.togglables & crate::constants::STATUS_ANTIWARP == 0 {
            return false;
        }
        let Some(pos) = player.position else {
            return false;
        };
        (pos - ship_pos).length_squared() <= radius_sq
    })
}

pub(crate) fn portal_place_available(portals: u32, fake_antiwarp_timer: f32) -> bool {
    portals > 0 && !antiwarp_settle_active(fake_antiwarp_timer)
}

pub(crate) fn warp_available(
    has_portal: bool,
    energy: f32,
    max_energy: f32,
    fake_antiwarp_timer: f32,
    bomb_cooldown: f32,
) -> bool {
    !antiwarp_settle_active(fake_antiwarp_timer)
        && (has_portal || (bomb_cooldown <= 0.0 && energy >= max_energy))
}

pub(crate) fn manual_warp_bomb_cooldown_after(current: f32, reuse_delay: f32) -> f32 {
    current.max(reuse_delay)
}

#[cfg(test)]
pub(crate) fn continuum_default_spawn_radius(player_count: usize, warp_radius_limit: u16) -> i32 {
    let player_count = player_count.min(u32::MAX as usize) as u32;
    let mut radius = (((player_count / 8) * 0x2000 + 0x400) / 0x60 + 0x100) as i32;
    radius = radius.min(warp_radius_limit.max(3) as i32);
    radius.max(3)
}
