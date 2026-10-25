use crate::vie::{randomized_door_mode_seed, vie_rng_next};
use crate::{DOOR_FIRST_TILE_ID, DOOR_LAST_TILE_ID, DOOR_MODE_LOCAL_INCREMENT, OPEN_DOOR_TILE_ID};

pub(crate) fn seeded_door_tile(id: u8, seed: u8) -> Option<u8> {
    if !(DOOR_FIRST_TILE_ID..=DOOR_LAST_TILE_ID).contains(&id) {
        return None;
    }

    Some(reference_game_seeded_door_table(seed)[(id - DOOR_FIRST_TILE_ID) as usize])
}

#[allow(dead_code)] // server-bin uses bounded door-stream advancement instead
pub(crate) fn next_door_seed(current: u8, rng_seed: &mut i32, door_mode: i16) -> u8 {
    match door_mode {
        DOOR_MODE_LOCAL_INCREMENT => current.wrapping_add(1),
        -2 => vie_rng_next(rng_seed) as u8,
        -1 => randomized_door_mode_seed(rng_seed),
        mode if mode >= 0 => mode as u8,
        _ => current,
    }
}

pub(crate) fn reference_game_seeded_door_table(seed: u8) -> [u8; 8] {
    [
        if seed & 0x01 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            162
        },
        if seed & 0x02 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            163
        },
        if seed & 0x04 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            164
        },
        if seed & 0x08 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            165
        },
        if seed & 0x10 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            166
        },
        if seed & 0x20 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            167
        },
        if seed & 0x40 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            168
        },
        if seed & 0x80 == 0 {
            OPEN_DOOR_TILE_ID
        } else {
            169
        },
    ]
}
