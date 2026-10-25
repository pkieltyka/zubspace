use super::*;

impl Game {
    pub(in crate::game) fn fire_burst(&mut self) {
        let tuning = self.ship_tuning();
        let in_safe = self.in_safe_zone();
        let cooldown = self.effective_bomb_cooldown(tuning.burst_cooldown);
        self.reveal_cloak_after_weapon_use();
        self.ship.burst_cooldown = cooldown;
        self.ship.bomb_cooldown = cooldown;
        self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(cooldown);
        self.ship.repel_cooldown = repel_cooldown_after_weapon(self.ship.repel_cooldown);
        self.ship.bursts = special_inventory_after_use(self.ship.bursts, in_safe);
        if in_safe {
            self.ship.vel = Vec2::ZERO;
            return;
        }

        self.ship.energy = special_weapon_energy_after_fire(self.ship.energy);
        let weapon_data =
            encode_weapon_data(7, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false);
        self.queue_outbound_weapon(weapon_data);
        let level = self.ship.gun_level;
        let damage = self.tuning.burst_damage;
        let frequency = self.player_frequency();
        self.burst_effects.push(BurstEffect {
            pos: self.ship.pos,
            age: 0.0,
            remote_source: None,
        });

        for (projectile_index, projectile) in self
            .initialize_local_projectiles(weapon_data, self.ship.pos, self.ship.vel, tuning)
            .into_iter()
            .enumerate()
        {
            let active = match projectile.behavior {
                crate::gameplay_rules::ProjectileBehavior::Burst { active } => active,
                _ => true,
            };
            self.bullets.push(Bullet {
                prediction: self.local_weapon_prediction(projectile_index as u8),
                pos: vec2(projectile.position.x, projectile.position.y),
                vel: vec2(projectile.velocity.x, projectile.velocity.y),
                frequency,
                ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Burst),
                age: 0.0,
                trail_timer: 0.0,
                bounces_remaining: 0,
                bouncing: false,
                activates_on_wall: matches!(
                    projectile.behavior,
                    crate::gameplay_rules::ProjectileBehavior::Burst { .. }
                ),
                active,
                level,
                damage,
            });
        }

        if let Some(sound) = self.assets.burst_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.32),
                },
            );
        }
    }

    pub(in crate::game) fn fire_repel(&mut self) {
        let tuning = self.ship_tuning();
        let in_safe = self.in_safe_zone();
        let cooldown = self.effective_bomb_cooldown(tuning.bomb_cooldown);
        self.reveal_cloak_after_weapon_use();
        self.ship.repel_cooldown = self.ship.repel_cooldown.max(tuning.repel_cooldown);
        self.ship.bomb_cooldown = cooldown;
        self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(cooldown);
        self.ship.repels = special_inventory_after_use(self.ship.repels, in_safe);
        if in_safe {
            self.ship.vel = Vec2::ZERO;
            return;
        }

        self.queue_outbound_weapon(encode_weapon_data(
            5,
            WEAPON_LEVEL_MIN,
            false,
            WEAPON_LEVEL_MIN,
            0,
            false,
        ));
        self.ship.energy = special_weapon_energy_after_fire(self.ship.energy);
        self.repels.push(RepelEffect {
            pos: self.ship.pos,
            age: 0.0,
            remote_source: None,
        });
        self.apply_repel_effect();

        if let Some(sound) = self.assets.repel_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.36),
                },
            );
        }
    }

    pub(in crate::game) fn launch_decoy(&mut self) {
        let tuning = self.ship_tuning();
        let in_safe = self.in_safe_zone();
        let cooldown = self.effective_bomb_cooldown(tuning.decoy_cooldown);
        self.reveal_cloak_after_weapon_use();
        self.ship.decoys = special_inventory_after_use(self.ship.decoys, in_safe);
        self.ship.bomb_cooldown = cooldown;
        self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(cooldown);
        self.ship.repel_cooldown = repel_cooldown_after_weapon(self.ship.repel_cooldown);
        if in_safe {
            self.ship.vel = Vec2::ZERO;
            return;
        }

        self.queue_outbound_weapon(encode_weapon_data(
            6,
            WEAPON_LEVEL_MIN,
            false,
            WEAPON_LEVEL_MIN,
            0,
            false,
        ));
        self.decoys.push(Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: self.local_weapon_prediction(0),
            pos: self.ship.pos,
            vel: self.ship.vel,
            frequency: self.player_frequency(),
            orientation: self.ship.orientation,
            initial_orientation: self.ship.orientation,
            remote_source: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: self.ship.ship_type,
            owner_name: self.local_player_name().to_owned(),
            owner_bounty: self.player_bounty,
            ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Decoy),
            age: 0.0,
        });

        if let Some(sound) = self.assets.decoy_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.34),
                },
            );
        }
    }

    pub(in crate::game) fn fire_thor(&mut self, heading: Vec2) {
        let tuning = self.ship_tuning();
        let in_safe = self.in_safe_zone();
        let cooldown = self.effective_bomb_cooldown(tuning.thor_cooldown);
        self.reveal_cloak_after_weapon_use();
        self.ship.thors = special_inventory_after_use(self.ship.thors, in_safe);
        self.ship.bomb_cooldown = cooldown;
        self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(cooldown);
        self.ship.repel_cooldown = repel_cooldown_after_weapon(self.ship.repel_cooldown);
        if in_safe {
            self.ship.vel = Vec2::ZERO;
            return;
        }

        let weapon_data =
            encode_weapon_data(8, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false);
        self.queue_outbound_weapon(weapon_data);
        let projectile = self
            .initialize_local_projectiles(weapon_data, self.ship.pos, self.ship.vel, tuning)
            .into_iter()
            .next()
            .expect("thor weapon initializes one projectile");
        self.thors.push(Thor {
            prediction: self.local_weapon_prediction(0),
            pos: vec2(projectile.position.x, projectile.position.y),
            vel: vec2(projectile.velocity.x, projectile.velocity.y),
            ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Thor),
            age: 0.0,
            damage: thor_damage_for_ship(
                self.tuning.bomb_damage,
                self.tuning.bouncing_bomb_damage_multiplier,
                tuning.bomb_bounces,
                0,
            ),
            shrapnel: ExplosiveShrapnel::none(),
            proximity_sensor: None,
            // Locally-fired Thor is encoded with WEAPON_LEVEL_MIN above, so
            // the wire-format level bits are 0 (= raw_level 0). Threading the
            // raw level through the projectile lets remote Thors at higher
            // levels size their radii correctly per the reference game.
            raw_level: projectile.raw_level,
        });
        self.bomb_flashes.push(BombFlash {
            pos: bomb_flash_position(self.ship.pos, heading, tuning.radius_pixels),
            age: 0.0,
            remote_source: None,
        });

        if let Some(sound) = self.assets.thor_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.36),
                },
            );
        }
    }

    pub(in crate::game) fn activate_rocket(&mut self, activation: RocketActivation) {
        let tuning = self.ship_tuning();
        if self.networked_play {
            self.pending_rocket_activation_sequence = Some(activation.pulse_sequence);
        }
        self.ship.rocket_timer = tuning.rocket_lifetime;
        self.ship.rocket_cooldown = 0.0;
        self.ship.rockets = self.ship.rockets.saturating_sub(1);
        // Audio follows the reconciled active effect once per render frame,
        // rather than playing a short one-shot on each activation prediction.
    }
}
