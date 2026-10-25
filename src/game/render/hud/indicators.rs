use super::*;

const REFERENCE_GAME_RIGHT_STATUS_ROWS: usize = 6;
const REFERENCE_GAME_LEFT_ITEM_ROWS: usize = 7;
const REFERENCE_GAME_RIGHT_STATUS_ROW_STEP: f32 = 25.0;
const REFERENCE_GAME_RIGHT_STATUS_Y_FRACTION: f32 = 0.57;
const REFERENCE_GAME_RIGHT_STATUS_TOP_OFFSET: f32 = 25.0 * 4.0;
const REFERENCE_GAME_LEFT_EMPTY_SOURCE: Rect = Rect {
    x: 152.0,
    y: 120.0,
    w: 4.0,
    h: 24.0,
};
const REFERENCE_GAME_RIGHT_EMPTY_SOURCE: Rect = Rect {
    x: 78.0,
    y: 120.0,
    w: 4.0,
    h: 24.0,
};

impl Game {
    pub(super) fn draw_weapon_status_indicators(&self) {
        if self.spectator_mode {
            // The reference game's SpectateView renders only the spectator's own X-radar
            // switch. The former ship's inventory and energy are stale here.
            self.draw_hud_icon(
                if self.ship.xradar { 36 } else { 37 },
                screen_width() - self.assets.icons.frame_size.x,
                screen_height() * REFERENCE_GAME_RIGHT_STATUS_Y_FRACTION + 1.0,
            );
            return;
        }
        let (x, mut y, _, _) = right_status_panel_bounds(
            screen_width(),
            screen_height(),
            self.assets.icons.frame_size.x,
            self.assets.icons.frame_size.y,
        );

        if let Some(frame) = gun_icon_frame(
            self.ship.gun_level,
            self.ship.multifire_capable,
            self.ship.multifire,
            self.ship.bouncing_bullets,
        ) {
            self.draw_hud_icon(frame, x, y);
        } else {
            self.draw_hud_empty_right_icon(x, y);
        }
        y += REFERENCE_GAME_RIGHT_STATUS_ROW_STEP;

        if let Some(frame) = bomb_icon_frame(
            self.ship.bomb_level,
            self.ship.proximity_bombs,
            self.ship.shrapnel_upgrades,
        ) {
            self.draw_hud_icon(frame, x, y);
        } else {
            self.draw_hud_empty_right_icon(x, y);
        }
        y += REFERENCE_GAME_RIGHT_STATUS_ROW_STEP;

        for frame in [
            status_icon_frame(self.ship.stealth, self.ship.stealth_capable, 32, 33),
            status_icon_frame(self.ship.cloak, self.ship.cloak_capable, 34, 35),
            status_icon_frame(self.ship.xradar, self.ship.xradar_capable, 36, 37),
            status_icon_frame(self.ship.antiwarp, self.ship.antiwarp_capable, 38, 39),
        ] {
            if let Some(frame) = frame {
                self.draw_hud_icon(frame, x, y);
            } else {
                self.draw_hud_empty_right_icon(x, y);
            }
            y += REFERENCE_GAME_RIGHT_STATUS_ROW_STEP;
        }
    }

    fn draw_hud_icon(&self, frame: usize, x: f32, y: f32) {
        draw_texture_ex(
            &self.assets.icons.texture,
            x,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(self.assets.icons.frame_size),
                source: Some(self.assets.icons.source_for_frame(frame)),
                ..Default::default()
            },
        );
    }

    fn draw_hud_empty_right_icon(&self, x: f32, y: f32) {
        draw_texture_ex(
            &self.assets.icons.texture,
            x + self.assets.icons.frame_size.x - REFERENCE_GAME_RIGHT_EMPTY_SOURCE.w,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(
                    REFERENCE_GAME_RIGHT_EMPTY_SOURCE.w,
                    REFERENCE_GAME_RIGHT_EMPTY_SOURCE.h,
                )),
                source: Some(REFERENCE_GAME_RIGHT_EMPTY_SOURCE),
                ..Default::default()
            },
        );
    }

    fn draw_hud_empty_left_icon(&self, y: f32) {
        draw_texture_ex(
            &self.assets.icons.texture,
            0.0,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(
                    REFERENCE_GAME_LEFT_EMPTY_SOURCE.w,
                    REFERENCE_GAME_LEFT_EMPTY_SOURCE.h,
                )),
                source: Some(REFERENCE_GAME_LEFT_EMPTY_SOURCE),
                ..Default::default()
            },
        );
    }

    pub(super) fn draw_item_indicators(&self) {
        let (_, mut y, _, _) = right_status_panel_bounds(
            screen_width(),
            screen_height(),
            self.assets.icons.frame_size.x,
            self.assets.icons.frame_size.y,
        );
        for (value, frame) in [
            (self.ship.bursts, 30),
            (self.ship.repels, 31),
            (self.ship.decoys, 40),
            (self.ship.thors, 41),
            (self.ship.bricks, 42),
            (self.ship.rockets, 43),
            (self.ship.portals, 46),
        ] {
            self.draw_item_indicator(value, frame, y);
            y += REFERENCE_GAME_RIGHT_STATUS_ROW_STEP;
        }
    }

    fn draw_item_indicator(&self, value: u32, frame: usize, y: f32) {
        if value == 0 {
            self.draw_hud_empty_left_icon(y);
            return;
        }

        draw_texture_ex(
            &self.assets.icons.texture,
            0.0,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(self.assets.icons.frame_size),
                source: Some(self.assets.icons.source_for_frame(frame)),
                ..Default::default()
            },
        );

        if value > 1 {
            let count_frame = value.min(10) as usize;
            draw_texture_ex(
                &self.assets.icon_counts.texture,
                23.0,
                y + 5.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.icon_counts.frame_size),
                    source: Some(self.assets.icon_counts.source_for_frame(count_frame)),
                    ..Default::default()
                },
            );
        }
    }

    pub(super) fn draw_timed_indicators(&self) {
        if let (Some(portal), Some(ttl)) = (self.active_portal(), self.active_portal_ttl()) {
            self.draw_timed_indicator(
                &self.assets.portal,
                portal.age,
                133.0,
                &timed_indicator_seconds_label(ttl),
                Color::from_rgba(255, 235, 95, 255),
            );
        }

        if self.ship.super_timer > 0.0 {
            self.draw_timed_indicator(
                &self.assets.super_effect,
                self.super_animation_age,
                101.0,
                &timed_indicator_seconds_label(self.ship.super_timer),
                Color::from_rgba(255, 235, 95, 255),
            );
        }

        if self.ship.shield_timer > 0.0 {
            let shield_lifetime = self.ship_tuning().shield_lifetime;
            self.draw_timed_indicator(
                &self.assets.shield_effect,
                self.shield_animation_age,
                85.0,
                &timed_indicator_percent_label(self.ship.shield_timer, shield_lifetime),
                Color::from_rgba(255, 235, 95, 255),
            );
        }

        if self.carrying_flag {
            // Multiplayer carry is authoritative state with no carry-expiry rule yet, so
            // `update_flags` holds `flag_carry_timer` at 0.0 (`game::flags`). The slot still
            // means "you are carrying the flag", so show it and put a number on it only when
            // a real countdown exists.
            let label = if self.flag_carry_timer > 0.0 {
                timed_indicator_seconds_label(self.flag_carry_timer)
            } else {
                String::new()
            };
            self.draw_timed_indicator(
                &self.assets.dropped_flag,
                get_time() as f32,
                117.0,
                &label,
                Color::from_rgba(165, 45, 40, 255),
            );
        }

        if self.spectator_mode {
            if let Some(flag_timer) = self
                .spectating_player_id
                .and_then(|player_id| self.network_world.players.get(&player_id))
                .and_then(|player| player.flag_timer)
                .filter(|timer| *timer > 0)
            {
                self.draw_timed_indicator(
                    &self.assets.dropped_flag,
                    get_time() as f32,
                    117.0,
                    &timed_indicator_centiseconds_label(flag_timer),
                    Color::from_rgba(165, 45, 40, 255),
                );
            }
        }
    }

    fn draw_timed_indicator(
        &self,
        sheet: &SpriteSheet,
        time: f32,
        y: f32,
        label: &str,
        label_color: Color,
    ) {
        let x = timed_indicator_icon_x(screen_width(), sheet.frame_size.x);
        draw_texture_ex(
            &sheet.texture,
            x,
            y - sheet.frame_size.y * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(sheet.frame_size),
                source: Some(sheet.source_at(time)),
                ..Default::default()
            },
        );
        let scale = 1.25;
        self.assets.tall_font.draw_tinted_scaled(
            label,
            x,
            y - 5.0,
            TextColor::White,
            label_color,
            TextAlign::Right,
            scale,
        );
    }
}

pub(super) fn left_item_panel_bounds(
    screen_height: f32,
    icon_width: f32,
    icon_count_width: f32,
    icon_height: f32,
) -> (f32, f32, f32, f32) {
    let y = reference_game_indicator_top_y(screen_height);
    let width = icon_width.max(23.0 + icon_count_width);
    let height = (REFERENCE_GAME_LEFT_ITEM_ROWS.saturating_sub(1) as f32
        * REFERENCE_GAME_RIGHT_STATUS_ROW_STEP)
        + icon_height;
    (0.0, y, width, height)
}

pub(super) fn right_status_panel_bounds(
    screen_width: f32,
    screen_height: f32,
    icon_width: f32,
    icon_height: f32,
) -> (f32, f32, f32, f32) {
    let x = screen_width - icon_width;
    let y = reference_game_indicator_top_y(screen_height);
    let height = (REFERENCE_GAME_RIGHT_STATUS_ROWS.saturating_sub(1) as f32
        * REFERENCE_GAME_RIGHT_STATUS_ROW_STEP)
        + icon_height;
    (x, y, icon_width, height)
}

fn reference_game_indicator_top_y(screen_height: f32) -> f32 {
    ((screen_height * REFERENCE_GAME_RIGHT_STATUS_Y_FRACTION) + 1.0)
        - REFERENCE_GAME_RIGHT_STATUS_TOP_OFFSET
}

fn timed_indicator_icon_x(screen_width: f32, frame_width: f32) -> f32 {
    screen_width - frame_width
}

fn gun_icon_frame(
    gun_level: u8,
    multifire_capable: bool,
    multifire: bool,
    bouncing: bool,
) -> Option<usize> {
    let level = usize::from(gun_level.checked_sub(1)?);
    let mut start = if multifire_capable {
        if multifire { 3 } else { 6 }
    } else {
        0
    };
    if bouncing {
        start += 9;
    }
    Some(start + level)
}

fn bomb_icon_frame(bomb_level: u8, proximity: bool, shrapnel_upgrades: u8) -> Option<usize> {
    let level = usize::from(bomb_level.checked_sub(1)?);
    let mut start = 18;
    if proximity && shrapnel_upgrades == 0 {
        start += 3;
    } else if proximity && shrapnel_upgrades > 0 {
        start += 9;
    } else if !proximity && shrapnel_upgrades > 0 {
        start += 6;
    }
    Some(start + level)
}

fn status_icon_frame(
    active: bool,
    available: bool,
    active_frame: usize,
    inactive_frame: usize,
) -> Option<usize> {
    if !available {
        return None;
    }
    Some(if active { active_frame } else { inactive_frame })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapon_status_icon_indices_match_reference_game_ranges() {
        assert_eq!(gun_icon_frame(0, false, false, false), None);
        assert_eq!(gun_icon_frame(1, false, false, false), Some(0));
        assert_eq!(gun_icon_frame(3, false, false, false), Some(2));
        assert_eq!(gun_icon_frame(1, true, true, false), Some(3));
        assert_eq!(gun_icon_frame(1, true, false, false), Some(6));
        assert_eq!(gun_icon_frame(1, false, false, true), Some(9));
        assert_eq!(gun_icon_frame(3, true, true, true), Some(14));
        assert_eq!(gun_icon_frame(3, true, false, true), Some(17));

        assert_eq!(bomb_icon_frame(0, false, 0), None);
        assert_eq!(bomb_icon_frame(1, false, 0), Some(18));
        assert_eq!(bomb_icon_frame(3, false, 0), Some(20));
        assert_eq!(bomb_icon_frame(1, true, 0), Some(21));
        assert_eq!(bomb_icon_frame(1, false, 1), Some(24));
        assert_eq!(bomb_icon_frame(1, true, 1), Some(27));

        assert_eq!(status_icon_frame(false, false, 32, 33), None);
        assert_eq!(status_icon_frame(false, true, 32, 33), Some(33));
        assert_eq!(status_icon_frame(true, true, 32, 33), Some(32));
    }

    #[test]
    fn right_status_panel_bounds_match_reference_game_positioning() {
        let (x, y, width, height) = right_status_panel_bounds(800.0, 600.0, 26.0, 24.0);

        assert_eq!(x, 774.0);
        assert_eq!(y, 243.0);
        assert_eq!(width, 26.0);
        assert_eq!(height, 149.0);
    }

    #[test]
    fn left_item_panel_bounds_match_reference_game_positioning() {
        let (x, y, width, height) = left_item_panel_bounds(600.0, 26.0, 11.0, 24.0);

        assert_eq!(x, 0.0);
        assert_eq!(y, 243.0);
        assert_eq!(width, 34.0);
        assert_eq!(height, 174.0);
    }

    #[test]
    fn timed_indicator_icon_is_flush_right_like_reference_game() {
        assert_eq!(timed_indicator_icon_x(800.0, 16.0), 784.0);
        assert_eq!(timed_indicator_icon_x(1366.0, 24.0), 1342.0);
    }
}
