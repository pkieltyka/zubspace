use super::*;

impl Game {
    pub(in crate::game::render) fn draw_radar(&self) {
        let map = self.map();
        let view = small_radar_view(
            map.width,
            map.height,
            self.ship.pos,
            screen_width(),
            screen_height(),
            self.tuning.map_zoom_factor,
        );
        let dim = view.dim_px;
        let origin = small_radar_origin(screen_width(), screen_height(), dim);
        let x = origin.x;
        let y = origin.y;
        let pixel = vec2(dim / view.tile_span.x, dim / view.tile_span.y).max(Vec2::ONE);

        self.draw_radar_frame(x, y, dim);
        draw_rectangle(x, y, dim, dim, Color::from_rgba(10, 25, 10, 235));
        map.radar_point_index()
            .visit_view_candidates(&map.radar_points, view, |point| {
                if !view.contains_tile(point.x, point.y) {
                    return;
                }
                let pos = view.project_tile(point.x as f32, point.y as f32, x, y);
                draw_rectangle(pos.x, pos.y, pixel.x, pixel.y, point.color);
            });
        let viewer_frequency = self.viewer_frequency();
        for object in map
            .objects
            .iter()
            .filter(|object| object.id == GOAL_TILE_ID)
        {
            if !view.contains_tile(object.x, object.y) {
                continue;
            }
            let pos = view.project_tile(object.x as f32, object.y as f32, x, y);
            draw_rectangle(
                pos.x,
                pos.y,
                pixel.x,
                pixel.y,
                radar_goal_tile_color(soccer_team_goal(
                    vec2(object.x as f32, object.y as f32),
                    viewer_frequency as u32,
                    self.tuning.soccer_mode,
                )),
            );
        }

        for prize in &self.prizes {
            draw_radar_view_dot(
                &view,
                x,
                y,
                prize.pos,
                Color::from_rgba(50, 255, 80, 255),
                2.0,
            );
        }
        if self.flags_visible_to_local_viewer() {
            if let Some(flag) = &self.dropped_flag {
                draw_radar_view_dot(
                    &view,
                    x,
                    y,
                    flag.pos,
                    Color::from_rgba(255, 70, 70, 255),
                    3.0,
                );
            }
        }
        if let Some(portal) = self.active_portal() {
            draw_radar_view_dot(
                &view,
                x,
                y,
                portal.pos,
                Color::from_rgba(105, 180, 255, 255),
                2.0,
            );
        }
        self.decoy_radar_presentations.set(0);
        for decoy in &self.decoys {
            let (color, size) = radar_player_style(
                false,
                decoy.frequency == viewer_frequency,
                decoy.owner_bounty.min(u16::MAX as u32) as u16,
                0,
                false,
                self.tuning.flagger_on_radar,
            );
            draw_radar_view_dot(&view, x, y, decoy.pos, color, size);
            let tile = decoy.pos / TILE_SIZE;
            if view.contains_tile(tile.x as usize, tile.y as usize) {
                self.decoy_radar_presentations
                    .set(self.decoy_radar_presentations.get() + 1);
            }
        }
        let tuning = self.ship_tuning();
        for bomb in self
            .bombs
            .iter()
            .filter(|bomb| radar_bomb_visible(tuning.see_bomb_level, bomb.level))
        {
            draw_radar_view_dot(
                &view,
                x,
                y,
                bomb.pos,
                Color::from_rgba(245, 115, 65, 255),
                2.0,
            );
        }
        if tuning.see_mines {
            for mine in &self.mines {
                draw_radar_view_dot(
                    &view,
                    x,
                    y,
                    mine.pos,
                    Color::from_rgba(245, 115, 65, 255),
                    2.0,
                );
            }
        }
        if self.tuning.ball_location
            && self.tuning.soccer_mode != 0
            && !self.networked_play
            && self.network_world.powerballs.is_empty()
        {
            draw_radar_view_dot(
                &view,
                x,
                y,
                self.powerball.pos,
                Color::from_rgba(255, 235, 95, 255),
                3.0,
            );
        }
        if self.tuning.ball_location {
            for ball in self.network_world.powerballs.values() {
                let (color, size) =
                    powerball_radar_style(ball.radar_kind(&self.network_world, viewer_frequency));
                draw_radar_view_dot(
                    &view,
                    x,
                    y,
                    ball.display_position(&self.network_world, &self.tuning),
                    color,
                    size,
                );
            }
        }
        for bot in self.bots.iter().filter(|bot| bot.respawn_timer <= 0.0) {
            draw_radar_view_dot(
                &view,
                x,
                y,
                bot.pos,
                Color::from_rgba(255, 90, 70, 255),
                3.0,
            );
        }
        let attached_children = self.network_world.attached_children_by_parent();
        for player in self.network_world.visible_players() {
            let player_visible = network_player_radar_visible(
                player.togglables,
                player.frequency,
                self.viewer_frequency(),
                self.ship.xradar,
            ) || attached_children.get(&player.player_id).is_some_and(
                |children| {
                    children
                        .iter()
                        .copied()
                        .any(network_child_reveals_parent_on_radar)
                },
            );
            if !player_visible {
                continue;
            }

            if let Some(pos) = player.display_position() {
                let carrying_ball = self
                    .network_world
                    .powerballs
                    .values()
                    .any(|ball| ball.carried && ball.owner_id == Some(player.player_id));
                let (color, size) = radar_player_style(
                    false,
                    player.frequency == viewer_frequency,
                    player.bounty,
                    player.flags,
                    carrying_ball,
                    self.tuning.flagger_on_radar,
                );
                draw_radar_view_dot(&view, x, y, pos, color, size);
            }
        }

        let (self_color, self_size) = radar_player_style(
            true,
            true,
            self.player_bounty.min(u16::MAX as u32) as u16,
            u16::from(self.carrying_flag),
            self.powerball.carried,
            self.tuning.flagger_on_radar,
        );
        let ship = view.project_world(self.ship.pos, x, y);
        draw_rectangle(
            ship.x - self_size * 0.5,
            ship.y - self_size * 0.5,
            self_size,
            self_size,
            self_color,
        );
        draw_rectangle_lines(x, y, dim, dim, 1.0, Color::from_rgba(165, 175, 190, 180));
        self.draw_radar_time_sector_label(y);
    }

    fn draw_radar_time_sector_label(&self, radar_y: f32) {
        let label = radar_time_sector_label(local_minutes_since_midnight(), self.ship.pos);
        self.assets.tall_font.draw_scaled(
            &label,
            screen_width() - 5.0,
            radar_y - 16.0,
            TextColor::White,
            TextAlign::Right,
            1.0,
        );
    }

    fn draw_radar_frame(&self, x: f32, y: f32, dim: f32) {
        let h = &self.assets.radar_h;
        let v = &self.assets.radar_v;
        let h_step = h.width().max(1.0);
        let v_step = v.height().max(1.0);
        let mut dx = 0.0;
        while dx < dim {
            let width = h_step.min(dim - dx);
            let source = Rect::new(0.0, 0.0, width, h.height());
            for fy in [y - h.height(), y + dim] {
                draw_texture_ex(
                    h,
                    x + dx,
                    fy,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(width, h.height())),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            }
            dx += h_step;
        }

        let mut dy = 0.0;
        while dy < dim {
            let height = v_step.min(dim - dy);
            let source = Rect::new(0.0, 0.0, v.width(), height);
            for fx in [x - v.width(), x + dim] {
                draw_texture_ex(
                    v,
                    fx,
                    y + dy,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(v.width(), height)),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            }
            dy += v_step;
        }
    }
}
