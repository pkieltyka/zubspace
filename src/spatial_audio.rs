use macroquad::prelude::*;

pub(crate) fn positional_sound_radius_px(screen_width: f32, screen_height: f32) -> f32 {
    screen_width.max(screen_height).max(1.0) * 0.5
}

pub(crate) fn positional_sound_volume(
    listener: Vec2,
    source: Vec2,
    radius_px: f32,
    base_volume: f32,
) -> Option<f32> {
    if radius_px <= 0.0 || base_volume <= 0.0 {
        return None;
    }

    let volume = 1.0 - listener.distance(source) / radius_px;
    if volume <= 0.0 {
        None
    } else {
        Some((base_volume * volume).min(base_volume))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_volume_is_full_at_listener_and_silent_outside_radius() {
        assert_eq!(
            positional_sound_volume(Vec2::ZERO, Vec2::ZERO, 100.0, 0.4),
            Some(0.4)
        );
        assert_eq!(
            positional_sound_volume(Vec2::ZERO, vec2(100.0, 0.0), 100.0, 0.4),
            None
        );
        assert_eq!(
            positional_sound_volume(Vec2::ZERO, vec2(101.0, 0.0), 100.0, 0.4),
            None
        );
    }

    #[test]
    fn positional_volume_falls_linearly_with_distance() {
        assert_eq!(
            positional_sound_volume(Vec2::ZERO, vec2(50.0, 0.0), 100.0, 0.4),
            Some(0.2)
        );
    }

    #[test]
    fn positional_sound_radius_uses_larger_view_extent_like_reference_game() {
        assert_eq!(positional_sound_radius_px(800.0, 600.0), 400.0);
        assert_eq!(positional_sound_radius_px(640.0, 900.0), 450.0);
    }
}
