use super::*;

impl Game {
    pub(in crate::game::render) fn draw_full_map_overlay(&self) {
        let map = self.map();
        let dim = (screen_height().min(screen_width()) * 0.86).clamp(300.0, 900.0);
        let x = (screen_width() - dim) * 0.5;
        let y = (screen_height() - dim) * 0.5;
        let scale = dim / map.width as f32;
        let pixel = scale.max(1.0);

        draw_rectangle(
            0.0,
            0.0,
            screen_width(),
            screen_height(),
            Color::from_rgba(0, 0, 0, 150),
        );
        draw_rectangle(
            x - 8.0,
            y - 8.0,
            dim + 16.0,
            dim + 16.0,
            Color::from_rgba(7, 12, 10, 235),
        );
        draw_rectangle(x, y, dim, dim, Color::from_rgba(8, 28, 9, 245));

        for point in &map.radar_points {
            draw_rectangle(
                x + point.x as f32 * scale,
                y + point.y as f32 * scale,
                pixel,
                pixel,
                point.color,
            );
        }
        let viewer_frequency = self.viewer_frequency();
        for object in map
            .objects
            .iter()
            .filter(|object| object.id == GOAL_TILE_ID)
        {
            draw_rectangle(
                x + object.x as f32 * scale,
                y + object.y as f32 * scale,
                pixel,
                pixel,
                radar_goal_tile_color(soccer_team_goal(
                    vec2(object.x as f32, object.y as f32),
                    viewer_frequency as u32,
                    self.tuning.soccer_mode,
                )),
            );
        }

        for prize in &self.prizes {
            self.draw_radar_dot(
                x,
                y,
                scale,
                prize.pos,
                Color::from_rgba(50, 255, 80, 255),
                3.0,
            );
        }
        if self.flags_visible_to_local_viewer() {
            if let Some(flag) = &self.dropped_flag {
                self.draw_radar_dot(
                    x,
                    y,
                    scale,
                    flag.pos,
                    Color::from_rgba(255, 70, 70, 255),
                    5.0,
                );
            }
        }
        if let Some(portal) = self.active_portal() {
            self.draw_radar_dot(
                x,
                y,
                scale,
                portal.pos,
                Color::from_rgba(105, 180, 255, 255),
                3.0,
            );
        }
        for decoy in &self.decoys {
            let (color, size) = radar_player_style(
                false,
                decoy.frequency == viewer_frequency,
                decoy.owner_bounty.min(u16::MAX as u32) as u16,
                0,
                false,
                self.tuning.flagger_on_radar,
            );
            self.draw_radar_dot(x, y, scale, decoy.pos, color, size + 2.0);
        }
        let tuning = self.ship_tuning();
        for bomb in self
            .bombs
            .iter()
            .filter(|bomb| radar_bomb_visible(tuning.see_bomb_level, bomb.level))
        {
            self.draw_radar_dot(
                x,
                y,
                scale,
                bomb.pos,
                Color::from_rgba(245, 115, 65, 255),
                3.0,
            );
        }
        if tuning.see_mines {
            for mine in &self.mines {
                self.draw_radar_dot(
                    x,
                    y,
                    scale,
                    mine.pos,
                    Color::from_rgba(245, 115, 65, 255),
                    3.0,
                );
            }
        }
        if self.tuning.ball_location
            && !self.networked_play
            && self.network_world.powerballs.is_empty()
        {
            self.draw_radar_dot(
                x,
                y,
                scale,
                self.powerball.pos,
                Color::from_rgba(255, 235, 95, 255),
                5.0,
            );
        }
        if self.tuning.ball_location {
            for ball in self.network_world.powerballs.values() {
                let (color, size) =
                    powerball_radar_style(ball.radar_kind(&self.network_world, viewer_frequency));
                self.draw_radar_dot(
                    x,
                    y,
                    scale,
                    ball.display_position(&self.network_world, &self.tuning),
                    color,
                    size + 2.0,
                );
            }
        }
        for bot in self.bots.iter().filter(|bot| bot.respawn_timer <= 0.0) {
            self.draw_radar_dot(
                x,
                y,
                scale,
                bot.pos,
                Color::from_rgba(255, 90, 70, 255),
                5.0,
            );
        }

        let view_w = screen_width() / TILE_SIZE * scale;
        let view_h = screen_height() / TILE_SIZE * scale;
        let ship_x = x + (self.ship.pos.x / TILE_SIZE) * scale;
        let ship_y = y + (self.ship.pos.y / TILE_SIZE) * scale;

        draw_rectangle_lines(
            ship_x - view_w * 0.5,
            ship_y - view_h * 0.5,
            view_w,
            view_h,
            1.0,
            Color::from_rgba(255, 255, 255, 110),
        );
        draw_rectangle(ship_x - 3.0, ship_y - 3.0, 6.0, 6.0, WHITE);
        draw_rectangle_lines(x, y, dim, dim, 1.0, Color::from_rgba(190, 205, 220, 210));
        self.assets.tall_font.draw_scaled(
            &map.name,
            x,
            y - 20.0,
            TextColor::White,
            TextAlign::Left,
            1.5,
        );
    }

    fn draw_radar_dot(&self, x: f32, y: f32, scale: f32, pos: Vec2, color: Color, size: f32) {
        draw_rectangle(
            x + (pos.x / TILE_SIZE) * scale - size * 0.5,
            y + (pos.y / TILE_SIZE) * scale - size * 0.5,
            size,
            size,
            color,
        );
    }
}
