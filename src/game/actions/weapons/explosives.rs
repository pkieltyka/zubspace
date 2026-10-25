use super::*;

impl Game {
    pub(in crate::game) fn fire_bomb(&mut self, heading: Vec2) {
        let tuning = self.ship_tuning();
        let level = self.effective_bomb_level();
        if level == 0 {
            return;
        }
        self.reveal_cloak_after_weapon_use();
        let weapon_data = self.bomb_weapon_data(level, false);
        self.queue_outbound_weapon(weapon_data);
        self.ship.energy -= self.effective_bomb_energy_cost(tuning).charged;
        let bomb_cooldown = self.effective_bomb_cooldown(tuning.bomb_cooldown);
        (self.ship.bullet_cooldown, self.ship.bomb_cooldown) =
            bomb_cooldowns_after_fire(self.ship.bullet_cooldown, bomb_cooldown, tuning.emp_bomb);
        if !tuning.emp_bomb {
            self.ship.repel_cooldown = repel_cooldown_after_weapon(self.ship.repel_cooldown);
        }
        self.ship.vel = bomb_recoil_velocity(self.ship.vel, heading, tuning.bomb_recoil_speed);
        let projectile = self
            .initialize_local_projectiles(weapon_data, self.ship.pos, self.ship.vel, tuning)
            .into_iter()
            .next()
            .expect("bomb weapon initializes one projectile");
        let bomb_pos = vec2(projectile.position.x, projectile.position.y);
        let damage = bomb_damage_for_ship(
            self.tuning.bomb_damage,
            self.tuning.bouncing_bomb_damage_multiplier,
            self.tuning.emp_bomb_damage_multiplier,
            tuning.bomb_bounces,
            tuning.emp_bomb,
        );
        let frequency = self.player_frequency();
        let emp_shutdown_time =
            emp_bomb_shutdown_time(tuning.emp_bomb, self.tuning.emp_bomb_shutdown_seconds);
        let bomb_vel = vec2(projectile.velocity.x, projectile.velocity.y);
        let shrapnel =
            self.current_explosive_shrapnel(tuning, bomb_pos, bomb_vel, level, frequency);
        self.bombs.push(Bomb {
            prediction: self.local_weapon_prediction(0),
            pos: bomb_pos,
            vel: bomb_vel,
            frequency,
            ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb),
            age: 0.0,
            trail_timer: 0.0,
            bounces_remaining: projectile.bounces_remaining,
            level: projectile.level,
            damage,
            emp_shutdown_time,
            shrapnel,
            proximity: self.ship.proximity_bombs,
            proximity_sensor: None,
        });
        self.bomb_flashes.push(BombFlash {
            pos: bomb_flash_position(self.ship.pos, heading, tuning.radius_pixels),
            age: 0.0,
            remote_source: None,
        });

        let fire_sound = if tuning.emp_bomb {
            self.assets.emp_bomb_fire[weapon_level_index(level)].as_ref()
        } else {
            self.assets.bomb_fire[weapon_level_index(level)].as_ref()
        };
        if let Some(sound) = fire_sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.36),
                },
            );
        }
    }

    pub(in crate::game) fn drop_mine(&mut self, _heading: Vec2) {
        let tuning = self.ship_tuning();
        let drop_pos = self.mine_drop_position();
        if !self.map().can_fit_world(drop_pos, 8.0) || !self.mine_drop_allowed_at(drop_pos, tuning)
        {
            return;
        }

        let level = self.effective_bomb_level();
        if level == 0 {
            return;
        }
        self.reveal_cloak_after_weapon_use();
        let weapon_data = self.bomb_weapon_data(level, true);
        self.queue_outbound_weapon(weapon_data);
        self.ship.energy -= self.effective_mine_energy_cost(tuning).charged;
        let bomb_cooldown = self.effective_bomb_cooldown(tuning.bomb_cooldown);
        (self.ship.bullet_cooldown, self.ship.bomb_cooldown) =
            bomb_cooldowns_after_fire(self.ship.bullet_cooldown, bomb_cooldown, tuning.emp_bomb);
        if !tuning.emp_bomb {
            self.ship.repel_cooldown = repel_cooldown_after_weapon(self.ship.repel_cooldown);
        }
        let damage = bomb_damage_for_ship(
            self.tuning.bomb_damage,
            self.tuning.bouncing_bomb_damage_multiplier,
            self.tuning.emp_bomb_damage_multiplier,
            tuning.bomb_bounces,
            tuning.emp_bomb,
        );
        let emp_shutdown_time =
            emp_bomb_shutdown_time(tuning.emp_bomb, self.tuning.emp_bomb_shutdown_seconds);
        let frequency = self.player_frequency();
        let projectile = self
            .initialize_local_projectiles(weapon_data, drop_pos, self.ship.vel, tuning)
            .into_iter()
            .next()
            .expect("mine weapon initializes one projectile");
        let mine_pos = vec2(projectile.position.x, projectile.position.y);
        let mine_vel = vec2(projectile.velocity.x, projectile.velocity.y);
        let shrapnel = self.current_explosive_shrapnel(
            tuning,
            mine_pos,
            mine_vel,
            projectile.level,
            frequency,
        );
        self.mines.push(Mine {
            vel: mine_vel,
            bomb_gravity: self.tuning.gravity_bombs.then_some(tuning.wormhole_gravity),
            prediction: self.local_weapon_prediction(0),
            pos: mine_pos,
            frequency,
            ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Mine),
            level: projectile.level,
            damage,
            emp_shutdown_time,
            shrapnel,
            proximity: self.ship.proximity_bombs,
            proximity_sensor: None,
            age: 0.0,
        });

        let fire_sound = if tuning.emp_bomb {
            self.assets.emp_bomb_fire[weapon_level_index(level)].as_ref()
        } else {
            self.assets.mine_fire[weapon_level_index(level)].as_ref()
        };
        if let Some(sound) = fire_sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.34),
                },
            );
        }
    }

    pub(in crate::game) fn mine_drop_allowed_for_heading(
        &self,
        _heading: Vec2,
        tuning: ShipTuning,
    ) -> bool {
        self.mine_drop_allowed_at(self.mine_drop_position(), tuning)
    }
}
