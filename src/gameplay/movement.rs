use super::*;
use crate::tile::is_solid_tile_with_doors;

pub(crate) fn ship_bounce_sound_threshold_squared() -> f32 {
    let threshold = 3.0 * TILE_SIZE;
    threshold * threshold
}

pub(crate) fn move_entity_with_collision(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: &mut Vec2,
    radius: f32,
    dt: f32,
    bounce_factor: f32,
) -> bool {
    move_entity_with_collision_and_passable_tiles(map, pos, vel, radius, dt, bounce_factor, &[])
}

pub(crate) fn move_entity_with_collision_and_passable_tiles(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: &mut Vec2,
    radius: f32,
    dt: f32,
    bounce_factor: f32,
    passable_tiles: &[IVec2],
) -> bool {
    let result = integrate_player_collision(
        MovementVector::new(pos.x, pos.y),
        MovementVector::new(vel.x, vel.y),
        dt,
        bounce_factor,
        |candidate, axis, signed_delta| {
            ship_axis_collides_with_solid_or_bounds(
                map,
                vec2(candidate.x, candidate.y),
                radius,
                passable_tiles,
                axis,
                signed_delta,
            )
        },
    );
    *pos = vec2(result.position.x, result.position.y);
    *vel = vec2(result.velocity.x, result.velocity.y);
    result.bounced
}

pub(crate) fn ship_axis_collides_with_solid_or_bounds(
    map: &ArenaMap,
    pos: Vec2,
    radius_px: f32,
    passable_tiles: &[IVec2],
    axis: CollisionAxis,
    signed_delta: f32,
) -> bool {
    movement_axis_overlaps_blocked_tile(
        MovementVector::new(pos.x, pos.y),
        radius_px,
        map.width,
        TILE_SIZE,
        axis,
        signed_delta,
        |x, y| {
            let tile = map.tile(x, y);
            if tile == BRICK_TILE_ID && passable_tiles.contains(&ivec2(x, y)) {
                return false;
            }
            is_solid_tile_with_doors(tile, map.door_seed)
        },
    )
}

#[cfg(test)]
pub(crate) fn can_fit_world_with_passable_tiles(
    map: &ArenaMap,
    pos: Vec2,
    radius_px: f32,
    passable_tiles: &[IVec2],
) -> bool {
    let min_tile_x = ((pos.x - radius_px) / TILE_SIZE).floor() as i32;
    let min_tile_y = ((pos.y - radius_px) / TILE_SIZE).floor() as i32;
    let max_tile_x = ((pos.x + radius_px) / TILE_SIZE).floor() as i32;
    let max_tile_y = ((pos.y + radius_px) / TILE_SIZE).floor() as i32;

    for y in min_tile_y..=max_tile_y {
        for x in min_tile_x..=max_tile_x {
            let tile = map.tile(x, y);
            if tile == BRICK_TILE_ID && passable_tiles.contains(&ivec2(x, y)) {
                continue;
            }
            if !is_solid_tile_with_doors(tile, map.door_seed) {
                continue;
            }

            if crate::tile::circle_overlaps_tile(pos, radius_px, x, y) {
                return false;
            }
        }
    }

    true
}

pub(crate) fn is_solid_world_for_frequency(
    map: &ArenaMap,
    pos: Vec2,
    frequency: u16,
    bricks: &[Brick],
) -> bool {
    let x = (pos.x / TILE_SIZE).floor() as i32;
    let y = (pos.y / TILE_SIZE).floor() as i32;
    let tile = map.tile(x, y);
    if tile == BRICK_TILE_ID
        && bricks
            .iter()
            .any(|brick| brick.tile == ivec2(x, y) && brick.team == frequency)
    {
        return false;
    }
    is_solid_tile_with_doors(tile, map.door_seed)
}

pub(crate) fn brick_tile_overlaps_ship_aabb(tile: IVec2, ship_pos: Vec2, ship_radius: f32) -> bool {
    let ship_min = pixel_rounded(ship_pos) - vec2(ship_radius, ship_radius);
    let ship_max = pixel_rounded(ship_pos) + vec2(ship_radius, ship_radius);

    movement_aabb_overlaps_tile(
        MovementVector::new(ship_min.x, ship_min.y),
        MovementVector::new(ship_max.x, ship_max.y),
        tile.x,
        tile.y,
        TILE_SIZE,
    )
}

fn pixel_rounded(pos: Vec2) -> Vec2 {
    vec2(pos.x as i32 as f32, pos.y as i32 as f32)
}

pub(crate) fn spawn_exhaust_particles(
    exhausts: &mut Vec<ExhaustParticle>,
    ship_pos: Vec2,
    heading: Vec2,
    reversing: bool,
    ship_radius: f32,
) {
    let exhaust_heading = if reversing { heading } else { -heading };
    let origin = ship_pos + exhaust_heading * ship_radius;
    let side = perpendicular(heading) * 3.0;

    for offset in [-side, side] {
        exhausts.push(ExhaustParticle {
            pos: origin + offset - vec2(8.0, 8.0),
            vel: exhaust_heading * 70.0 + offset * 5.0,
            age: 0.0,
        });
    }
}

pub(crate) fn entity_needs_door_relocation(
    map: &ArenaMap,
    pos: Vec2,
    radius: f32,
    before_seed: u8,
    after_seed: u8,
) -> bool {
    crate::gameplay_rules::newly_closed_door_overlap(
        pos.x,
        pos.y,
        radius,
        map.width,
        TILE_SIZE,
        before_seed,
        after_seed,
        |x, y| {
            (x >= 0 && y >= 0 && x < map.width as i32 && y < map.width as i32)
                .then(|| map.tile(x, y))
        },
        is_solid_tile_with_doors,
    )
}
pub(crate) fn dropped_flag_position(map: &ArenaMap, pos: Vec2) -> Vec2 {
    if map.is_solid_world(pos) {
        map.spawn_position()
    } else {
        pos
    }
}

pub(crate) const WEAPON_COLLISION_STEP_SECONDS: f32 =
    crate::gameplay_rules::weapon_motion::STEP_SECONDS;

pub(crate) fn move_bouncing_point_for_frequency(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: &mut Vec2,
    dt: f32,
    frequency: u16,
    bricks: &[Brick],
) -> bool {
    let mut remaining = dt.max(0.0);
    let mut bounced = false;
    while remaining > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        bounced |= move_bouncing_point_step_for_frequency(map, pos, vel, step, frequency, bricks);
        remaining -= step;
    }

    bounced
}

#[cfg(test)]
pub(crate) fn move_point_until_solid_for_frequency(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: Vec2,
    dt: f32,
    frequency: u16,
    bricks: &[Brick],
) -> bool {
    let mut remaining = dt.max(0.0);
    while remaining > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        if move_point_until_solid_step_for_frequency(map, pos, vel, step, frequency, bricks) {
            return true;
        }
        remaining -= step;
    }

    false
}

pub(crate) fn move_point_until_solid_step_for_frequency(
    map: &ArenaMap,
    pos: &mut Vec2,
    mut vel: Vec2,
    dt: f32,
    frequency: u16,
    bricks: &[Brick],
) -> bool {
    shared_weapon_terrain_step(map, pos, &mut vel, dt, frequency, bricks, false)
}

pub(crate) fn move_bouncing_point_step_for_frequency(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: &mut Vec2,
    dt: f32,
    frequency: u16,
    bricks: &[Brick],
) -> bool {
    shared_weapon_terrain_step(map, pos, vel, dt, frequency, bricks, true)
}

fn shared_weapon_terrain_step(
    map: &ArenaMap,
    pos: &mut Vec2,
    vel: &mut Vec2,
    dt: f32,
    frequency: u16,
    bricks: &[Brick],
    bounce: bool,
) -> bool {
    use crate::gameplay_rules::{RulesVector, weapon_motion};
    let mut position = RulesVector::new(pos.x, pos.y);
    let mut velocity = RulesVector::new(vel.x, vel.y);
    let contact =
        weapon_motion::step_terrain(&mut position, &mut velocity, dt, bounce, false, |x, y| {
            is_solid_world_for_frequency(map, vec2(x, y), frequency, bricks)
        });
    *pos = vec2(position.x, position.y);
    *vel = vec2(velocity.x, velocity.y);
    contact.any()
}

pub(crate) fn seed_map_doors_from_security(map: &mut ArenaMap, challenge: SecurityChallenge) {
    map.door_rng_seed = challenge.door_seed as i32;
}

pub(crate) fn door_update_count_and_next_timer(
    timer: f32,
    dt: f32,
    door_delay_seconds: f32,
) -> (usize, f32) {
    let delay = door_delay_seconds.max(0.01);
    let remaining = timer - dt;
    if remaining > 0.0 {
        return (0, remaining);
    }

    let overdue = -remaining;
    let count = (overdue / delay).floor() as usize + 1;
    let next_timer = delay - (overdue % delay);
    (count, next_timer)
}

pub(crate) fn advance_map_door_seed_steps(map: &mut ArenaMap, door_mode: i16, update_count: usize) {
    for _ in 0..update_count {
        map.door_seed = next_door_seed(map.door_seed, &mut map.door_rng_seed, door_mode);
    }
}

pub(crate) fn synchronize_map_doors_from_security(
    map: &mut ArenaMap,
    challenge: SecurityChallenge,
    door_mode: i16,
    door_delay_seconds: f32,
    server_tick: u32,
) -> f32 {
    // SECURITY is an idempotent absolute bind, not an incremental reseed.
    // Settings and map publication can legitimately replay the same cached
    // challenge after the local map has already advanced. Restart both parts
    // of the VIE door state before applying the challenge-relative elapsed
    // count; retaining the progressed byte while resetting only the RNG puts
    // the browser on a different, individually valid seed sequence.
    map.door_seed = u8::MAX;
    seed_map_doors_from_security(map, challenge);

    let delay_ticks = crate::continuum::seconds_to_ticks_i32(door_delay_seconds.max(0.01)).max(1);
    let elapsed_ticks = continuum_tick_diff(server_tick, challenge.timestamp).max(0);
    let update_count = (elapsed_ticks / delay_ticks) as usize;

    advance_map_door_seed_steps(map, door_mode, update_count);

    let remainder = elapsed_ticks % delay_ticks;
    crate::continuum::ticks_to_seconds_i32(delay_ticks - remainder)
}
