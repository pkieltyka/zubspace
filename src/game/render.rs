use super::*;

#[cfg(target_arch = "wasm32")]
mod client_debug;
mod debug;
mod effects;
mod hud;
mod lvz;
mod objects;
mod radar;
mod ships;

impl Game {
    pub(crate) fn draw(&mut self, partial_seconds: f32, render_input: &RenderInputContext) {
        let local = self.local_render_prediction(partial_seconds, render_input);
        // Labels temporarily select screen space and restore this camera.
        // Give every draw the same render-only target, recomputed absolutely
        // each frame so zero-step frames never accumulate prediction offsets.
        self.camera.target = local.position;
        #[cfg(target_arch = "wasm32")]
        if crate::browser_metrics::client_debug_frame_requested() {
            self.capture_client_debug_frame(&local, partial_seconds);
        }
        clear_background(Color::from_rgba(5, 5, 8, 255));
        set_camera(&self.camera);

        self.draw_lvz_map_objects_for_layers(LVZ_LAYER_BELOW_ALL, LVZ_LAYER_BELOW_ALL);
        self.draw_starfield();
        self.draw_lvz_map_objects_for_layers(
            LVZ_LAYER_AFTER_BACKGROUND,
            LVZ_LAYER_AFTER_BACKGROUND,
        );
        self.draw_map();
        self.draw_doors();
        self.draw_map_objects();
        self.draw_lvz_map_objects_for_layers(LVZ_LAYER_AFTER_TILES, LVZ_LAYER_AFTER_TILES);
        self.draw_hidden_solid_tile_debug_overlay();
        self.draw_dropped_flag();
        self.draw_network_flags();
        self.draw_prizes();
        self.draw_powerball_trails();
        self.draw_powerballs();
        self.draw_exhausts();
        self.draw_rocket_particles();
        self.draw_bullet_trails();
        let projectile_partial = if self.gameplay_step_ready() {
            partial_seconds
        } else {
            0.0
        };
        #[cfg(target_arch = "wasm32")]
        if crate::browser_metrics::client_debug_muzzle_requested() {
            self.capture_client_debug_muzzle(projectile_partial);
        }
        self.draw_bullets(projectile_partial);
        self.draw_bullet_impacts();
        self.draw_shrapnel(projectile_partial);
        self.draw_bomb_flashes();
        self.draw_bomb_trails();
        self.draw_bombs(projectile_partial);
        self.draw_thors(projectile_partial);
        self.draw_bot_bullets(projectile_partial);
        self.draw_mines(projectile_partial);
        self.draw_remote_projectiles(projectile_partial);
        self.draw_bricks();
        self.draw_lvz_map_objects_for_layers(LVZ_LAYER_AFTER_WEAPONS, LVZ_LAYER_AFTER_WEAPONS);
        self.draw_decoys(projectile_partial);
        self.draw_bots();
        self.draw_network_players(projectile_partial);
        self.draw_explosions();
        self.draw_repels();
        self.draw_portal();
        self.draw_warps();
        self.draw_burst_effects();
        if self.spectator_mode {
            self.draw_spectator_marker();
        } else {
            self.draw_ship(&local);
        }

        self.draw_carried_flag(local.position);
        self.draw_lvz_map_objects_for_layers(LVZ_LAYER_AFTER_SHIPS, LVZ_LAYER_TOPMOST);

        set_default_camera();
        self.draw_lvz_screen_objects_for_layers(LVZ_LAYER_BELOW_ALL, LVZ_LAYER_AFTER_SHIPS);
        self.draw_lvz_screen_objects_for_layers(LVZ_LAYER_AFTER_GAUGES, LVZ_LAYER_AFTER_GAUGES);
        self.draw_hud(&local);
        self.draw_lvz_screen_objects_for_layers(LVZ_LAYER_AFTER_CHAT, LVZ_LAYER_TOPMOST);
    }

    pub(in crate::game) fn draw_flag_sprite_at(&self, pos: Vec2, time: f32, team: bool) {
        let (texture, frame_size, source) = self.flag_sprite_params(time, team);
        draw_texture_ex(
            texture,
            pos.x - frame_size.x * 0.5,
            pos.y - frame_size.y * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(frame_size),
                source: Some(source),
                ..Default::default()
            },
        );
    }

    fn flag_sprite_params(&self, time: f32, team: bool) -> (&Texture2D, Vec2, Rect) {
        let frame_start = if team { FLAG_TEAM_FRAME_START } else { 0 };
        if let Some(texture) = self.active_lvz_flag_texture() {
            let frame_size = lvz_square_sheet_frame_size(texture.width, FLAG_FRAMES_PER_TEAM);
            let frame = lvz_animation_frame(time, 100, FLAG_FRAMES_PER_TEAM) + frame_start;
            return (
                &texture.texture,
                frame_size,
                lvz_source_for_frame(frame_size, frame, FLAG_FRAMES_PER_TEAM),
            );
        }

        (
            &self.assets.flag.texture,
            self.assets.flag.frame_size,
            self.assets
                .flag
                .source_loop_range(time, frame_start, FLAG_FRAMES_PER_TEAM),
        )
    }

    fn active_lvz_flag_texture(&self) -> Option<&LvzTexture> {
        self.active_lvz_texture_by_stem("flag")
    }

    pub(in crate::game) fn active_lvz_texture_by_stem(&self, stem: &str) -> Option<&LvzTexture> {
        let normalized_stem = stem.to_ascii_lowercase();
        self.lvz_render_cache
            .graphic_overrides
            .get(normalized_stem.as_str())
            .and_then(|name| self.lvz_render_cache.textures.get(name))
    }

    fn current_tile_texture(&self) -> &Texture2D {
        self.map_tilesets
            .get(self.map_index)
            .and_then(|texture| texture.as_ref())
            .unwrap_or(&self.assets.tiles)
    }

    fn draw_starfield(&self) {
        let half_screen = vec2(screen_width(), screen_height()) * 0.55;
        let visible_min = self.camera.target - half_screen;
        let visible_max = self.camera.target + half_screen;
        let time = get_time() as f32;

        for layer in 0..STAR_LAYER_COUNT {
            let cell_size = STAR_LAYER_CELLS[layer];
            let parallax = self.camera.target * (1.0 - STAR_LAYER_SPEEDS[layer]);
            let min = visible_min - parallax - vec2(cell_size, cell_size);
            let max = visible_max - parallax + vec2(cell_size, cell_size);
            let min_x = (min.x / cell_size).floor() as i32;
            let min_y = (min.y / cell_size).floor() as i32;
            let max_x = (max.x / cell_size).ceil() as i32;
            let max_y = (max.y / cell_size).ceil() as i32;

            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let hash = hash_grid(x, y, layer as u32);
                    let offset = vec2(
                        (hash & 0xff) as f32 / 255.0 * cell_size,
                        ((hash >> 8) & 0xff) as f32 / 255.0 * cell_size,
                    );
                    let pos = vec2(x as f32 * cell_size, y as f32 * cell_size) + offset + parallax;
                    let texture =
                        &self.assets.stars[(hash as usize >> 16) % self.assets.stars.len()];
                    let twinkle = (((time * 2.0 + (hash >> 24) as f32).sin() + 1.0) * 18.0) as u8;
                    let alpha = STAR_LAYER_ALPHA[layer].saturating_add(twinkle);

                    draw_texture_ex(
                        texture,
                        pos.x - texture.width() * 0.5,
                        pos.y - texture.height() * 0.5,
                        Color::from_rgba(210, 218, 235, alpha),
                        DrawTextureParams::default(),
                    );
                }
            }
        }
    }

    fn draw_map(&self) {
        let map = self.map();
        let tile_texture = self.current_tile_texture();
        let half_screen = vec2(screen_width(), screen_height()) * 0.5;
        let min = self.camera.target - half_screen - vec2(TILE_SIZE * 2.0, TILE_SIZE * 2.0);
        let max = self.camera.target + half_screen + vec2(TILE_SIZE * 2.0, TILE_SIZE * 2.0);
        let min_x = (min.x / TILE_SIZE).floor().max(0.0) as i32;
        let min_y = (min.y / TILE_SIZE).floor().max(0.0) as i32;
        let max_x = (max.x / TILE_SIZE).ceil().min(map.width as f32) as i32;
        let max_y = (max.y / TILE_SIZE).ceil().min(map.height as f32) as i32;

        for y in min_y..max_y {
            for x in min_x..max_x {
                let tile = map.tile(x, y);
                let Some(tile) = map.render_tile(tile) else {
                    continue;
                };

                let source = tile_source(tile);
                draw_texture_ex(
                    tile_texture,
                    x as f32 * TILE_SIZE,
                    y as f32 * TILE_SIZE,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(TILE_SIZE, TILE_SIZE)),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            }
        }
    }

    fn draw_doors(&self) {
        let map = self.map();
        if map.doors.is_empty() {
            return;
        }
        let tile_texture = self.current_tile_texture();

        let half_screen = vec2(screen_width(), screen_height()) * 0.5;
        let min = self.camera.target - half_screen - vec2(TILE_SIZE * 2.0, TILE_SIZE * 2.0);
        let max = self.camera.target + half_screen + vec2(TILE_SIZE * 2.0, TILE_SIZE * 2.0);
        let time = get_time() as f32;

        for door in &map.doors {
            let current = map.tile(door.x as i32, door.y as i32);
            if seeded_door_tile(current, map.door_seed) != Some(current) {
                continue;
            }

            let pos = vec2(door.x as f32 * TILE_SIZE, door.y as f32 * TILE_SIZE);
            if pos.x > max.x
                || pos.y > max.y
                || pos.x + TILE_SIZE < min.x
                || pos.y + TILE_SIZE < min.y
            {
                continue;
            }

            draw_texture_ex(
                tile_texture,
                pos.x,
                pos.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(TILE_SIZE, TILE_SIZE)),
                    source: Some(door_animation_source(door.id, time)),
                    ..Default::default()
                },
            );
        }
    }

    fn draw_map_objects(&self) {
        let map = self.map();
        let time = get_time() as f32;
        let half_screen = vec2(screen_width(), screen_height()) * 0.5;
        let min = self.camera.target - half_screen - vec2(128.0, 128.0);
        let max = self.camera.target + half_screen + vec2(128.0, 128.0);

        for object in &map.objects {
            let pos = vec2(object.x as f32 * TILE_SIZE, object.y as f32 * TILE_SIZE);
            if pos.x > max.x || pos.y > max.y || pos.x + 96.0 < min.x || pos.y + 96.0 < min.y {
                continue;
            }

            let Some(sheet) = self.assets.sheet_for_tile(object.id) else {
                continue;
            };
            if self.networked_play && object.id == FLAG_TILE_ID {
                continue;
            }
            if object.id == FLAG_TILE_ID
                && flag_source_hidden(
                    self.carrying_flag,
                    self.flag_respawn_timer,
                    self.dropped_flag.is_some() || self.bots.iter().any(|bot| bot.carrying_flag),
                )
            {
                continue;
            }
            let source = match object.id {
                FLAG_TILE_ID => sheet.source_loop_range(time, 0, FLAG_FRAMES_PER_TEAM),
                GOAL_TILE_ID => {
                    let frame_start = if soccer_team_goal(
                        vec2(object.x as f32, object.y as f32),
                        self.viewer_frequency() as u32,
                        self.tuning.soccer_mode,
                    ) {
                        0
                    } else {
                        GOAL_SCORING_FRAME_START
                    };
                    sheet.source_loop_range(time, frame_start, GOAL_FRAMES_PER_TEAM)
                }
                _ => sheet.source_at(time),
            };
            draw_texture_ex(
                &sheet.texture,
                pos.x,
                pos.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(sheet.frame_size),
                    source: Some(source),
                    ..Default::default()
                },
            );
        }
    }

    fn draw_dropped_flag(&self) {
        if self.networked_play || !self.flags_visible_to_local_viewer() {
            return;
        }

        let Some(flag) = &self.dropped_flag else {
            return;
        };

        self.draw_flag_sprite_at(flag.pos, flag.age, false);
    }

    fn draw_network_flags(&self) {
        if !self.flags_visible_to_local_viewer() {
            return;
        }

        for flag in self
            .network_world
            .flags
            .values()
            .filter(|flag| flag.dropped && flag.carried_by.is_none())
            .filter(|flag| flag.position != Vec2::ZERO)
        {
            let frame_start = if flag.owner == self.viewer_frequency() {
                FLAG_TEAM_FRAME_START
            } else {
                0
            };
            self.draw_flag_sprite_at(flag.position, get_time() as f32, frame_start != 0);
        }
    }
}

/// Off-screen slack, in screen pixels, for a culled remote projectile.
///
/// Every remote projectile sprite is centred on its position, so the slack only
/// has to cover the largest animated frame at the deepest camera zoom: the
/// 80-pixel EMP burst frame at the 2x gameplay zoom needs 80, and the rest of
/// the kinds are far smaller. Anything past that is not on the monitor, and its
/// textured quad is work the compositor never displays.
pub(crate) const REMOTE_PROJECTILE_CULL_MARGIN_PX: f32 = 96.0;

/// Whether a projectile drawn at `screen` occupies any visible pixel.
///
/// The decision is taken in screen space because `Camera2D::world_to_screen`
/// already folds in the target and the per-viewport zoom, which is what the
/// sprite is scaled by. The draw path and the render trace share this one
/// predicate so retained evidence can never claim a draw the cull suppressed.
pub(crate) fn remote_projectile_is_visible(
    screen: Vec2,
    viewport_width: f32,
    viewport_height: f32,
    margin: f32,
) -> bool {
    screen.x > -margin
        && screen.x < viewport_width + margin
        && screen.y > -margin
        && screen.y < viewport_height + margin
}
#[cfg(test)]
mod remote_projectile_cull_tests {
    use super::*;

    #[test]
    fn remote_projectile_cull_keeps_partially_visible_sprites() {
        let margin = REMOTE_PROJECTILE_CULL_MARGIN_PX;
        for position in [
            vec2(720.0, 500.0),
            vec2(0.0, 0.0),
            vec2(1_440.0, 1_000.0),
            vec2(-margin + 1.0, 500.0),
            vec2(1_440.0 + margin - 1.0, 500.0),
            vec2(500.0, -margin + 1.0),
            vec2(500.0, 1_000.0 + margin - 1.0),
        ] {
            assert!(
                remote_projectile_is_visible(position, 1_440.0, 1_000.0, margin),
                "screen point {position:?} still paints visible pixels and must be drawn",
            );
        }
    }

    #[test]
    fn remote_projectile_cull_drops_only_fully_offscreen_sprites() {
        let margin = REMOTE_PROJECTILE_CULL_MARGIN_PX;
        for position in [
            vec2(-margin, 500.0),
            vec2(1_440.0 + margin, 500.0),
            vec2(500.0, -margin),
            vec2(500.0, 1_000.0 + margin),
            vec2(-10_000.0, 10_000.0),
        ] {
            assert!(
                !remote_projectile_is_visible(position, 1_440.0, 1_000.0, margin),
                "screen point {position:?} is past the sprite slack and must be culled",
            );
        }
    }
}
