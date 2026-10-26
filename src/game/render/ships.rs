use super::*;

mod bots;
mod local;
mod network;

impl Game {
    fn arena_score_leader_id(&self) -> Option<u16> {
        crate::scoreboard::arena_score_leader(self.network_world.players.values().map(|player| {
            (
                player.player_id,
                player.ship,
                player.kill_points,
                player.flag_points,
            )
        }))
    }

    fn draw_camera_relative_leader_badge(&self, offset: Vec2, label: &str) {
        let scale = SHIP_LABEL_FONT_SIZE as f32 / self.assets.tall_font.line_height();
        let anchor = camera_relative_ship_label_position(
            &self.camera,
            offset,
            vec2(screen_width(), screen_height()),
        );
        // Keep the name's existing anchor and native bitmap font unchanged.
        // The 16px badge sits immediately left of the complete name label.
        let x = (anchor.x - self.assets.tall_font.measure_width_scaled(label, scale) * 0.5 - 20.0)
            .floor();
        let y = (anchor.y - 8.0).floor();
        set_default_camera();
        draw_texture_ex(
            &self.assets.leader_badge.texture,
            x,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(self.assets.leader_badge.frame_size),
                source: Some(self.assets.leader_badge.source_at(get_time() as f32)),
                ..Default::default()
            },
        );
        set_camera(&self.camera);
    }

    pub(in crate::game::render) fn draw_ship_label_text(
        &self,
        pos: Vec2,
        label: &str,
        color: TextColor,
    ) {
        self.draw_camera_relative_ship_label_text(pos - self.camera.target, label, color);
    }

    fn draw_camera_relative_ship_label_text(&self, offset: Vec2, label: &str, color: TextColor) {
        let scale = SHIP_LABEL_FONT_SIZE as f32 / self.assets.tall_font.line_height();
        let pos = camera_relative_ship_label_position(
            &self.camera,
            offset,
            vec2(screen_width(), screen_height()),
        );
        let y = pos.y - self.assets.tall_font.line_height_scaled(scale) * 0.5;
        set_default_camera();
        self.assets.tall_font.draw_tinted_scaled(
            label,
            pos.x + 1.0,
            y + 1.0,
            TextColor::White,
            Color::from_rgba(0, 0, 0, 210),
            TextAlign::Center,
            scale,
        );
        self.assets
            .tall_font
            .draw_scaled(label, pos.x, y, color, TextAlign::Center, scale);
        set_camera(&self.camera);
    }

    pub(in crate::game::render) fn draw_ship_label(&self, pos: Vec2, label: &str, color: Color) {
        self.draw_camera_relative_ship_label(pos - self.camera.target, label, color);
    }

    fn draw_camera_relative_ship_label(&self, offset: Vec2, label: &str, color: Color) {
        let scale = SHIP_LABEL_FONT_SIZE as f32 / self.assets.tall_font.line_height();
        let pos = camera_relative_ship_label_position(
            &self.camera,
            offset,
            vec2(screen_width(), screen_height()),
        );
        let y = pos.y - self.assets.tall_font.line_height_scaled(scale) * 0.5;
        set_default_camera();
        self.assets.tall_font.draw_tinted_scaled(
            label,
            pos.x + 1.0,
            y + 1.0,
            TextColor::White,
            Color::from_rgba(0, 0, 0, 210),
            TextAlign::Center,
            scale,
        );
        self.assets.tall_font.draw_tinted_scaled(
            label,
            pos.x,
            y,
            TextColor::White,
            color,
            TextAlign::Center,
            scale,
        );
        set_camera(&self.camera);
    }

    pub(in crate::game::render) fn draw_emp_spark_at(&self, pos: Vec2, time: f32) {
        draw_texture_ex(
            &self.assets.emp_spark.texture,
            pos.x - 20.0,
            pos.y - 20.0,
            Color::from_rgba(180, 195, 255, 230),
            DrawTextureParams {
                dest_size: Some(self.assets.emp_spark.frame_size),
                source: Some(self.assets.emp_spark.source_at(time)),
                ..Default::default()
            },
        );
    }

    pub(in crate::game::render) fn draw_ship_sprite(
        &self,
        pos: Vec2,
        orientation: f32,
        ship_type: usize,
        color: Color,
    ) {
        let frame = ship_sprite_frame(orientation);
        if let Some(texture) = self.active_lvz_ship_texture(ship_type) {
            let frame_size = lvz_single_ship_frame_size(texture.width, texture.height);
            let source =
                lvz_source_for_frame(frame_size, frame, (texture.width / frame_size.x) as usize);
            draw_texture_ex(
                &texture.texture,
                pos.x - frame_size.x * 0.5,
                pos.y - frame_size.y * 0.5,
                color,
                DrawTextureParams {
                    dest_size: Some(frame_size),
                    source: Some(source),
                    ..Default::default()
                },
            );
            return;
        }

        let index = ship_type * 40 + frame;
        let columns = (self.assets.ships.width() / SHIP_FRAME) as usize;
        let source = Rect::new(
            (index % columns) as f32 * SHIP_FRAME,
            (index / columns) as f32 * SHIP_FRAME,
            SHIP_FRAME,
            SHIP_FRAME,
        );

        draw_texture_ex(
            &self.assets.ships,
            pos.x - SHIP_FRAME * 0.5,
            pos.y - SHIP_FRAME * 0.5,
            color,
            DrawTextureParams {
                dest_size: Some(vec2(SHIP_FRAME, SHIP_FRAME)),
                source: Some(source),
                ..Default::default()
            },
        );
    }

    fn active_lvz_ship_texture(&self, ship_type: usize) -> Option<&LvzTexture> {
        self.active_lvz_texture_by_stem(&format!("ship{}", ship_type + 1))
    }

    pub(in crate::game::render) fn draw_turret_sprite(
        &self,
        pos: Vec2,
        orientation: f32,
        color: Color,
    ) {
        let frame = ship_sprite_frame(orientation);
        let source = self.assets.turrets.source_for_frame(frame);

        draw_texture_ex(
            &self.assets.turrets.texture,
            pos.x - self.assets.turrets.frame_size.x * 0.5,
            pos.y - self.assets.turrets.frame_size.y * 0.5,
            color,
            DrawTextureParams {
                dest_size: Some(self.assets.turrets.frame_size),
                source: Some(source),
                ..Default::default()
            },
        );
    }

    pub(in crate::game::render) fn draw_flag_sprite(&self, pos: Vec2, time: f32, team: bool) {
        self.draw_flag_sprite_at(pos, time, team);
    }
}

fn camera_relative_ship_label_position(camera: &Camera2D, offset: Vec2, screen: Vec2) -> Vec2 {
    // Project the camera-relative delta with the linear transform. Projecting
    // target+offset as an absolute point cancels large f32 world terms only
    // after scaling; around half pixels that flips rounding by one pixel.
    // Camera offset is already in clip space and remains part of the result.
    let projected =
        camera.matrix().transform_vector3(offset.extend(0.0)) + camera.offset.extend(0.0);
    vec2(
        (projected.x * 0.5 + 0.5) * screen.x,
        (0.5 - projected.y * 0.5) * screen.y,
    )
    .round()
}

pub(super) fn ship_sprite_frame(orientation: f32) -> usize {
    usize::from(crate::continuum_position::protocol_direction(orientation))
}

fn ship_label_team_color() -> TextColor {
    TextColor::Yellow
}

fn ship_label_enemy_color() -> TextColor {
    TextColor::Blue
}

fn ship_label_objective_color() -> TextColor {
    TextColor::DarkRed
}

pub(super) fn ship_label_color_for_frequency(
    player_frequency: u16,
    viewer_frequency: u16,
    objective_carrier: bool,
) -> TextColor {
    if objective_carrier {
        ship_label_objective_color()
    } else if player_frequency == viewer_frequency {
        ship_label_team_color()
    } else {
        ship_label_enemy_color()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ship_label_anchor_does_not_jitter_as_follow_camera_moves() {
        for screen in [
            vec2(1885.0, 1279.0),
            vec2(1440.0, 1000.0),
            vec2(1279.0, 721.0),
        ] {
            let mut camera = Camera2D {
                zoom: vec2(2.0 / screen.x, 2.0 / screen.y),
                ..Default::default()
            };
            for offset in [vec2(0.0, -34.0), vec2(0.0, -48.0)] {
                let expected = camera_relative_ship_label_position(&camera, offset, screen);
                for step in 0..4096 {
                    camera.target =
                        vec2(8000.0 + step as f32 * 0.123, 8100.0 + step as f32 * 0.349);
                    assert_eq!(
                        camera_relative_ship_label_position(&camera, offset, screen),
                        expected,
                        "camera-relative name/energy anchor must be translation invariant at {screen:?}, {:?}",
                        camera.target
                    );
                }
            }
        }
    }

    #[test]
    fn turning_ship_sprite_uses_the_same_lower_bin_as_its_gun() {
        // The reference game's PlayerManager::Render and WeaponManager::FireWeapons
        // both cast orientation * 40 to u8. Nearest-frame rounding shows the
        // next nine-degree nose before the gun can face in that direction.
        assert_eq!(ship_sprite_frame(0.019), 0);
        for direction in 0..40 {
            for part in [0.1_f32, 0.49, 0.51, 0.9] {
                let orientation = (direction as f32 + part) / 40.0;
                assert_eq!(ship_sprite_frame(orientation), direction);
                assert_eq!(
                    ship_sprite_frame(orientation),
                    usize::from(crate::continuum_position::protocol_direction(orientation))
                );
            }
        }
        assert_eq!(ship_sprite_frame(0.999), 39);
        assert_eq!(ship_sprite_frame(1.0), 0);
    }

    #[test]
    fn ship_label_color_matches_reference_game_team_enemy_priority() {
        assert_eq!(
            ship_label_color_for_frequency(7, 7, false),
            ship_label_team_color()
        );
        assert_eq!(
            ship_label_color_for_frequency(8, 7, false),
            ship_label_enemy_color()
        );
        assert_eq!(
            ship_label_color_for_frequency(7, 7, true),
            ship_label_objective_color()
        );
        assert_eq!(
            ship_label_color_for_frequency(8, 7, true),
            ship_label_objective_color()
        );
    }
}
