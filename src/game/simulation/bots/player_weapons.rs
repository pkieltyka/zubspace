use super::*;

impl Game {
    pub(in crate::game) fn apply_player_weapons_to_bots(&mut self) {
        let mut killed = Vec::new();
        let mut dropped_flags = Vec::new();
        for bot in &mut self.bots {
            if bot.respawn_timer > 0.0 {
                continue;
            }
            let bot_radius = self.tuning.ship(bot.ship_type).radius_pixels;

            for bullet in &mut self.bullets {
                if bullet.ttl > 0.0
                    && bullet.active
                    && bullet_contact_overlaps_ship(bullet.pos, bot.pos, bot_radius)
                {
                    bullet.ttl = 0.0;
                    let damage = bot_damage_after_timed_protection(bot, bullet.damage);
                    bot.energy -=
                        bullet_or_burst_damage_after_exactness(self.tuning.exact_damage, damage);
                    self.bullet_impacts.push(BulletImpact {
                        pos: bullet.pos,
                        age: 0.0,
                        remote_source: None,
                    });
                }
            }

            for shrap in &mut self.shrapnel {
                if shrap.ttl > 0.0 && bullet_contact_overlaps_ship(shrap.pos, bot.pos, bot_radius) {
                    shrap.ttl = 0.0;
                    let damage = shrapnel_damage(shrap.age, shrap.inactive_damage, shrap.damage);
                    let damage = bot_damage_after_timed_protection(bot, damage);
                    bot.energy -=
                        bullet_or_burst_damage_after_exactness(self.tuning.exact_damage, damage);
                    self.bullet_impacts.push(BulletImpact {
                        pos: shrap.pos,
                        age: 0.0,
                        remote_source: None,
                    });
                }
            }

            for thor in &mut self.thors {
                if thor.ttl > 0.0 && explosive_contact_overlaps_ship(thor.pos, bot.pos, bot_radius)
                {
                    thor.ttl = 0.0;
                    self.explosions.push(Explosion {
                        prediction: None,
                        pos: thor.pos,
                        damage_pos: thor.pos,
                        age: 0.0,
                        kind: ExplosionKind::Bomb,
                        damages_bots: true,
                        damage: thor.damage,
                        radius: bomb_explosion_radius(
                            self.tuning.bomb_explode_pixels,
                            WEAPON_LEVEL_MAX,
                        ),
                        emp_shutdown_time: 0.0,
                        remote_source: None,
                    });
                }
            }

            for explosion in self.explosions.iter().filter(|explosion| {
                explosion_bot_damage_active(explosion.damages_bots, explosion.age)
            }) {
                let damage = bot_damage_after_timed_protection(
                    bot,
                    explosion_bot_damage(explosion, bot.pos),
                );
                bot.energy -= damage;
                let emp_seconds = explosion_emp_shutdown_seconds(explosion, damage);
                bot.emp_shutdown_timer = bot.emp_shutdown_timer.max(emp_seconds);
            }

            if bot.energy <= 0.0 {
                killed.push((bot.id, bot.pos));
                bot.respawn_timer = BOT_RESPAWN_DELAY;
                bot.vel = Vec2::ZERO;
                bot.energy = 0.0;
                if bot.carrying_flag {
                    dropped_flags.push(bot.pos);
                    bot.carrying_flag = false;
                }
                bot.super_timer = 0.0;
                bot.shield_timer = 0.0;
                bot.engine_shutdown_timer = 0.0;
                bot.emp_shutdown_timer = 0.0;
                bot.deaths += 1;
            }
        }

        for explosion in &mut self.explosions {
            if explosion_bot_damage_active(explosion.damages_bots, explosion.age) {
                explosion.damages_bots = false;
            }
        }

        for pos in dropped_flags {
            self.drop_flag_at(pos);
        }

        if killed.is_empty() {
            return;
        }

        for (id, pos) in killed {
            self.network_world.lvz.trigger_kill();
            self.bot_score += 1;
            let defeated_bounty = self
                .bots
                .iter_mut()
                .find(|bot| bot.id == id)
                .map(|bot| {
                    let bounty = bot.bounty;
                    bot.bounty = 0;
                    bounty
                })
                .unwrap_or(0);
            if defeated_bounty > 0 {
                self.player_bounty = self
                    .player_bounty
                    .saturating_add(self.tuning.bounty_increase_for_kill);
            }
            self.add_ship_explosion(pos);
            self.spawn_prize_at(pos, CONTINUUM_MAX_GREEN_COUNT);
            self.push_notification(
                &bot_destroyed_notification(id, defeated_bounty, self.local_player_name()),
                Color::from_rgba(255, 210, 100, 255),
            );
        }
    }
}
