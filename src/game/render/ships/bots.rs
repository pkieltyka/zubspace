use super::*;

impl Game {
    pub(in crate::game::render) fn draw_bots(&self) {
        for bot in &self.bots {
            if bot.respawn_timer > 0.0 {
                continue;
            }

            self.draw_ship_sprite(bot.pos, bot.orientation, bot.ship_type, WHITE);
            if bot.engine_shutdown_timer > 0.0 {
                self.draw_emp_spark_at(bot.pos, get_time() as f32);
            }
            if bot.carrying_flag {
                self.draw_flag_sprite(bot.pos + vec2(0.0, -22.0), get_time() as f32, false);
            }

            if crate::browser_metrics::debug_mode() {
                let bar_width = 28.0;
                let x = bot.pos.x - bar_width * 0.5;
                let y = bot.pos.y + self.tuning.ship(bot.ship_type).radius_pixels + 8.0;
                draw_rectangle(x, y, bar_width, 3.0, Color::from_rgba(30, 15, 15, 210));
                draw_rectangle(
                    x,
                    y,
                    bar_width * (bot.energy / bot.max_energy).clamp(0.0, 1.0),
                    3.0,
                    Color::from_rgba(255, 90, 70, 230),
                );
            }
            let label = ship_label(
                &format!("Bot {}", bot.id + 1),
                bot.bounty,
                bot.carrying_flag,
                false,
            );
            let label_color = if bot.carrying_flag {
                ship_label_objective_color()
            } else {
                ship_label_enemy_color()
            };
            self.draw_ship_label_text(bot.pos + vec2(0.0, -34.0), &label, label_color);
            if crate::browser_metrics::debug_mode()
                && let Some((energy, color)) = energy_label(bot.energy, bot.max_energy)
            {
                self.draw_ship_label(bot.pos + vec2(0.0, -48.0), &energy, color);
            }
        }
    }
}
