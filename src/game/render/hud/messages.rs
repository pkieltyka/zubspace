use super::*;

impl Game {
    pub(super) fn draw_chat(&self) {
        if self.chat.messages.is_empty() && !self.chat.active {
            return;
        }

        let text_scale = 1.0;
        let edge_padding = 2.0;
        let line_height = self.assets.tall_font.line_height_scaled(text_scale);
        let available_width =
            (screen_width() - (screen_width() * 0.165 + 11.0) - 12.0 - edge_padding).max(8.0);
        let char_width = self
            .assets
            .tall_font
            .measure_width_scaled("W", text_scale)
            .max(1.0);
        let max_chars = (available_width / char_width).floor().max(1.0) as usize;
        let mut rows = Vec::new();
        for (index, message) in self.chat.messages.iter().enumerate() {
            let color = self
                .chat
                .message_colors
                .get(index)
                .copied()
                .unwrap_or(TextColor::Green);
            for text in wrap_hud_text(message, max_chars) {
                rows.push((text, color));
            }
        }
        let input = if self.chat.active {
            let cursor = if ((get_time() * 2.0) as i32) % 2 == 0 {
                "_"
            } else {
                ""
            };
            wrap_hud_text(&format!("> {}{}", self.chat.input, cursor), max_chars)
        } else {
            Vec::new()
        };
        let available_rows =
            ((screen_height() - 180.0).max(line_height) / line_height).floor() as usize;
        let limit = if self.menu_visible {
            64
        } else {
            (screen_height() / 100.0) as usize + 1
        };
        let message_limit = limit.min(available_rows).saturating_sub(input.len());
        let first = rows.len().saturating_sub(message_limit);
        let mut y = screen_height()
            - edge_padding
            - line_height * ((rows.len() - first + input.len()) as f32);
        for (text, color) in rows.iter().skip(first) {
            self.assets.tall_font.draw_scaled(
                text,
                edge_padding,
                y,
                *color,
                TextAlign::Left,
                text_scale,
            );
            y += line_height;
        }
        for text in input {
            self.assets.tall_font.draw_scaled(
                &text,
                edge_padding,
                y,
                TextColor::Blue,
                TextAlign::Left,
                text_scale,
            );
            y += line_height;
        }
    }

    pub(super) fn draw_notifications(&self) {
        if self.notifications.is_empty() {
            return;
        }

        let text_scale = NOTIFICATION_TEXT_SCALE;
        let line_height = self.assets.tall_font.line_height_scaled(text_scale);
        let max_width = self
            .notifications
            .iter()
            .map(|notification| {
                self.assets
                    .tall_font
                    .measure_width_scaled(&notification.message, text_scale)
            })
            .fold(0.0_f32, f32::max);
        let origin = notification_stack_position(
            screen_width(),
            screen_height(),
            max_width,
            self.notifications.len(),
            line_height,
        );
        let x = origin.x;
        let mut y = origin.y;
        for notification in &self.notifications {
            let fade = (notification.ttl / 0.35).clamp(0.0, 1.0);
            let mut shadow = Color::from_rgba(0, 0, 0, 210);
            shadow.a *= fade;
            let mut color = notification.color;
            color.a *= fade;
            self.assets.tall_font.draw_tinted_scaled(
                &notification.message,
                x + 1.0,
                y + 1.0,
                TextColor::White,
                shadow,
                TextAlign::Left,
                text_scale,
            );
            self.assets.tall_font.draw_tinted_scaled(
                &notification.message,
                x,
                y,
                TextColor::White,
                color,
                TextAlign::Left,
                text_scale,
            );
            y += line_height;
        }
    }
}
