use super::*;

pub(super) fn first_safe_zone_position() -> (u16, u16) {
    let map = zone_map();
    let index = map
        .tiles
        .iter()
        .position(|&tile| tile == SAFE_TILE_ID)
        .expect("Chaos Zone should include safe tiles");
    let x = (((index % 1024) as f32 + 0.5) * TILE_SIZE_PX)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16;
    let y = (((index / 1024) as f32 + 0.5) * TILE_SIZE_PX)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16;
    (x, y)
}

pub(super) fn take_prize_request_payload(
    collectible_id: u32,
    x: u16,
    y: u16,
    prize_id: i16,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(11);
    payload.push(TAKE_PRIZE_REQUEST);
    payload.extend(collectible_id.to_le_bytes());
    payload.extend(x.to_le_bytes());
    payload.extend(y.to_le_bytes());
    payload.extend(prize_id.to_le_bytes());
    payload
}

pub(super) fn flag_request_payload(flag_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3);
    payload.push(FLAG_REQUEST);
    payload.extend(flag_id.to_le_bytes());
    payload
}

pub(super) fn attach_request_payload(player_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3);
    payload.push(ATTACH_REQUEST);
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(super) fn spectate_request_payload(player_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(3);
    payload.push(SPECTATE_REQUEST);
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(super) fn drop_brick_request_payload(x: u16, y: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(5);
    payload.push(DROP_BRICK_REQUEST);
    payload.extend(x.to_le_bytes());
    payload.extend(y.to_le_bytes());
    payload
}

pub(super) fn ball_pickup_request_payload(ball_id: u8, timestamp: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(6);
    payload.push(BALL_PICKUP_REQUEST);
    payload.push(ball_id);
    payload.extend(timestamp.to_le_bytes());
    payload
}

pub(super) fn ball_fire_request_payload(
    ball_id: u8,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    player_id: u16,
    _timestamp: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16);
    payload.push(BALL_FIRE_REQUEST);
    payload.push(ball_id);
    payload.extend(x.to_le_bytes());
    payload.extend(y.to_le_bytes());
    payload.extend(velocity_x.to_le_bytes());
    payload.extend(velocity_y.to_le_bytes());
    payload.extend(player_id.to_le_bytes());
    payload.extend(0_u32.to_le_bytes());
    payload
}

pub(super) fn ball_goal_request_payload(ball_id: u8, x: i16, y: i16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(6);
    payload.push(BALL_GOAL_REQUEST);
    payload.push(ball_id);
    payload.extend(x.to_le_bytes());
    payload.extend(y.to_le_bytes());
    payload
}

pub(super) fn server_goal_position_for_frequency(frequency: u16, should_count: bool) -> (i16, i16) {
    let mode = arena_soccer_mode();
    let map = zone_map();
    map.tiles
        .iter()
        .enumerate()
        .filter(|&(_, &tile)| tile == GOAL_TILE_ID)
        .map(|(index, _)| {
            let x = (((index % 1024) as f32 + 0.5) * TILE_SIZE_PX).round() as u16;
            let y = (((index / 1024) as f32 + 0.5) * TILE_SIZE_PX).round() as u16;
            (x, y)
        })
        .find(|&(x, y)| {
            world_position_touches_goal(x, y)
                && soccer_goal_counts_world(x, y, frequency, mode) == should_count
        })
        .map(|(x, y)| (x as i16, y as i16))
        .expect("Chaos Zone should include a matching soccer goal tile")
}
