use super::super::zone::TILE_SIZE_PX;

pub(crate) fn soccer_goal_counts_world(x: u16, y: u16, frequency: u16, soccer_mode: u8) -> bool {
    soccer_goal_info_world(x, y, frequency, soccer_mode).0
}

pub(crate) fn soccer_goal_info_world(
    x: u16,
    y: u16,
    frequency: u16,
    soccer_mode: u8,
) -> (bool, Option<u16>) {
    let x = x as f32 / TILE_SIZE_PX;
    let y = y as f32 / TILE_SIZE_PX;
    match soccer_mode {
        0 => (true, None),
        1 => {
            let owner = u16::from(x >= 512.0);
            (frequency % 2 != owner, Some(owner))
        }
        2 => {
            let owner = u16::from(y >= 512.0);
            (frequency % 2 != owner, Some(owner))
        }
        3 => {
            let owner = quadrant_owner(x, y);
            (frequency % 4 != owner, Some(owner))
        }
        4 => (frequency % 4 == quadrant_owner(x, y), None),
        5 => {
            let owner = side_owner(x, y);
            (frequency % 4 != owner, Some(owner))
        }
        6 => (frequency % 4 == side_owner(x, y), None),
        _ => (false, None),
    }
}

fn quadrant_owner(x: f32, y: f32) -> u16 {
    match (x >= 512.0, y >= 512.0) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    }
}

fn side_owner(x: f32, y: f32) -> u16 {
    if x < y {
        if x + y < 1024.0 { 0 } else { 1 }
    } else if x + y < 1024.0 {
        2
    } else {
        3
    }
}
