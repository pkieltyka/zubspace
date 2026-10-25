use super::*;

#[allow(dead_code)]
pub(crate) fn take_prize_request(
    collectible_id: u32,
    pos: Vec2,
    kind: PrizeKind,
    negative: bool,
) -> TakePrizeRequest {
    let prize_id = kind.protocol_id();
    TakePrizeRequest {
        collectible_id,
        x: protocol_world_coord(pos.x),
        y: protocol_world_coord(pos.y),
        prize_id: if negative { -prize_id } else { prize_id },
    }
}

#[allow(dead_code)]
pub(crate) fn drop_brick_request(pos: Vec2) -> DropBrickRequest {
    DropBrickRequest {
        x: protocol_world_coord(pos.x),
        y: protocol_world_coord(pos.y),
    }
}

#[allow(dead_code)]
pub(crate) fn ball_fire_request(
    ball_id: u8,
    pos: Vec2,
    velocity: Vec2,
    player_id: u16,
) -> BallFireRequest {
    BallFireRequest {
        ball_id,
        x: protocol_ball_coord(pos.x),
        y: protocol_ball_coord(pos.y),
        velocity_x: protocol_ball_velocity_component(velocity.x),
        velocity_y: protocol_ball_velocity_component(velocity.y),
        player_id,
        timestamp: 0,
    }
}

#[allow(dead_code)]
pub(crate) fn ball_goal_request(ball_id: u8, pos: Vec2) -> BallGoalRequest {
    BallGoalRequest {
        ball_id,
        x: protocol_signed_world_coord(pos.x),
        y: protocol_signed_world_coord(pos.y),
    }
}
