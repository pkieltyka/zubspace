use super::*;

impl Game {
    pub(in crate::game::render) fn draw_bullets(&self, partial_seconds: f32) {
        for bullet in &self.bullets {
            let presented = local_bullet_presentation(
                bullet,
                partial_seconds,
                self.map(),
                &self.bricks,
                &self.tuning,
            );
            if bullet.activates_on_wall {
                // Burst pieces draw from the bullet sheet's own four-frame loop, not the 80px empburst
                // explosion sheet: using empburst drew every piece as a full explosion circle, so a
                // 24-piece activation read as a pile of burst circles instead of the reference's ring.
                let (frame_start, frame_count) = burst_projectile_sprite_frames(
                    self.assets.bullets.frame_count(),
                    presented.burst_active,
                );
                draw_texture_ex(
                    &self.assets.bullets.texture,
                    presented.position.x - 2.5,
                    presented.position.y - 2.5,
                    WHITE,
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
                continue;
            }

            let source = if bullet.bouncing {
                self.assets.bullets.source_loop_range(
                    presented.animation_seconds,
                    BOUNCING_BULLET_FRAME_START
                        + weapon_level_index(bullet.level) * BULLET_FRAMES_PER_LEVEL,
                    BOUNCING_BULLET_FRAMES,
                )
            } else {
                self.assets.bullets.source_loop_range(
                    presented.animation_seconds,
                    weapon_level_index(bullet.level) * BULLET_FRAMES_PER_LEVEL,
                    BULLET_FRAMES_PER_LEVEL,
                )
            };
            draw_texture_ex(
                &self.assets.bullets.texture,
                presented.position.x - 2.5,
                presented.position.y - 2.5,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.bullets.frame_size),
                    source: Some(source),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_bot_bullets(&self, partial_seconds: f32) {
        for bullet in &self.bot_bullets {
            let presented =
                bot_bullet_presentation(bullet, partial_seconds, self.map(), &self.bricks);
            let source = if bullet.bouncing {
                self.assets.bullets.source_loop_range(
                    presented.animation_seconds,
                    BOUNCING_BULLET_FRAME_START
                        + weapon_level_index(bullet.level) * BULLET_FRAMES_PER_LEVEL,
                    BOUNCING_BULLET_FRAMES,
                )
            } else {
                self.assets.bullets.source_loop_range(
                    presented.animation_seconds,
                    weapon_level_index(bullet.level) * BULLET_FRAMES_PER_LEVEL,
                    BULLET_FRAMES_PER_LEVEL,
                )
            };
            let color = if bullet.bouncing && bullet.level >= 3 {
                Color::from_rgba(120, 190, 255, 255)
            } else {
                Color::from_rgba(255, 105, 80, 255)
            };
            draw_texture_ex(
                &self.assets.bullets.texture,
                presented.position.x - 2.5,
                presented.position.y - 2.5,
                color,
                DrawTextureParams {
                    dest_size: Some(self.assets.bullets.frame_size),
                    source: Some(source),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_bullet_trails(&self) {
        for trail in self
            .bullet_trails
            .iter()
            .chain(self.prepared_remote_bullet_trails.iter())
        {
            let color = if trail.bouncing && trail.level >= 3 {
                Color::from_rgba(100, 170, 255, 255)
            } else {
                WHITE
            };
            draw_texture_ex(
                &self.assets.bullet_trail.texture,
                trail.pos.x - 1.0,
                trail.pos.y - 1.0,
                color,
                DrawTextureParams {
                    dest_size: Some(vec2(2.0, 2.0)),
                    source: Some(self.assets.bullet_trail.source_at_range(
                        trail.age,
                        BULLET_TRAIL_FRAME_START,
                        BULLET_TRAIL_FRAMES,
                    )),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_bullet_impacts(&self) {
        for impact in &self.bullet_impacts {
            draw_texture_ex(
                &self.assets.bullet_impact.texture,
                impact.pos.x - 8.0,
                impact.pos.y - 8.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.bullet_impact.frame_size),
                    source: Some(self.assets.bullet_impact.source_at(impact.age)),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_shrapnel(&self, partial_seconds: f32) {
        for shrap in &self.shrapnel {
            let presented = local_shrapnel_presentation(
                shrap,
                partial_seconds,
                self.map(),
                &self.bricks,
                &self.tuning,
            );
            draw_texture_ex(
                &self.assets.shrapnel.texture,
                presented.position.x - 4.0,
                presented.position.y - 4.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.shrapnel.frame_size),
                    source: Some(self.assets.shrapnel.source_loop_range(
                        presented.animation_seconds,
                        shrapnel_sprite_frame_start(shrap.level, shrap.bouncing),
                        10,
                    )),
                    ..Default::default()
                },
            );
        }
    }
}
