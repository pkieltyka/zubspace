use super::*;

impl Game {
    pub(in crate::game::render) fn draw_prizes(&self) {
        for prize in &self.prizes {
            draw_texture_ex(
                &self.assets.prizes.texture,
                prize.pos.x - 8.0,
                prize.pos.y - 8.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.prizes.frame_size),
                    source: Some(
                        self.assets
                            .prizes
                            .source_for_frame(prize.kind.frame_index()),
                    ),
                    ..Default::default()
                },
            );
        }
    }
}
