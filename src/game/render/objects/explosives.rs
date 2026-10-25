use super::*;

impl Game {
    pub(in crate::game::render) fn draw_bombs(&self, partial_seconds: f32) {
        let bomb_gravity = self
            .tuning
            .gravity_bombs
            .then_some(self.ship_tuning().wormhole_gravity);
        for bomb in &self.bombs {
            let presented = local_bomb_presentation(
                bomb,
                partial_seconds,
                self.map(),
                &self.bricks,
                &self.tuning,
                bomb_gravity,
            );
            let frame_start = bomb_sprite_frame_start(
                bomb.level,
                presented.bounces_remaining,
                bomb.emp_shutdown_time > 0.0,
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
    }

    pub(in crate::game::render) fn draw_thors(&self, partial_seconds: f32) {
        for thor in &self.thors {
            let presented = thor_presentation(thor, partial_seconds);
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

    pub(in crate::game::render) fn draw_bomb_flashes(&self) {
        for flash in &self.bomb_flashes {
            draw_texture_ex(
                &self.assets.bomb_flash.texture,
                flash.pos.x - 4.0,
                flash.pos.y - 4.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.bomb_flash.frame_size),
                    source: Some(self.assets.bomb_flash.source_at_with_count(flash.age, 6)),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_bomb_trails(&self) {
        for trail in self
            .bomb_trails
            .iter()
            .chain(self.prepared_remote_bomb_trails.iter())
        {
            draw_texture_ex(
                &self.assets.bomb_trail.texture,
                trail.pos.x - 8.0,
                trail.pos.y - 8.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.bomb_trail.frame_size),
                    source: Some(self.assets.bomb_trail.source_at_with_count(trail.age, 10)),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_mines(&self, partial_seconds: f32) {
        for mine in &self.mines {
            let dt = partial_seconds
                .max(0.0)
                .min(GAMEPLAY_STEP_SECONDS)
                .min(mine.ttl.max(0.0));
            let mut position = mine.pos;
            let mut velocity = mine.vel;
            update_mine_terrain_motion(
                &mut position,
                &mut velocity,
                dt,
                self.map(),
                &self.bricks,
                mine.frequency,
                mine.bomb_gravity,
            );
            let frame_start = mine_sprite_frame_start(mine.level, mine.emp_shutdown_time > 0.0);
            draw_texture_ex(
                &self.assets.mines.texture,
                position.x - 8.0,
                position.y - 8.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.mines.frame_size),
                    source: Some(self.assets.mines.source_loop_range(
                        mine.age + dt,
                        frame_start,
                        MINE_FRAMES_PER_LEVEL,
                    )),
                    ..Default::default()
                },
            );
        }
    }
}
