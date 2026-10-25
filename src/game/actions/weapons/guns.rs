use super::*;

impl Game {
    pub(in crate::game) fn fire_bullet(&mut self, _heading: Vec2) {
        let tuning = self.ship_tuning();
        let level = self.effective_gun_level();
        if level == 0 {
            return;
        }
        self.reveal_cloak_after_weapon_use();
        self.ship.energy -= self.effective_bullet_energy_cost(tuning).charged;
        (self.ship.bullet_cooldown, self.ship.bomb_cooldown) =
            gun_cooldowns_after_fire(self.ship.bomb_cooldown, tuning.bullet_cooldown);
        let frequency = self.player_frequency();
        let weapon_data = encode_weapon_data(
            if self.ship.bouncing_bullets { 2 } else { 1 },
            level,
            false,
            WEAPON_LEVEL_MIN,
            0,
            false,
        );
        self.queue_outbound_weapon(weapon_data);
        let damage = leveled_weapon_damage(
            self.tuning.bullet_damage,
            self.tuning.bullet_damage_upgrade,
            level,
        );
        for (projectile_index, projectile) in self
            .initialize_local_projectiles(weapon_data, self.ship.pos, self.ship.vel, tuning)
            .into_iter()
            .enumerate()
        {
            self.bullets.push(Bullet {
                prediction: self.local_weapon_prediction(projectile_index as u8),
                pos: vec2(projectile.position.x, projectile.position.y),
                vel: vec2(projectile.velocity.x, projectile.velocity.y),
                frequency,
                ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet),
                age: 0.0,
                trail_timer: 0.0,
                bounces_remaining: 0,
                bouncing: matches!(
                    projectile.behavior,
                    crate::gameplay_rules::ProjectileBehavior::BouncingBullet
                ),
                activates_on_wall: false,
                active: true,
                level,
                damage,
            });
        }

        if let Some(sound) = self.assets.gun_fire[weapon_level_index(level)].as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.24),
                },
            );
        }
    }

    pub(in crate::game) fn fire_multifire(&mut self) {
        let tuning = self.ship_tuning();
        let level = self.effective_gun_level();
        if level == 0 {
            return;
        }
        self.reveal_cloak_after_weapon_use();
        self.ship.energy -= self.effective_multifire_energy_cost(tuning).charged;
        (self.ship.bullet_cooldown, self.ship.bomb_cooldown) =
            gun_cooldowns_after_fire(self.ship.bomb_cooldown, tuning.multifire_cooldown);
        let frequency = self.player_frequency();
        let weapon_data = encode_weapon_data(
            if self.ship.bouncing_bullets { 2 } else { 1 },
            level,
            false,
            WEAPON_LEVEL_MIN,
            0,
            true,
        );
        self.queue_outbound_weapon(weapon_data);
        let damage = leveled_weapon_damage(
            self.tuning.bullet_damage,
            self.tuning.bullet_damage_upgrade,
            level,
        );

        for (projectile_index, projectile) in self
            .initialize_local_projectiles(weapon_data, self.ship.pos, self.ship.vel, tuning)
            .into_iter()
            .enumerate()
        {
            self.bullets.push(Bullet {
                prediction: self.local_weapon_prediction(projectile_index as u8),
                pos: vec2(projectile.position.x, projectile.position.y),
                vel: vec2(projectile.velocity.x, projectile.velocity.y),
                frequency,
                ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet),
                age: 0.0,
                trail_timer: 0.0,
                bounces_remaining: 0,
                bouncing: matches!(
                    projectile.behavior,
                    crate::gameplay_rules::ProjectileBehavior::BouncingBullet
                ),
                activates_on_wall: false,
                active: true,
                level,
                damage,
            });
        }

        if let Some(sound) = self.assets.gun_fire[weapon_level_index(level)].as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.28),
                },
            );
        }
    }
}
