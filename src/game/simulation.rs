use super::*;

mod bots;
mod effects;
mod lifecycle;
mod powerball;
mod prizes;
mod projectiles;
mod remote;

fn nearest_sound_position(current: Option<Vec2>, listener: Vec2, candidate: Vec2) -> Option<Vec2> {
    match current {
        Some(current)
            if current.distance_squared(listener) <= candidate.distance_squared(listener) =>
        {
            Some(current)
        }
        _ => Some(candidate),
    }
}
