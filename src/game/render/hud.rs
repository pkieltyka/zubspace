use super::super::*;

mod debug;
mod gauges;
mod indicators;
mod menu;
mod messages;
mod metrics;
mod network_notice;
mod scoreboard;

use gauges::{
    REFERENCE_GAME_HEALTHBAR_HEIGHT, REFERENCE_GAME_HEALTHBAR_SOURCE,
    REFERENCE_GAME_HEALTHBAR_WIDTH,
};

const HUD_TEXT_X: f32 = 16.0;
const HUD_TEXT_Y: f32 = 44.0;
const HUD_TEXT_SCALE: f32 = 1.0;
const HUD_LINE_SPACING: f32 = 14.0;
const HUD_RADAR_MARGIN: f32 = 12.0;
const HUD_MIN_CHARS: usize = 24;

fn connection_cooldown_text(remaining: Option<f64>) -> Option<String> {
    remaining.filter(|seconds| *seconds > 0.0).map(|remaining| {
        let seconds = remaining.ceil() as u32;
        format!("Connection cooldown: {}:{:02}", seconds / 60, seconds % 60)
    })
}

#[derive(Clone, Debug, PartialEq)]
struct HudTextLine {
    text: String,
    color: TextColor,
}

impl Game {
    pub(super) fn draw_hud(&self, local: &LocalRenderPrediction) {
        let tuning = self.visibility_ship_tuning();
        let energy_hud_x = (screen_width() - REFERENCE_GAME_HEALTHBAR_WIDTH) * 0.5;
        if !self.spectator_mode {
            self.draw_energy_bar(energy_hud_x, tuning.energy_max);
            draw_texture_ex(
                &self.assets.health_bar,
                energy_hud_x,
                0.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(
                        REFERENCE_GAME_HEALTHBAR_WIDTH,
                        REFERENCE_GAME_HEALTHBAR_HEIGHT,
                    )),
                    source: Some(REFERENCE_GAME_HEALTHBAR_SOURCE),
                    ..Default::default()
                },
            );
            self.draw_energy_digits();
        }
        self.draw_connection_ping();

        let speed = self.ship.vel.length();
        let mode = if self.player_respawn_timer > 0.0 {
            format!("Respawn {:.1}", self.player_respawn_timer)
        } else if self.spectator_mode {
            "Spectator".to_string()
        } else {
            format!("Ship {}", self.ship.ship_type + 1)
        };
        let visible_energy = self.visible_hud_energy();

        let radar_view = small_radar_view(
            self.map().width,
            self.map().height,
            self.ship.pos,
            screen_width(),
            screen_height(),
            self.tuning.map_zoom_factor,
        );
        let radar_origin = small_radar_origin(screen_width(), screen_height(), radar_view.dim_px);
        let radar_x = radar_origin.x;
        let hud_available_width = (radar_x - HUD_TEXT_X - HUD_RADAR_MARGIN).max(1.0);
        let max_chars = hud_max_chars(
            hud_available_width,
            self.assets
                .tall_font
                .measure_width_scaled("W", HUD_TEXT_SCALE),
        );
        let hud_lines =
            self.compact_hud_lines(&mode, visible_energy as i32, speed as i32, max_chars);
        for (index, line) in hud_lines.iter().enumerate() {
            self.assets.tall_font.draw_scaled(
                &line.text,
                HUD_TEXT_X,
                HUD_TEXT_Y + index as f32 * HUD_LINE_SPACING,
                line.color,
                TextAlign::Left,
                HUD_TEXT_SCALE,
            );
        }

        if let Some(network_status) = network_objective_status(
            &self.network_world.soccer_scores,
            self.network_world.personal_koth_timer,
            self.network_world.koth_timer,
            self.network_world.ufo_available,
        ) {
            let y = HUD_TEXT_Y + hud_lines.len() as f32 * HUD_LINE_SPACING;
            self.assets.tall_font.draw_scaled(
                &network_status,
                HUD_TEXT_X,
                y,
                TextColor::Green,
                TextAlign::Left,
                HUD_TEXT_SCALE,
            );
        }
        self.publish_hud_ui_metrics(
            local,
            energy_hud_x,
            &hud_lines,
            radar_x,
            radar_origin.y,
            radar_view.dim_px,
        );
        self.draw_network_notice();

        if !self.spectator_mode {
            self.draw_timed_indicators();
            self.draw_item_indicators();
        }
        self.draw_weapon_status_indicators();
        self.draw_chat();
        self.draw_notifications();
        let cooldown_text = connection_cooldown_text(
            self.client_authority
                .quality_hold_remaining(macroquad::miniquad::date::now()),
        );
        #[cfg(any(test, target_arch = "wasm32"))]
        let cooldown_text = cooldown_text.or_else(|| {
            self.quality_return_recovery
                .as_ref()
                .map(|attempt| attempt.progress_text(macroquad::miniquad::date::now()))
        });
        if let Some(text) = cooldown_text {
            self.assets.tall_font.draw_scaled(
                &text,
                screen_width() * 0.5,
                50.0,
                TextColor::Yellow,
                TextAlign::Center,
                1.0,
            );
        }
        if crate::browser_metrics::debug_mode() && self.arena_directory_visible {
            self.draw_arena_directory();
        }
        if !self.scoreboard_visible {
            self.draw_menu();
        }
        self.draw_radar();
        if !self.scoreboard_visible
            && (is_key_down(KeyCode::LeftAlt) || is_key_down(KeyCode::RightAlt))
        {
            self.draw_full_map_overlay();
        }
        // F9 toggles `developer_overlay` (`src/input.rs`), and the draw used to ignore that flag entirely, so the
        // panel - including its network status line - was painted in every build at every moment while the key
        // appeared to do nothing. Honour the toggle it already has.
        if crate::browser_metrics::debug_mode() && self.developer_overlay {
            self.draw_developer_overlay();
        }
        if self.scoreboard_visible {
            self.draw_scoreboard();
            self.draw_player_list_frame_rate();
        }
    }

    fn compact_hud_lines(
        &self,
        mode: &str,
        visible_energy: i32,
        speed: i32,
        max_chars: usize,
    ) -> Vec<HudTextLine> {
        if !crate::browser_metrics::debug_mode() {
            return Vec::new();
        }
        let mut source = vec![
            HudTextLine {
                text: format!(
                    "{}  E{:04}  S{:03}  Bnty {}",
                    mode, visible_energy, speed, self.player_bounty
                ),
                color: TextColor::White,
            },
            HudTextLine {
                text: format!(
                    "Goals {}-{}  Ball {}-{}  Bots {}",
                    self.flag_score,
                    self.bot_flag_score,
                    self.ball_score,
                    self.bot_ball_score,
                    self.bot_score
                ),
                color: TextColor::Green,
            },
            HudTextLine {
                text: self.map_and_status_line(),
                color: TextColor::White,
            },
            HudTextLine {
                text: format!(
                    "Gun{} Bomb{} Brst{:02} Repl{:02}",
                    self.ship.gun_level, self.ship.bomb_level, self.ship.bursts, self.ship.repels
                ),
                color: TextColor::Blue,
            },
            HudTextLine {
                text: format!(
                    "Wall{:02} Decy{:02} Thor{:02} Rckt{:02}",
                    self.ship.bricks, self.ship.decoys, self.ship.thors, self.ship.rockets
                ),
                color: TextColor::Blue,
            },
            HudTextLine {
                text: format!(
                    "Prtl{:02} Shrp{} R{:.3} T{:.1} S{:.1} C{:.1}",
                    self.ship.portals,
                    self.ship.shrapnel_upgrades,
                    self.ship.rotation_per_second,
                    self.ship.thrust_acceleration,
                    self.ship.top_speed,
                    self.ship.recharge_per_second
                ),
                color: TextColor::Blue,
            },
        ];

        let mut lines = Vec::new();
        for line in source.drain(..) {
            for text in wrap_hud_text(&line.text, max_chars) {
                lines.push(HudTextLine {
                    text,
                    color: line.color,
                });
            }
        }
        lines
    }

    fn map_and_status_line(&self) -> String {
        let mut status = format!("Map {}", self.map().name);
        let tags = [
            (self.carrying_flag, "FLAG"),
            (self.ship.multifire_capable && self.ship.multifire, "MULTI"),
            (self.ship.stealth, "STEALTH"),
            (self.ship.cloak, "CLOAK"),
            (self.ship.xradar, "XRADAR"),
            (self.ship.antiwarp, "ANTI"),
            (self.ship.proximity_bombs, "PROX"),
            (self.ship.bouncing_bullets, "BOUNCE"),
            (self.ship.super_timer > 0.0, "SUPER"),
            (self.ship.shield_timer > 0.0, "SHIELD"),
            (self.ship.engine_shutdown_timer > 0.0, "SHUTDOWN"),
            (self.in_safe_zone(), "SAFE"),
        ];
        let active = tags
            .iter()
            .filter_map(|(enabled, label)| enabled.then_some(*label))
            .collect::<Vec<_>>();
        if !active.is_empty() {
            status.push_str("  ");
            status.push_str(&active.join(" "));
        }
        status
    }
}

fn hud_max_chars(available_width: f32, glyph_width: f32) -> usize {
    ((available_width / glyph_width.max(1.0)).floor() as usize).max(HUD_MIN_CHARS)
}

fn wrap_hud_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_string();
        }

        while current.chars().count() > max_chars {
            let boundary = current
                .char_indices()
                .nth(max_chars)
                .expect("longer than max_chars")
                .0;
            let rest = current.split_off(boundary);
            lines.push(current);
            current = rest;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn notification_origin(screen_width: f32, screen_height: f32) -> Vec2 {
    let y_max = (screen_height - 180.0).max(130.0);
    vec2(
        (screen_width * NOTIFICATION_ANCHOR_X_FRACTION).clamp(16.0, 240.0),
        (screen_height * NOTIFICATION_ANCHOR_Y_FRACTION).clamp(130.0, y_max),
    )
}

fn notification_stack_position(
    screen_width: f32,
    screen_height: f32,
    width: f32,
    count: usize,
    line_height: f32,
) -> Vec2 {
    let origin = notification_origin(screen_width, screen_height);
    let max_x = (screen_width - width - 6.0).max(6.0);
    let max_y = (screen_height - count as f32 * line_height - 6.0).max(6.0);
    vec2(origin.x.min(max_x), origin.y.min(max_y))
}

fn notification_stack_inside(
    origin: Vec2,
    width: f32,
    count: usize,
    line_height: f32,
    screen_width: f32,
    screen_height: f32,
) -> bool {
    if count == 0 {
        return true;
    }

    origin.x >= 0.0
        && origin.y >= 0.0
        && origin.x + width <= screen_width
        && origin.y + count as f32 * line_height <= screen_height
}

#[cfg(test)]
mod tests;
