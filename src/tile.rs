use macroquad::prelude::*;

use crate::door::seeded_door_tile;
use crate::{
    DOOR_FIRST_TILE_ID, DOOR_LAST_TILE_ID, FLAG_TILE_ID, GOAL_TILE_ID, OPEN_DOOR_TILE_ID,
    TILE_SIZE, WORMHOLE_TILE_ID,
};

pub(crate) const DOOR_ANIMATION_DURATION: f32 = 0.4;

pub(crate) fn world_to_tile(pos: Vec2) -> IVec2 {
    ivec2(
        (pos.x / TILE_SIZE).floor() as i32,
        (pos.y / TILE_SIZE).floor() as i32,
    )
}

pub(crate) fn tile_center(tile: IVec2) -> Vec2 {
    vec2(
        (tile.x as f32 + 0.5) * TILE_SIZE,
        (tile.y as f32 + 0.5) * TILE_SIZE,
    )
}

pub(crate) fn brick_line_tiles(start: Vec2, end: Vec2) -> Vec<IVec2> {
    let delta = end - start;
    let distance = delta.length();
    let direction = if distance > 0.0 {
        delta / distance
    } else {
        Vec2::ZERO
    };
    let mut tiles = Vec::new();
    let mut position = start;
    let mut step = 0.0;
    while step <= distance {
        let tile = ivec2(position.x as i32, position.y as i32);
        if tiles.last().copied() != Some(tile) {
            tiles.push(tile);
        }
        position += direction;
        step += 1.0;
    }
    if tiles.is_empty() {
        tiles.push(ivec2(start.x as i32, start.y as i32));
    }
    tiles
}

pub(crate) fn circle_overlaps_tile(pos: Vec2, radius_px: f32, tile_x: i32, tile_y: i32) -> bool {
    let min = vec2(tile_x as f32 * TILE_SIZE, tile_y as f32 * TILE_SIZE);
    let max = min + vec2(TILE_SIZE, TILE_SIZE);
    let closest = vec2(pos.x.clamp(min.x, max.x), pos.y.clamp(min.y, max.y));

    pos.distance_squared(closest) <= radius_px * radius_px
}

pub(crate) fn tile_source(tile: u8) -> Rect {
    let index = tile.saturating_sub(1) as usize;
    let columns = 19;
    Rect::new(
        (index % columns) as f32 * TILE_SIZE,
        (index / columns) as f32 * TILE_SIZE,
        TILE_SIZE,
        TILE_SIZE,
    )
}

pub(crate) fn is_door_tile(id: u8) -> bool {
    (DOOR_FIRST_TILE_ID..=DOOR_LAST_TILE_ID).contains(&id)
}

pub(crate) fn door_animation_source(door_id: u8, time: f32) -> Rect {
    let door_group = (door_id.saturating_sub(DOOR_FIRST_TILE_ID) / 4).min(1);
    let frame = ((time.rem_euclid(DOOR_ANIMATION_DURATION) / DOOR_ANIMATION_DURATION) * 4.0)
        .floor()
        .min(3.0) as u8;
    tile_source(DOOR_FIRST_TILE_ID + door_group * 4 + frame)
}

pub(crate) fn should_draw_base_tile(id: u8) -> bool {
    id != 0
        && id != FLAG_TILE_ID
        && id != GOAL_TILE_ID
        && id != OPEN_DOOR_TILE_ID
        && !is_door_tile(id)
        && id <= 190
}

pub(crate) fn is_solid_tile(id: u8) -> bool {
    if id == 0 {
        return false;
    }
    if (162..=169).contains(&id) {
        return true;
    }
    if id < 170 {
        return true;
    }
    if id == WORMHOLE_TILE_ID {
        return false;
    }
    if (192..=240).contains(&id) {
        return true;
    }
    if (242..=252).contains(&id) {
        return true;
    }

    false
}

pub(crate) fn is_solid_tile_with_doors(id: u8, door_seed: u8) -> bool {
    if let Some(seeded) = seeded_door_tile(id, door_seed) {
        return seeded != OPEN_DOOR_TILE_ID;
    }
    is_solid_tile(id)
}

pub(crate) fn animated_tile_size(id: u8) -> Option<usize> {
    match id {
        GOAL_TILE_ID | 216 | 218 | FLAG_TILE_ID => Some(1),
        217 => Some(2),
        219 => Some(6),
        WORMHOLE_TILE_ID => Some(5),
        _ => None,
    }
}
