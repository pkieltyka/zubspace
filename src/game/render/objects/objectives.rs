use super::*;

impl Game {
    pub(in crate::game::render) fn draw_powerballs(&self) {
        if !self.network_world.powerballs.is_empty() {
            let current_server_tick = self.browser_server_tick();
            for ball in self.network_world.powerballs.values() {
                self.draw_powerball_at(
                    ball.display_position(&self.network_world, &self.tuning),
                    ball.timestamp as f32 / 100.0,
                    ball.objective_phased_passing(
                        self.tuning.pass_delay_seconds,
                        current_server_tick,
                    ),
                );
            }
            return;
        }

        // No network ball — draw the local-only fallback only when soccer is
        // actually enabled for this arena. Non-soccer zones (e.g. chaos) skip it.
        if self.networked_play || self.tuning.soccer_mode == 0 {
            return;
        }

        self.draw_powerball_at(
            self.powerball.pos,
            self.powerball.age,
            self.powerball.pass_cooldown > 0.0 && !self.powerball.carried,
        );
    }

    fn draw_powerball_at(&self, pos: Vec2, age: f32, phased: bool) {
        let frame_start = if phased {
            POWERBALL_PHASED_FRAME_START
        } else {
            0
        };
        draw_texture_ex(
            &self.assets.powerball.texture,
            pos.x - 8.0,
            pos.y - 8.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(self.assets.powerball.frame_size),
                source: Some(self.assets.powerball.source_loop_range(
                    age,
                    frame_start,
                    POWERBALL_FRAMES,
                )),
                ..Default::default()
            },
        );
    }

    pub(in crate::game::render) fn draw_powerball_trails(&self) {
        if self.networked_play {
            return;
        }
        for trail in &self.powerball_trails {
            draw_texture_ex(
                &self.assets.powerball.texture,
                trail.pos.x - 8.0,
                trail.pos.y - 8.0,
                Color::from_rgba(180, 220, 255, 150),
                DrawTextureParams {
                    dest_size: Some(self.assets.powerball.frame_size),
                    source: Some(self.assets.powerball.source_at_range(
                        trail.age,
                        POWERBALL_TRAIL_FRAME_START,
                        POWERBALL_FRAMES,
                    )),
                    ..Default::default()
                },
            );
        }
    }
}
