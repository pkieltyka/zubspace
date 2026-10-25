use super::*;

pub(crate) fn continuum_player_spawn_position(
    map: &ArenaMap,
    tuning: &ArenaTuning,
    frequency: u16,
    player_count: usize,
    ship_radius: f32,
    rng_seed: &mut i32,
) -> Vec2 {
    let configured_regions: Vec<_> = tuning
        .spawn_regions
        .iter()
        .copied()
        .filter(|region| region.is_configured())
        .collect();

    if !configured_regions.is_empty() {
        let index = frequency as usize % configured_regions.len();
        return continuum_region_spawn_position(
            map,
            configured_regions[index],
            ship_radius,
            rng_seed,
        )
        .unwrap_or_else(|| map.spawn_position());
    }

    continuum_default_spawn_position(
        map,
        tuning.radar_mode,
        tuning.warp_radius_limit,
        frequency,
        player_count,
        ship_radius,
        rng_seed,
    )
    .unwrap_or_else(|| map.spawn_position())
}

pub(crate) fn continuum_region_spawn_position(
    map: &ArenaMap,
    region: ContinuumSpawnRegion,
    ship_radius: f32,
    rng_seed: &mut i32,
) -> Option<Vec2> {
    let center = continuum_spawn_region_center(region);
    let mut position = tile_center(center);
    if map.can_fit_world(position, ship_radius) {
        // Keep the center as the deterministic fallback, matching the reference game.
    } else {
        position = map.spawn_position();
    }

    if region.radius == 0 {
        return Some(position);
    }

    let radius = region.radius as i32;
    for _ in 0..100 {
        let x_offset = (vie_rng_next(rng_seed) % (radius as u32 * 2)) as i32 - radius;
        let y_offset = (vie_rng_next(rng_seed) % (radius as u32 * 2)) as i32 - radius;
        let spawn = tile_center(ivec2(center.x + x_offset, center.y + y_offset));
        if map.can_fit_world(spawn, ship_radius) {
            return Some(spawn);
        }
    }

    Some(position)
}

pub(crate) fn continuum_spawn_region_center(region: ContinuumSpawnRegion) -> IVec2 {
    let x = continuum_spawn_center_coord(region.x);
    let y = continuum_spawn_center_coord(region.y);
    ivec2(x, y)
}

pub(crate) fn continuum_spawn_center_coord(coord: i16) -> i32 {
    if coord == 0 {
        MAP_TILES / 2
    } else if coord < 0 {
        coord as i32 + MAP_TILES
    } else {
        coord as i32
    }
}

pub(crate) fn continuum_default_spawn_position(
    map: &ArenaMap,
    radar_mode: u8,
    warp_radius_limit: u16,
    frequency: u16,
    player_count: usize,
    ship_radius: f32,
    rng_seed: &mut i32,
) -> Option<Vec2> {
    let fallback = map.spawn_position();
    for _ in 0..100 {
        let tile = match radar_mode {
            1 | 3 => {
                let x = ((frequency & 1) as i32 * 0x300) + (vie_rng_next(rng_seed) as u8 as i32);
                let y = (vie_rng_next(rng_seed) as u8 as i32) + 0x100;
                ivec2(x, y)
            }
            2 | 4 => {
                let x = ((frequency & 1) as i32 * 0x300) + (vie_rng_next(rng_seed) as u8 as i32);
                let y =
                    (((frequency / 2) & 1) as i32 * 0x300) + (vie_rng_next(rng_seed) as u8 as i32);
                ivec2(x, y)
            }
            _ => {
                let radius = continuum_default_spawn_radius(player_count, warp_radius_limit);
                let range = (radius - 2).max(1) as u32;
                let offset = (MAP_TILES - radius) / 2;
                let x = (vie_rng_next(rng_seed) % range) as i32 - 9
                    + offset
                    + (vie_rng_next(rng_seed) % 0x14) as i32;
                let y = (vie_rng_next(rng_seed) % range) as i32 - 9
                    + offset
                    + (vie_rng_next(rng_seed) % 0x14) as i32;
                ivec2(x, y)
            }
        };
        let spawn = tile_center(tile);
        if map.can_fit_world(spawn, ship_radius) {
            return Some(spawn);
        }
    }

    Some(fallback)
}
