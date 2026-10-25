use macroquad::prelude::*;

use crate::{WEAPON_LEVEL_MAX, WEAPON_LEVEL_MIN};

/// Clamp a 1-indexed per-shot weapon level to the representable range [1, 4].
/// This is not for arena settings capability fields, where 0 means no weapon.
#[allow(dead_code)]
pub(crate) fn clamp_weapon_level(level: u8) -> u8 {
    level.clamp(WEAPON_LEVEL_MIN, WEAPON_LEVEL_MAX)
}

pub(crate) fn orientation_to_heading(orientation: f32) -> Vec2 {
    let (x, y) = crate::continuum_position::orientation_to_heading_components(orientation);
    vec2(x, y)
}

pub(crate) fn canonical_discrete_heading(orientation: f32) -> Vec2 {
    let (x, y) = crate::continuum_position::canonical_discrete_heading_components(orientation);
    vec2(x, y)
}

// `protocol_direction` lives in `continuum_position.rs` next to its
// inverse `protocol_direction_to_orientation`. Re-exported here so existing
// `math::*` callers keep working.
#[allow(dead_code)]
pub(crate) use crate::continuum_position::protocol_direction;

#[allow(dead_code)]
pub(crate) use crate::continuum_position::protocol_position_coord;

#[allow(dead_code)]
pub(crate) fn protocol_world_coord(value: f32) -> u16 {
    value.trunc().clamp(0.0, u16::MAX as f32) as u16
}

#[allow(dead_code)]
pub(crate) fn protocol_signed_world_coord(value: f32) -> i16 {
    value.trunc().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

// `protocol_velocity_component` (encode) lives in `continuum_position.rs`
// next to its decoder `protocol_velocity_pixels_per_second`. Re-exported
// here so existing math.rs callers keep compiling.
pub(crate) use crate::continuum_position::protocol_velocity_component;

#[allow(dead_code)]
pub(crate) fn protocol_ball_coord(value: f32) -> u16 {
    (value * 16.0).trunc().clamp(0.0, u16::MAX as f32) as u16
}

#[allow(dead_code)]
pub(crate) fn protocol_ball_velocity_component(value: f32) -> i16 {
    (value * 160.0)
        .trunc()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

pub(crate) fn heading_to_orientation(heading: Vec2) -> f32 {
    ((heading.y.atan2(heading.x) + std::f32::consts::FRAC_PI_2) / std::f32::consts::TAU)
        .rem_euclid(1.0)
}

/// Convert a 1-indexed weapon level to a 0-indexed array index. This is
/// just `weapon_level_to_bits(level) as usize` — the body lived here for
/// historical reasons; route through the canonical helper so we can never
/// drift from the wire encoding.
pub(crate) fn weapon_level_index(level: u8) -> usize {
    crate::continuum_weapon::weapon_level_to_bits(level) as usize
}

pub(crate) fn hash_grid(x: i32, y: i32, salt: u32) -> u32 {
    let mut value = (x as u32).wrapping_mul(0x9e37_79b9)
        ^ (y as u32).wrapping_mul(0x85eb_ca6b)
        ^ salt.wrapping_mul(0xc2b2_ae35);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}
