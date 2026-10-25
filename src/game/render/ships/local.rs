use super::*;

impl Game {
    pub(in crate::game::render) fn draw_ship(&self, local: &LocalRenderPrediction) {
        if !self.local_ship_pose_presentable() {
            return;
        }

        // Owner-requested local cloak cue. Stealth only hides radar and must
        // leave the ship opaque; cloak also has its separate HUD indicator.
        self.draw_ship_sprite(
            local.position,
            local.orientation,
            self.ship.ship_type,
            Color::new(1.0, 1.0, 1.0, if self.ship.cloak { 0.5 } else { 1.0 }),
        );
        if self.ship.engine_shutdown_timer > 0.0 {
            self.draw_emp_spark_at(local.position, get_time() as f32);
        }
        let label = ship_label(
            self.local_player_name(),
            self.player_bounty,
            self.carrying_flag,
            self.powerball.carried,
        );
        self.draw_camera_relative_ship_label_text(
            vec2(0.0, -34.0),
            &label,
            ship_label_team_color(),
        );
        if let Some((energy, color)) = energy_label(self.visible_hud_energy(), self.ship.max_energy)
        {
            self.draw_camera_relative_ship_label(vec2(0.0, -48.0), &energy, color);
        }
    }

    pub(in crate::game::render) fn draw_spectator_marker(&self) {
        // Following uses the target's own ship/name; the free-camera marker
        // would otherwise sit directly over and obscure that ship.
        if self.spectating_player_id.is_some() {
            return;
        }
        let pos = self.ship.pos;
        let color = Color::from_rgba(120, 220, 255, 190);
        draw_line(pos.x - 13.0, pos.y, pos.x - 4.0, pos.y, 2.0, color);
        draw_line(pos.x + 4.0, pos.y, pos.x + 13.0, pos.y, 2.0, color);
        draw_line(pos.x, pos.y - 13.0, pos.x, pos.y - 4.0, 2.0, color);
        draw_line(pos.x, pos.y + 4.0, pos.x, pos.y + 13.0, 2.0, color);
        self.draw_ship_label(pos + vec2(0.0, -34.0), "Spectator", color);
    }

    pub(in crate::game::render) fn draw_carried_flag(&self, position: Vec2) {
        if !self.carrying_flag {
            return;
        }

        self.draw_flag_sprite(position + vec2(0.0, -20.0), get_time() as f32, true);
    }
}
