use super::*;

mod layout;
use layout::{scoreboard_layout, scoreboard_name};

impl Game {
    pub(in crate::game) fn player_list_rows(&self) -> Vec<crate::scoreboard::ScoreboardEntry> {
        let mut rows = scoreboard_entries(
            self.local_player_name(),
            self.bot_score,
            self.player_deaths,
            self.player_bounty,
            self.flag_score,
            self.ball_score,
            self.ship.ship_type,
            self.ship.energy,
            &self.bots,
            &self.network_world,
        );
        rows.sort_by(|a, b| {
            b.local.cmp(&a.local).then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
        });
        rows
    }

    pub(in crate::game) fn player_list_max_scroll(&self, width: f32, height: f32) -> usize {
        let rows = self.player_list_rows().len();
        rows.saturating_sub(scoreboard_layout(width, height, rows).visible_player_rows)
    }

    pub(in crate::game) fn player_list_geometry(
        width: f32,
        height: f32,
        rows: usize,
    ) -> (Rect, usize) {
        let layout = scoreboard_layout(width, height, rows);
        (
            Rect::new(layout.x, layout.y, layout.width, layout.height),
            layout.visible_player_rows,
        )
    }

    pub(super) fn draw_scoreboard(&self) {
        let rows = self.player_list_rows();
        let layout = scoreboard_layout(screen_width(), screen_height(), rows.len());
        if layout.visible_player_rows == 0 {
            return;
        }
        draw_rectangle(
            layout.x,
            layout.y,
            layout.width,
            layout.height,
            Color::from_rgba(8, 11, 17, 235),
        );
        draw_rectangle_lines(
            layout.x,
            layout.y,
            layout.width,
            layout.height,
            1.0,
            Color::from_rgba(155, 175, 195, 200),
        );
        self.assets.tall_font.draw_scaled(
            &rows.len().to_string(),
            layout.x + 49.0,
            layout.y + 1.0,
            TextColor::Green,
            TextAlign::Center,
            1.0,
        );
        for (label, right, stats) in [
            ("Score", layout.score_right, false),
            ("W", layout.kills_right, true),
            ("L", layout.deaths_right, true),
            ("R", layout.rating_right, true),
            ("Ave", layout.average_right, true),
        ] {
            let y = layout.y
                + 1.0
                + if stats && layout.wrapped_rows {
                    12.0
                } else {
                    0.0
                };
            self.assets.tall_font.draw_scaled(
                label,
                right,
                y,
                TextColor::Green,
                TextAlign::Right,
                1.0,
            );
        }
        draw_line(
            layout.x,
            layout.y + layout.header_height - 2.0,
            layout.x + layout.width,
            layout.y + layout.header_height - 2.0,
            1.0,
            Color::from_rgba(155, 175, 195, 200),
        );
        let start = self
            .scoreboard_scroll
            .offset
            .min(rows.len().saturating_sub(layout.visible_player_rows));
        for (index, row) in rows
            .iter()
            .skip(start)
            .take(layout.visible_player_rows)
            .enumerate()
        {
            if start + index == self.scoreboard_scroll.selected {
                let y = layout.y + layout.header_height + index as f32 * layout.row_height;
                // The reference game puts its selected-player marker before the name.
                draw_triangle(
                    vec2(layout.x + 2.0, y + 2.0),
                    vec2(layout.x + 2.0, y + 10.0),
                    vec2(layout.x + 7.0, y + 6.0),
                    Color::from_rgba(175, 215, 240, 255),
                );
            }
            self.assets.tall_font.draw_scaled(
                &scoreboard_name(row),
                layout.text_x,
                layout.y + layout.header_height + index as f32 * layout.row_height,
                if row.local {
                    TextColor::Yellow
                } else {
                    TextColor::White
                },
                TextAlign::Left,
                1.0,
            );
            let row_y = layout.y + layout.header_height + index as f32 * layout.row_height;
            let color = if row.local {
                TextColor::Yellow
            } else {
                TextColor::White
            };
            for (value, right, stats) in [
                (row.total_score().to_string(), layout.score_right, false),
                (row.kills.to_string(), layout.kills_right, true),
                (row.deaths.to_string(), layout.deaths_right, true),
                (row.rating().to_string(), layout.rating_right, true),
                (
                    format!("{:.1}", row.average_bounty()),
                    layout.average_right,
                    true,
                ),
            ] {
                let y = row_y
                    + if stats && layout.wrapped_rows {
                        12.0
                    } else {
                        0.0
                    };
                self.assets
                    .tall_font
                    .draw_scaled(&value, right, y, color, TextAlign::Right, 1.0);
            }
        }
        if layout.hidden_player_rows > 0 {
            let track = layout.visible_player_rows as f32 * layout.row_height;
            let thumb =
                (track * layout.visible_player_rows as f32 / rows.len().max(1) as f32).max(2.0);
            let progress =
                start as f32 / rows.len().saturating_sub(layout.visible_player_rows).max(1) as f32;
            draw_rectangle(
                layout.x + layout.width - 3.0,
                layout.y + layout.header_height,
                2.0,
                track,
                Color::from_rgba(80, 100, 125, 160),
            );
            draw_rectangle(
                layout.x + layout.width - 3.0,
                layout.y + layout.header_height + progress * (track - thumb),
                2.0,
                thumb,
                WHITE,
            );
            if layout.footer_height > 0.0 {
                self.assets.tall_font.draw_scaled(
                    &format!(
                        "{}-{} / {}  UP/DOWN",
                        start + 1,
                        start + layout.visible_player_rows,
                        rows.len()
                    ),
                    layout.text_x,
                    layout.y + layout.header_height + track + 1.0,
                    TextColor::Green,
                    TextAlign::Left,
                    1.0,
                );
            }
        }
    }

    pub(super) fn draw_arena_directory(&self) {
        if self.network_world.arena_directory.is_empty() {
            return;
        }

        let rows = arena_directory_rows(&self.network_world.arena_directory, 12);
        let width = (screen_width() * 0.24).clamp(260.0, 360.0);
        let row_h = 20.0;
        let height = 48.0 + row_h * rows.len() as f32;
        let scoreboard_rows = scoreboard_entries(
            self.local_player_name(),
            self.bot_score,
            self.player_deaths,
            self.player_bounty,
            self.flag_score,
            self.ball_score,
            self.ship.ship_type,
            self.ship.energy,
            &self.bots,
            &self.network_world,
        );
        let scoreboard_layout =
            scoreboard_layout(screen_width(), screen_height(), scoreboard_rows.len());
        let (x, y) = arena_directory_panel_position(
            screen_width(),
            screen_height(),
            width,
            height,
            scoreboard_layout.x + scoreboard_layout.width,
            scoreboard_layout.y + scoreboard_layout.height,
        );

        draw_rectangle(x, y, width, height, Color::from_rgba(8, 11, 17, 225));
        draw_rectangle_lines(
            x,
            y,
            width,
            height,
            1.0,
            Color::from_rgba(155, 175, 195, 200),
        );
        self.assets.tall_font.draw_scaled(
            "Arena Directory",
            x + 14.0,
            y + 12.0,
            TextColor::Yellow,
            TextAlign::Left,
            1.5,
        );
        self.assets.tall_font.draw_scaled(
            "Arena                  Pop",
            x + 14.0,
            y + 38.0,
            TextColor::Blue,
            TextAlign::Left,
            1.25,
        );

        let mut row_y = y + 68.0;
        for row in rows {
            self.assets.tall_font.draw_scaled(
                &row,
                x + 14.0,
                row_y - 12.0,
                TextColor::White,
                TextAlign::Left,
                1.25,
            );
            row_y += row_h;
        }
    }
}
