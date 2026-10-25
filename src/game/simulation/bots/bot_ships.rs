use super::*;
use crate::math::canonical_discrete_heading;

impl Game {
    pub(in crate::game) fn update_bots(&mut self, dt: f32) {
        let map = &self.maps[self.map_index];
        let player_pos = self.ship.pos;
        let player_hidden = self.ship.cloak || self.ship.stealth || self.in_safe_zone();
        let antiwarp_active = self.ship.antiwarp && !self.in_safe_zone();
        let mut fired = Vec::new();
        let mut exhaust_spawns = Vec::new();

        for bot in &mut self.bots {
            let bot_radius = self.tuning.ship(bot.ship_type).radius_pixels;
            bot.age += dt;
            bot.fire_cooldown -= dt;
            bot.exhaust_timer -= dt;

            if bot.respawn_timer > 0.0 {
                bot.respawn_timer -= dt;
                if bot.respawn_timer <= 0.0 {
                    let fallback = map.spawn_position();
                    bot.pos = map
                        .random_open_position_near(player_pos, 12, 46, 80)
                        .unwrap_or(fallback);
                    bot.vel = Vec2::ZERO;
                    let max_energy = bot_spawn_energy(self.tuning.ship(bot.ship_type));
                    bot.max_energy = max_energy;
                    bot.energy = max_energy;
                    bot.bounty = 0;
                    bot.carrying_flag = false;
                    bot.super_timer = 0.0;
                    bot.shield_timer = 0.0;
                    bot.engine_shutdown_timer = 0.0;
                    bot.emp_shutdown_timer = 0.0;
                    bot.orientation = bot_spawn_orientation(bot.id);
                    bot.age = 0.0;
                    bot.fire_cooldown = bot_initial_fire_cooldown(
                        self.tuning.ship(bot.ship_type).bullet_cooldown,
                        bot.id,
                    );
                    bot.exhaust_timer = 0.0;
                }
                continue;
            }
            let engine_shutdown = bot.engine_shutdown_timer > 0.0;

            let objective_target = bot_flag_objective_target(
                bot.carrying_flag,
                self.dropped_flag.as_ref().map(|flag| flag.pos),
                map.first_object_position(FLAG_TILE_ID),
                map.first_object_position(GOAL_TILE_ID),
                self.carrying_flag || self.flag_respawn_timer > 0.0,
            );
            let Some(target_pos) = objective_target
                .or_else(|| {
                    bot_prize_target_position(bot.pos, bot.energy, bot.max_energy, &self.prizes)
                })
                .or_else(|| bot_target_position(bot.pos, player_pos, player_hidden, &self.decoys))
            else {
                bot.vel *= 0.992_f32.powf(dt * 60.0);
                move_entity_with_collision(
                    map,
                    &mut bot.pos,
                    &mut bot.vel,
                    bot_radius,
                    dt,
                    self.tuning.bounce_factor,
                );
                continue;
            };

            let to_target = target_pos - bot.pos;
            let distance = to_target.length();
            if distance > 1.0 {
                let desired = heading_to_orientation(to_target / distance);
                bot.orientation = turn_towards_orientation(
                    bot.orientation,
                    desired,
                    bot_rotation_per_second(engine_shutdown),
                    dt,
                );
            }

            let heading = canonical_discrete_heading(bot.orientation);
            if distance > 120.0 && !engine_shutdown {
                bot.vel += heading * BOT_THRUST_ACCELERATION * dt;
                if bot.exhaust_timer <= 0.0 {
                    bot.exhaust_timer = EXHAUST_INTERVAL;
                    exhaust_spawns.push((bot.pos, heading, bot_radius));
                }
            } else {
                bot.vel *= 0.986_f32.powf(dt * 60.0);
            }
            bot.vel = truncate(bot.vel, BOT_TOP_SPEED);
            move_entity_with_collision(
                map,
                &mut bot.pos,
                &mut bot.vel,
                bot_radius,
                dt,
                self.tuning.bounce_factor,
            );
            bot.vel *= 0.992_f32.powf(dt * 60.0);

            let aim = if distance > 1.0 {
                (to_target / distance).dot(heading)
            } else {
                0.0
            };
            let jammed = bot_jammed_by_antiwarp(
                player_pos,
                antiwarp_active,
                bot.pos,
                self.tuning.antiwarp_radius,
            );
            let bot_tuning = self.tuning.ship(bot.ship_type);
            let bullet_level = bot_bullet_level(bot_tuning);
            if bot_can_fire(
                distance,
                aim,
                bot.fire_cooldown,
                jammed,
                engine_shutdown,
                bot_bullet_range(self.tuning, bot.ship_type),
            ) {
                bot.fire_cooldown =
                    bot_fire_cooldown_after_shot(bot_tuning.bullet_cooldown, bot.id);
                fired.push(BotBullet {
                    owner_id: bot.id,
                    pos: bot.pos + heading * 22.0,
                    vel: bot.vel + heading * bot_tuning.bullet_speed,
                    frequency: u16::MAX,
                    ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet),
                    age: 0.0,
                    trail_timer: 0.0,
                    level: bullet_level,
                    damage: bot_bullet_damage(self.tuning, bot.ship_type),
                    bouncing: true,
                });
            }
        }

        let fired_count = fired.len();
        self.bot_bullets.extend(fired);
        if fired_count > 0 {
            self.play_bot_gun_sound();
        }
        for (pos, heading, radius) in exhaust_spawns {
            spawn_exhaust_particles(&mut self.exhausts, pos, heading, false, radius);
        }
        self.apply_player_weapons_to_bots();
    }

    pub(in crate::game) fn play_bot_gun_sound(&self) {
        if let Some(sound) = self.assets.gun_fire[0].as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(BOT_GUN_VOLUME),
                },
            );
        }
    }
}
