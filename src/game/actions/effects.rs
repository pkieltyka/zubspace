use super::*;

fn thrust_sound_transition(
    started: bool,
    reversing: bool,
    afterburning: bool,
    next_reversing: bool,
    next_afterburning: bool,
) -> (bool, bool) {
    let direction_changed = started && reversing != next_reversing;
    let afterburner_started = next_afterburning && (!started || !afterburning);
    (
        !started || direction_changed || afterburner_started,
        afterburner_started,
    )
}

impl Game {
    pub(in crate::game) fn add_bullet_impact(&mut self, pos: Vec2) {
        self.add_bullet_impact_with_source(pos, None);
    }

    pub(in crate::game) fn add_remote_bullet_impact(
        &mut self,
        pos: Vec2,
        source: RemoteWeaponSourceIdentity,
    ) {
        self.add_bullet_impact_with_source(pos, Some(source));
    }

    fn add_bullet_impact_with_source(
        &mut self,
        pos: Vec2,
        remote_source: Option<RemoteWeaponSourceIdentity>,
    ) {
        self.bullet_impacts.push(BulletImpact {
            pos,
            age: 0.0,
            remote_source,
        });

        if let Some(sound) = self.assets.bullet_impact_sound.as_ref() {
            let volume = if remote_source.is_some() {
                let Some(volume) = self.positional_sound_volume(pos, 0.18) else {
                    return;
                };
                volume
            } else {
                self.effect_volume(0.18)
            };
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume,
                },
            );
        }
    }

    pub(in crate::game) fn spawn_exhaust(&mut self, heading: Vec2, reversing: bool, dt: f32) {
        self.exhaust_timer -= dt;
        if self.exhaust_timer > 0.0 {
            return;
        }
        self.exhaust_timer = EXHAUST_INTERVAL;
        let ship_radius = self.ship_tuning().radius_pixels;
        spawn_exhaust_particles(
            &mut self.exhausts,
            self.ship.pos,
            heading,
            reversing,
            ship_radius,
        );
    }

    pub(in crate::game) fn spawn_rocket_exhaust(&mut self, heading: Vec2, dt: f32) {
        self.exhaust_timer -= dt;
        if self.exhaust_timer > 0.0 {
            return;
        }
        self.exhaust_timer = EXHAUST_INTERVAL;

        let origin = self.ship.pos - heading * (self.ship_tuning().radius_pixels + 3.0);
        let side = perpendicular(heading) * 7.0;
        let back = -heading * 95.0;

        for (offset, spread) in [(-side, -0.3_f32), (Vec2::ZERO, 0.0), (side, 0.3)] {
            let velocity = (back + perpendicular(heading) * spread * 75.0).normalize_or_zero();
            self.rocket_particles.push(RocketParticle {
                pos: origin + offset - vec2(12.0, 12.0),
                vel: velocity * 95.0,
                age: 0.0,
            });
        }
    }

    pub(in crate::game) fn add_explosion(
        &mut self,
        pos: Vec2,
        level: u8,
        damage: f32,
        emp_shutdown_time: f32,
        shrapnel: ExplosiveShrapnel,
    ) {
        self.add_explosion_with_damage_pos(pos, pos, level, damage, emp_shutdown_time, shrapnel);
    }

    pub(in crate::game) fn add_explosion_with_damage_pos(
        &mut self,
        pos: Vec2,
        damage_pos: Vec2,
        level: u8,
        damage: f32,
        emp_shutdown_time: f32,
        shrapnel: ExplosiveShrapnel,
    ) {
        self.add_predicted_explosion(
            pos,
            damage_pos,
            level,
            damage,
            emp_shutdown_time,
            shrapnel,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::game) fn add_predicted_explosion(
        &mut self,
        pos: Vec2,
        damage_pos: Vec2,
        level: u8,
        damage: f32,
        emp_shutdown_time: f32,
        shrapnel: ExplosiveShrapnel,
        prediction: Option<crate::model::OwnedProjectilePrediction>,
    ) {
        if let Some(prediction) = prediction {
            self.predicted_explosion_receipts.push_back(prediction);
            while self.predicted_explosion_receipts.len()
                > crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS
            {
                self.predicted_explosion_receipts.pop_front();
            }
        }
        let kind = explosion_kind_for_emp_shutdown(emp_shutdown_time);
        self.explosions.push(Explosion {
            prediction,
            pos,
            damage_pos,
            age: 0.0,
            kind,
            damages_bots: true,
            damage,
            radius: bomb_explosion_radius(self.tuning.bomb_explode_pixels, level),
            emp_shutdown_time,
            remote_source: None,
        });
        self.spawn_shrapnel(pos, shrapnel);
        self.play_bomb_explosion_sound(kind);
    }

    pub(in crate::game) fn add_remote_visual_explosion(
        &mut self,
        pos: Vec2,
        emp_shutdown_time: f32,
        source: RemoteWeaponSourceIdentity,
    ) {
        let kind = explosion_kind_for_emp_shutdown(emp_shutdown_time);
        self.explosions.push(Explosion {
            prediction: None,
            pos,
            damage_pos: pos,
            age: 0.0,
            kind,
            damages_bots: false,
            damage: 0.0,
            radius: 0.0,
            emp_shutdown_time,
            remote_source: Some(source),
        });
        #[cfg(not(test))]
        self.play_positional_bomb_explosion_sound(kind, pos);
    }

    pub(in crate::game) fn add_ship_explosion(&mut self, pos: Vec2) {
        self.ship_explosions.push(ShipExplosion { pos, age: 0.0 });
        #[cfg(test)]
        if self.assets.0.is_none() {
            return;
        }
        self.play_ship_explosion_sound(self.effect_volume(0.34));
    }

    pub(in crate::game) fn add_remote_ship_explosion(
        &mut self,
        pos: Vec2,
        death: NetworkDeathIdentity,
    ) {
        self.ship_explosions.push(ShipExplosion { pos, age: 0.0 });
        self.remote_ship_explosion_count = self.remote_ship_explosion_count.wrapping_add(1);
        self.last_remote_ship_explosion_position = Some(pos);
        self.remote_ship_explosion_witnesses
            .push_back(RemoteShipExplosionWitness {
                death,
                ordinal: self.remote_ship_explosion_count,
                position: pos,
            });
        while self.remote_ship_explosion_witnesses.len() > MAX_REMOTE_SHIP_EXPLOSION_WITNESSES {
            self.remote_ship_explosion_witnesses.pop_front();
        }
        #[cfg(test)]
        if self.assets.0.is_none() {
            return;
        }
        if let Some(volume) = self.positional_sound_volume(pos, 0.34) {
            self.play_ship_explosion_sound(volume);
        }
    }

    fn play_ship_explosion_sound(&self, volume: f32) {
        if let Some(sound) = self.assets.ship_explode.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume,
                },
            );
        }
    }

    fn play_positional_bomb_explosion_sound(&self, kind: ExplosionKind, pos: Vec2) {
        let Some(volume) = self.positional_sound_volume(pos, 0.34) else {
            return;
        };
        let sound = match kind {
            ExplosionKind::Bomb => self.assets.bomb_explode.as_ref(),
            ExplosionKind::EmpBomb => self.assets.emp_bomb_explode.as_ref(),
        };
        if let Some(sound) = sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume,
                },
            );
        }
    }

    fn play_bomb_explosion_sound(&self, kind: ExplosionKind) {
        #[cfg(test)]
        if self.assets.0.is_none() {
            return;
        }
        let sound = match kind {
            ExplosionKind::Bomb => self.assets.bomb_explode.as_ref(),
            ExplosionKind::EmpBomb => self.assets.emp_bomb_explode.as_ref(),
        };
        if let Some(sound) = sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.34),
                },
            );
        }
    }

    fn spawn_shrapnel(&mut self, pos: Vec2, shrapnel: ExplosiveShrapnel) {
        if self.networked_play || shrapnel.count == 0 {
            return;
        }

        let count = shrapnel.count;
        let level = shrapnel.level;
        let seed = shrapnel.rng_seed;
        let bouncing = shrapnel.bouncing;
        let damage = active_shrapnel_damage(
            self.tuning.bullet_damage,
            self.tuning.bullet_damage_upgrade,
            self.tuning.shrapnel_damage_multiplier,
            level,
        );
        let inactive_damage = inactive_shrapnel_damage(self.tuning.inactive_shrapnel_damage, level);
        let frequency = self.player_frequency();
        let spawn_pos = shrapnel_projectile_position(pos);
        if !shrapnel_spawn_allowed(self.map(), spawn_pos, frequency, &self.bricks) {
            return;
        }

        let parent_weapon_data = encode_weapon_data(3, 1, bouncing, level, count as u8, false);
        for projectile in crate::gameplay_rules::initialize_shrapnel_projectiles(
            parent_weapon_data,
            crate::gameplay_rules::RulesVector::new(spawn_pos.x, spawn_pos.y),
            seed,
            self.tuning.shrapnel_speed,
            self.tuning.shrapnel_random,
        ) {
            self.shrapnel.push(Shrapnel {
                level,
                pos: spawn_pos,
                vel: vec2(projectile.velocity.x, projectile.velocity.y),
                frequency,
                ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Shrapnel),
                age: 0.0,
                bouncing,
                damage,
                inactive_damage,
            });
        }
    }

    pub(in crate::game) fn start_thrust_sound(&mut self, reversing: bool, afterburning: bool) {
        let (play_transition, afterburner_started) = thrust_sound_transition(
            self.thrust_sound_started,
            self.thrust_sound_reversing,
            self.thrust_sound_afterburning,
            reversing,
            afterburning,
        );
        if !play_transition {
            self.thrust_sound_afterburning = afterburning;
            return;
        }

        let sound = if reversing {
            self.assets.reverse_thrust.as_ref()
        } else {
            self.assets.thrust.as_ref()
        };

        if let Some(sound) = sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.22),
                },
            );
        }
        if afterburner_started {
            self.afterburner_sound_play_count = self.afterburner_sound_play_count.wrapping_add(1);
        }
        self.thrust_sound_started = true;
        self.thrust_sound_reversing = reversing;
        self.thrust_sound_afterburning = afterburning;
    }
}

#[cfg(test)]
mod tests {
    use super::thrust_sound_transition;

    #[test]
    fn afterburner_entry_retriggers_thrust_cue_without_repeating_while_held() {
        assert_eq!(
            thrust_sound_transition(false, false, false, false, false),
            (true, false)
        );
        assert_eq!(
            thrust_sound_transition(true, false, false, false, true),
            (true, true)
        );
        assert_eq!(
            thrust_sound_transition(true, false, true, false, true),
            (false, false)
        );
        assert_eq!(
            thrust_sound_transition(true, false, true, false, false),
            (false, false)
        );
        assert_eq!(
            thrust_sound_transition(true, false, false, false, true),
            (true, true),
            "re-entering Afterburner while ordinary thrust stays held owns one new cue"
        );
    }
}
