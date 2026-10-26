use super::*;

const REFERENCE_GAME_ENERGY_HALF_WIDTH: f32 = 240.0;
const REFERENCE_GAME_ENERGY_SCREEN_MARGIN: f32 = 8.0;
pub(super) const REFERENCE_GAME_HEALTHBAR_WIDTH: f32 = 116.0;
pub(super) const REFERENCE_GAME_HEALTHBAR_HEIGHT: f32 = 28.0;
pub(super) const REFERENCE_GAME_HEALTHBAR_SOURCE: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 116.0,
    h: 28.0,
};

/// RTT from which the HUD ping turns yellow, and then red, so a player can
/// see that their connection, not the game, is behind late hits.
const PING_HIGH_MS: u32 = 150;
const PING_BAD_MS: u32 = 300;

/// The text colour for a high or bad ping; `None` keeps the ordinary readout.
pub(super) fn ping_warning_color(ping_ms: u32) -> Option<TextColor> {
    if ping_ms >= PING_BAD_MS {
        Some(TextColor::DarkRed)
    } else if ping_ms >= PING_HIGH_MS {
        Some(TextColor::Yellow)
    } else {
        None
    }
}

impl Game {
    pub(super) fn draw_player_list_frame_rate(&self) {
        let digits = self.frame_rate.digits();
        let size = self.assets.energy_font.frame_size * 0.5;
        let suffix = " fps";
        let right = screen_width() - 4.0;
        // Ping starts four pixels below the full-size energy digits. Leave
        // another four-pixel gap below that twelve-pixel readout.
        let y = self.assets.energy_font.frame_size.y
            + 8.0
            + size.y.max(self.assets.tall_font.line_height());
        let mut x =
            right - self.assets.tall_font.measure_width(suffix) - digits.len() as f32 * size.x;
        for byte in digits.bytes() {
            if byte.is_ascii_digit() {
                draw_texture_ex(
                    &self.assets.energy_font.texture,
                    x,
                    y,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(size),
                        source: Some(
                            self.assets
                                .energy_font
                                .source_for_frame((byte - b'0') as usize),
                        ),
                        ..Default::default()
                    },
                );
            } else {
                // The energy sheet contains digits only; use the matching
                // blue bitmap text for the decimal point/startup placeholder.
                self.assets.tall_font.draw_scaled(
                    &char::from(byte).to_string(),
                    x,
                    y,
                    TextColor::Blue,
                    TextAlign::Left,
                    1.0,
                );
            }
            x += size.x;
        }
        self.assets
            .tall_font
            .draw_scaled(suffix, right, y, TextColor::Blue, TextAlign::Right, 1.0);
    }

    pub(super) fn draw_connection_ping(&self) {
        if !self.networked_play {
            return;
        }
        let ping = crate::browser_metrics::connection_ping_ms();
        let suffix = " ms";
        let right = screen_width() - 4.0;
        let y = self.assets.energy_font.frame_size.y + 4.0;
        let suffix_width = self.assets.tall_font.measure_width(suffix);
        // The energy sprites are blue and do not tint cleanly, so a high
        // ping is drawn in the HUD text font's own yellow or red.
        if let Some(color) = ping.and_then(ping_warning_color) {
            let text = format!("{}{suffix}", ping.unwrap_or_default());
            self.assets
                .tall_font
                .draw_scaled(&text, right, y, color, TextAlign::Right, 1.0);
            return;
        }
        self.assets
            .tall_font
            .draw_scaled(suffix, right, y, TextColor::Blue, TextAlign::Right, 1.0);
        if let Some(ping) = ping {
            // Half-sized original energy sprites keep the HUD's digit style.
            let digits = ping.to_string();
            let size = self.assets.energy_font.frame_size * 0.5;
            let left = right - suffix_width - digits.len() as f32 * size.x;
            for (index, digit) in digits.bytes().enumerate() {
                draw_texture_ex(
                    &self.assets.energy_font.texture,
                    left + index as f32 * size.x,
                    y,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(size),
                        source: Some(
                            self.assets
                                .energy_font
                                .source_for_frame((digit - b'0') as usize),
                        ),
                        ..Default::default()
                    },
                );
            }
        } else {
            self.assets.tall_font.draw_scaled(
                "--",
                right - suffix_width,
                y,
                TextColor::Blue,
                TextAlign::Right,
                1.0,
            );
        }
    }
    pub(super) fn draw_energy_bar(&self, x: f32, tuning_energy_max: f32) {
        let visible_energy = self.visible_hud_energy();
        let center_x = x + REFERENCE_GAME_HEALTHBAR_WIDTH * 0.5;
        let visual_half_width = reference_game_energy_half_width(screen_width());
        let (max_energy_half_width, energy_half_width) = centered_energy_bar_half_widths(
            visible_energy,
            self.ship.max_energy,
            tuning_energy_max,
            visual_half_width,
        );
        let energy_percent = if self.ship.max_energy > 0.0 {
            (visible_energy / self.ship.max_energy).clamp(0.0, 1.0)
        } else {
            0.0
        };

        draw_rectangle(
            center_x - visual_half_width,
            10.0,
            visual_half_width * 2.0,
            10.0,
            Color::from_rgba(18, 20, 28, 220),
        );
        self.draw_centered_energy_gradient_strip(center_x, 10.0, max_energy_half_width, 2.0, 5);

        let frame_start = if energy_percent > 0.5 {
            0
        } else if energy_percent > 0.25 {
            14
        } else {
            28
        };
        let frame = frame_start + (((get_time() as f32).rem_euclid(1.4) / 1.4) * 14.0) as usize;
        self.draw_centered_energy_gradient_strip(
            center_x,
            14.0,
            energy_half_width,
            6.0,
            frame.min(frame_start + 13),
        );
    }

    fn draw_centered_energy_gradient_strip(
        &self,
        center_x: f32,
        y: f32,
        half_width: f32,
        height: f32,
        frame: usize,
    ) {
        self.draw_energy_gradient_strip(center_x - half_width, y, half_width, height, frame);
        self.draw_energy_gradient_strip(center_x, y, half_width, height, frame);
    }

    fn draw_energy_gradient_strip(&self, x: f32, y: f32, width: f32, height: f32, frame: usize) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        draw_texture_ex(
            &self.assets.bullet_trail.texture,
            x,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(width, height)),
                source: Some(self.assets.bullet_trail.source_for_frame(frame)),
                ..Default::default()
            },
        );
    }

    pub(super) fn draw_energy_digits(&self) {
        let mut energy = self.visible_hud_energy().max(0.0) as u32;
        let measured_digits = energy_digit_count(energy);
        let x_start =
            energy_digits_start_x(screen_width(), self.assets.energy_font.frame_size.x, energy);
        let mut digit_count = 0;
        let y = 0.0;

        while energy > 0 || digit_count == 0 {
            let digit = (energy % 10) as usize;
            let x = x_start
                + (measured_digits - digit_count - 1) as f32 * self.assets.energy_font.frame_size.x;
            draw_texture_ex(
                &self.assets.energy_font.texture,
                x,
                y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.energy_font.frame_size),
                    source: Some(self.assets.energy_font.source_for_frame(digit)),
                    ..Default::default()
                },
            );

            energy /= 10;
            digit_count += 1;
        }
    }
}

pub(super) fn energy_digit_count(mut energy: u32) -> u32 {
    let mut digits = 0;
    while energy > 0 || digits == 0 {
        energy /= 10;
        digits += 1;
    }
    digits
}

pub(super) fn energy_digits_start_x(screen_width: f32, digit_width: f32, energy: u32) -> f32 {
    screen_width - energy_digit_count(energy) as f32 * digit_width
}

fn centered_energy_bar_half_widths(
    energy: f32,
    max_energy: f32,
    tuning_energy_max: f32,
    visual_half_width: f32,
) -> (f32, f32) {
    let max_half_width = visual_half_width * (max_energy / tuning_energy_max).clamp(0.0, 1.0);
    let energy_half_width = visual_half_width * (energy / tuning_energy_max).clamp(0.0, 1.0);
    (max_half_width, energy_half_width)
}

pub(super) fn reference_game_energy_half_width(screen_width: f32) -> f32 {
    let available_half_width =
        (screen_width - REFERENCE_GAME_ENERGY_SCREEN_MARGIN * 2.0).max(0.0) * 0.5;
    REFERENCE_GAME_ENERGY_HALF_WIDTH.min(available_half_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_energy_bar_shrinks_equally_from_center() {
        assert_eq!(
            centered_energy_bar_half_widths(500.0, 1000.0, 1000.0, 240.0),
            (240.0, 120.0)
        );
        let (max_half_width, energy_half_width) =
            centered_energy_bar_half_widths(1000.0, 1700.0, 1700.0, 240.0);
        assert_eq!(max_half_width, 240.0);
        assert!((energy_half_width - 141.17648).abs() < 0.001);
    }

    #[test]
    fn reference_game_energy_bar_uses_original_width_when_viewport_allows() {
        assert_eq!(reference_game_energy_half_width(800.0), 240.0);
        assert_eq!(reference_game_energy_half_width(360.0), 172.0);
    }

    #[test]
    fn healthbar_frame_uses_first_reference_game_sheet_frame() {
        assert_eq!(REFERENCE_GAME_HEALTHBAR_WIDTH, 116.0);
        assert_eq!(REFERENCE_GAME_HEALTHBAR_HEIGHT, 28.0);
        assert_eq!(
            REFERENCE_GAME_HEALTHBAR_SOURCE,
            Rect::new(0.0, 0.0, 116.0, 28.0)
        );
    }

    #[test]
    fn energy_digits_are_right_aligned_like_reference_game() {
        assert_eq!(energy_digit_count(0), 1);
        assert_eq!(energy_digit_count(9), 1);
        assert_eq!(energy_digit_count(10), 2);
        assert_eq!(energy_digit_count(1700), 4);
        assert_eq!(energy_digits_start_x(1280.0, 16.0, 1700), 1216.0);
        assert_eq!(energy_digits_start_x(360.0, 16.0, 7), 344.0);
    }

    #[test]
    fn ping_turns_yellow_at_150_ms_and_red_at_300_ms() {
        assert_eq!(ping_warning_color(0), None);
        assert_eq!(ping_warning_color(149), None);
        assert_eq!(ping_warning_color(150), Some(TextColor::Yellow));
        assert_eq!(ping_warning_color(299), Some(TextColor::Yellow));
        assert_eq!(ping_warning_color(300), Some(TextColor::DarkRed));
        assert_eq!(ping_warning_color(2_000), Some(TextColor::DarkRed));
    }
}
