use super::*;

impl Game {
    pub(super) fn draw_bricks(&self) {
        let frame_count = (self.assets.bricks.frame_count() / 2).max(1);
        let viewer_frequency = self.viewer_frequency();
        for brick in &self.bricks {
            let pos = vec2(
                brick.tile.x as f32 * TILE_SIZE,
                brick.tile.y as f32 * TILE_SIZE,
            );
            let frame_start = if brick.team == viewer_frequency {
                frame_count
            } else {
                0
            };
            draw_texture_ex(
                &self.assets.bricks.texture,
                pos.x,
                pos.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.bricks.frame_size),
                    source: Some(self.assets.bricks.source_loop_range(
                        brick.age,
                        frame_start,
                        frame_count,
                    )),
                    ..Default::default()
                },
            );
        }
    }

    pub(super) fn draw_decoys(&self, partial_seconds: f32) {
        self.decoy_ordinary_ship_presentations.set(0);
        self.decoy_name_label_presentations.set(0);
        for decoy in &self.decoys {
            let pos = decoy_presentation_position(decoy, partial_seconds, self.map(), &self.bricks);
            self.draw_ship_sprite(pos, decoy.orientation, decoy.ship_type, WHITE);
            self.decoy_ordinary_ship_presentations
                .set(self.decoy_ordinary_ship_presentations.get() + 1);
            // The reference game labels other players' decoys, but never our own.
            // The sprite and radar indicator remain visible on both paths.
            if !self.decoy_name_label_visible(decoy) {
                continue;
            }
            let label = ship_label(&decoy.owner_name, decoy.owner_bounty, false, false);
            self.draw_ship_label_text(
                pos + vec2(0.0, -34.0),
                &label,
                super::ships::ship_label_color_for_frequency(
                    decoy.frequency,
                    self.viewer_frequency(),
                    false,
                ),
            );
            self.decoy_name_label_presentations
                .set(self.decoy_name_label_presentations.get() + 1);
        }
    }

    pub(in crate::game) fn decoy_name_label_visible(&self, decoy: &Decoy) -> bool {
        let Some(viewer_id) = self.client_authority.view().scope.player_id else {
            return false;
        };
        decoy
            .remote_source
            .is_some_and(|source| source.identity.player_id() != viewer_id)
    }

    pub(super) fn draw_explosions(&self) {
        for explosion in &self.ship_explosions {
            draw_texture_ex(
                &self.assets.ship_explosion.texture,
                explosion.pos.x - 24.0,
                explosion.pos.y - 24.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.ship_explosion.frame_size),
                    source: Some(self.assets.ship_explosion.source_at(explosion.age)),
                    ..Default::default()
                },
            );
        }

        for explosion in &self.explosions {
            let (sheet, half_size) = match explosion.kind {
                ExplosionKind::Bomb => (&self.assets.bomb_explosion, 40.0),
                ExplosionKind::EmpBomb => (&self.assets.emp_burst, 40.0),
            };
            draw_texture_ex(
                &sheet.texture,
                explosion.pos.x - half_size,
                explosion.pos.y - half_size,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(sheet.frame_size),
                    source: Some(sheet.source_at(explosion.age)),
                    ..Default::default()
                },
            );
        }
    }

    pub(super) fn draw_repels(&self) {
        for repel in &self.repels {
            draw_texture_ex(
                &self.assets.repel.texture,
                repel.pos.x - 48.0,
                repel.pos.y - 48.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.repel.frame_size),
                    source: Some(self.assets.repel.source_at(repel.age)),
                    ..Default::default()
                },
            );
        }
    }

    pub(super) fn draw_burst_effects(&self) {
        for effect in &self.burst_effects {
            draw_texture_ex(
                &self.assets.emp_burst.texture,
                effect.pos.x - 40.0,
                effect.pos.y - 40.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.emp_burst.frame_size),
                    source: Some(self.assets.emp_burst.source_at(effect.age)),
                    ..Default::default()
                },
            );
        }
    }

    pub(super) fn draw_portal(&self) {
        let Some(portal) = self.active_portal() else {
            return;
        };

        draw_texture_ex(
            &self.assets.portal.texture,
            portal.pos.x - 8.0,
            portal.pos.y - 8.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(self.assets.portal.frame_size),
                source: Some(self.assets.portal.source_at(portal.age)),
                ..Default::default()
            },
        );
    }

    pub(super) fn draw_warps(&self) {
        for warp in &self.warps {
            draw_texture_ex(
                &self.assets.warp.texture,
                warp.pos.x - 24.0,
                warp.pos.y - 24.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(self.assets.warp.frame_size),
                    source: Some(self.assets.warp.source_at(warp.age)),
                    ..Default::default()
                },
            );
        }
    }
}
