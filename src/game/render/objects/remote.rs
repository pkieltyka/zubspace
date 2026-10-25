use super::*;

impl Game {
    pub(in crate::game::render) fn draw_remote_projectiles(&self, partial_seconds: f32) {
        let viewport = vec2(screen_width(), screen_height());
        for projectile in &self.prepared_remote_projectiles {
            let presented = self.remote_projectile_draw_presentation(projectile, partial_seconds);
            let screen = self.camera.world_to_screen(presented.position);
            if !remote_projectile_is_visible(
                screen,
                viewport.x,
                viewport.y,
                REMOTE_PROJECTILE_CULL_MARGIN_PX,
            ) {
                continue;
            }
            match projectile.kind {
                RemoteProjectileKind::Bullet { bouncing } => {
                    let source = if bouncing {
                        self.assets.bullets.source_loop_range(
                            presented.animation_seconds,
                            BOUNCING_BULLET_FRAME_START
                                + weapon_level_index(projectile.level) * BULLET_FRAMES_PER_LEVEL,
                            BOUNCING_BULLET_FRAMES,
                        )
                    } else {
                        self.assets.bullets.source_loop_range(
                            presented.animation_seconds,
                            weapon_level_index(projectile.level) * BULLET_FRAMES_PER_LEVEL,
                            BULLET_FRAMES_PER_LEVEL,
                        )
                    };
                    draw_texture_ex(
                        &self.assets.bullets.texture,
                        presented.position.x - 2.5,
                        presented.position.y - 2.5,
                        Color::from_rgba(205, 225, 255, 235),
                        DrawTextureParams {
                            dest_size: Some(self.assets.bullets.frame_size),
                            source: Some(source),
                            ..Default::default()
                        },
                    );
                }
                RemoteProjectileKind::Burst { .. } => {
                    // Same bullet-sheet burst loop as the local renderer; the empburst sheet is the
                    // separate EMP explosion and made remote burst pieces read as huge circles.
                    let (frame_start, frame_count) = burst_projectile_sprite_frames(
                        self.assets.bullets.frame_count(),
                        presented.burst_active,
                    );
                    draw_texture_ex(
                        &self.assets.bullets.texture,
                        presented.position.x - 2.5,
                        presented.position.y - 2.5,
                        Color::from_rgba(205, 225, 255, 235),
                        DrawTextureParams {
                            dest_size: Some(self.assets.bullets.frame_size),
                            source: Some(self.assets.bullets.source_loop_range_with_duration(
                                presented.animation_seconds,
                                frame_start,
                                frame_count,
                                BURST_PROJECTILE_ANIMATION_DURATION,
                            )),
                            ..Default::default()
                        },
                    );
                }
                RemoteProjectileKind::Shrapnel { bouncing } => {
                    draw_texture_ex(
                        &self.assets.shrapnel.texture,
                        presented.position.x - 4.0,
                        presented.position.y - 4.0,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(self.assets.shrapnel.frame_size),
                            source: Some(self.assets.shrapnel.source_loop_range(
                                presented.animation_seconds,
                                shrapnel_sprite_frame_start(projectile.level, bouncing),
                                10,
                            )),
                            ..Default::default()
                        },
                    );
                }
                RemoteProjectileKind::Bomb { .. } => {
                    let frame_start = bomb_sprite_frame_start(
                        projectile.level,
                        presented.bounces_remaining,
                        projectile.emp_shutdown_time > 0.0,
                    );
                    draw_texture_ex(
                        &self.assets.bombs.texture,
                        presented.position.x - 8.0,
                        presented.position.y - 8.0,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(self.assets.bombs.frame_size),
                            source: Some(self.assets.bombs.source_loop_range(
                                presented.animation_seconds,
                                frame_start,
                                BOMB_FRAMES_PER_LEVEL,
                            )),
                            ..Default::default()
                        },
                    );
                }
                RemoteProjectileKind::Mine { .. } => {
                    let frame_start = mine_sprite_frame_start(
                        projectile.level,
                        projectile.emp_shutdown_time > 0.0,
                    );
                    draw_texture_ex(
                        &self.assets.mines.texture,
                        presented.position.x - 8.0,
                        presented.position.y - 8.0,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(self.assets.mines.frame_size),
                            source: Some(self.assets.mines.source_loop_range(
                                presented.animation_seconds,
                                frame_start,
                                MINE_FRAMES_PER_LEVEL,
                            )),
                            ..Default::default()
                        },
                    );
                }
                RemoteProjectileKind::Thor => {
                    draw_texture_ex(
                        &self.assets.bombs.texture,
                        presented.position.x - 8.0,
                        presented.position.y - 8.0,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(self.assets.bombs.frame_size),
                            source: Some(self.assets.bombs.source_at_range(
                                presented.animation_seconds,
                                THOR_FRAME_START,
                                THOR_FRAMES,
                            )),
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }
}
