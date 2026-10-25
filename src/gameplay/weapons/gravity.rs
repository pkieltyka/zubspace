use super::*;

pub(crate) fn wormhole_gravity_velocity(
    map: &ArenaMap,
    pos: Vec2,
    velocity: Vec2,
    gravity: i16,
    gravity_top_speed: f32,
    dt: f32,
) -> (Vec2, Option<f32>) {
    let sample = sample_wormhole_gravity_in_grid(
        MovementVector::new(pos.x, pos.y),
        gravity,
        gravity_top_speed,
        dt,
        TILE_SIZE,
        WormholeTileGrid {
            tiles: &map.tiles,
            width: map.width,
            height: map.height,
            wormhole_tile: WORMHOLE_TILE_ID,
        },
    );
    (
        velocity + vec2(sample.velocity_delta.x, sample.velocity_delta.y),
        sample.top_speed,
    )
}

pub(crate) fn bomb_wormhole_gravity_velocity(
    map: &ArenaMap,
    pos: Vec2,
    velocity: Vec2,
    gravity: i16,
    enabled: bool,
    dt: f32,
) -> Vec2 {
    if !enabled {
        return velocity;
    }

    wormhole_gravity_velocity(map, pos, velocity, gravity, 0.0, dt).0
}

pub(crate) fn remote_bomb_wormhole_gravity_velocity(
    map: &ArenaMap,
    pos: Vec2,
    velocity: Vec2,
    bomb_gravity: Option<i16>,
    dt: f32,
) -> Vec2 {
    let Some(gravity) = bomb_gravity else {
        return velocity;
    };
    bomb_wormhole_gravity_velocity(map, pos, velocity, gravity, true, dt)
}
