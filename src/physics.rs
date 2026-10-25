use macroquad::prelude::{Vec2, vec2};

#[allow(dead_code)] // client-bin only
pub fn turn_towards_orientation(
    current: f32,
    target: f32,
    rotation_per_second: f32,
    dt: f32,
) -> f32 {
    let delta = ((target - current + 0.5).rem_euclid(1.0)) - 0.5;
    let max_step = rotation_per_second * dt;
    if delta.abs() <= max_step {
        target.rem_euclid(1.0)
    } else {
        (current + delta.signum() * max_step).rem_euclid(1.0)
    }
}

#[allow(dead_code)] // client-bin only
pub fn perpendicular(v: Vec2) -> Vec2 {
    vec2(-v.y, v.x)
}

#[allow(dead_code)] // client bot simulation only
pub fn truncate(v: Vec2, max_len: f32) -> Vec2 {
    let len = v.length();
    if len > max_len && len > 0.0 {
        v / len * max_len
    } else {
        v
    }
}
