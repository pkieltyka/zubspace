use super::*;

impl Game {
    pub(in crate::game::render) fn draw_exhausts(&self) {
        let frame_count = self.assets.exhaust.frame_count() / 2;
        for exhaust in &self.exhausts {
            draw_texture_ex(
                &self.assets.exhaust.texture,
                exhaust.pos.x,
                exhaust.pos.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.exhaust.frame_size),
                    source: Some(
                        self.assets
                            .exhaust
                            .source_at_with_count(exhaust.age, frame_count),
                    ),
                    ..Default::default()
                },
            );
        }
    }

    pub(in crate::game::render) fn draw_rocket_particles(&self) {
        for particle in &self.rocket_particles {
            draw_texture_ex(
                &self.assets.rocket.texture,
                particle.pos.x,
                particle.pos.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.rocket.frame_size),
                    source: Some(self.assets.rocket.source_at(particle.age)),
                    ..Default::default()
                },
            );
        }
    }
}
