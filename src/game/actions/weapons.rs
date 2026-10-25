use super::*;

mod explosives;
mod guns;
mod prediction;
mod specials;

impl Game {
    fn initialize_local_projectiles(
        &self,
        weapon_data: u16,
        position: Vec2,
        velocity: Vec2,
        tuning: ShipTuning,
    ) -> Vec<crate::gameplay_rules::ProjectileInitialization> {
        crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
            weapon_data,
            crate::gameplay_rules::RulesVector::new(position.x, position.y),
            crate::gameplay_rules::RulesVector::new(velocity.x, velocity.y),
            crate::continuum_position::protocol_direction(self.ship.orientation),
            crate::gameplay_rules::ProjectileInitializationTuning {
                bullet_speed: tuning.bullet_speed,
                bomb_speed: tuning.bomb_speed,
                burst_speed: tuning.burst_speed,
                ship_radius: tuning.radius_pixels,
                multifire_angle: tuning.multifire_angle,
                double_barrel: tuning.double_barrel,
                burst_count: tuning.burst_bullet_count,
                bomb_bounce_count: tuning.bomb_bounces,
            },
        )
    }

    fn mine_drop_position(&self) -> Vec2 {
        self.ship.pos
    }

    fn mine_drop_allowed_at(&self, drop_pos: Vec2, tuning: ShipTuning) -> bool {
        let (player_mines, team_mines, overlaps_existing_mine) = self.active_mine_counts(drop_pos);
        mine_drop_allowed(
            player_mines,
            team_mines,
            tuning.max_mines,
            self.tuning.team_max_mines,
            overlaps_existing_mine,
        )
    }

    fn active_mine_counts(&self, check_pos: Vec2) -> (usize, usize, bool) {
        let frequency = self.player_frequency();
        let mut player_mines = self.mines.len();
        let mut team_mines = player_mines;
        let mut overlaps_existing_mine = self
            .mines
            .iter()
            .any(|mine| mine_positions_overlap(mine.pos, check_pos));

        for projectile in &self.remote_projectiles {
            if !matches!(projectile.kind, RemoteProjectileKind::Mine { .. }) {
                continue;
            }
            if Some(projectile.source.identity.player_id())
                == self.client_authority.view().scope.player_id
            {
                player_mines += 1;
            }
            if projectile.frequency == frequency {
                team_mines += 1;
            }
            overlaps_existing_mine |= mine_positions_overlap(projectile.pos, check_pos);
        }

        (player_mines, team_mines, overlaps_existing_mine)
    }

    fn reveal_cloak_after_weapon_use(&mut self) {
        if weapon_use_reveals_cloak(self.in_safe_zone(), self.ship.cloak) {
            self.ship.cloak = false;
            self.ship.flash = true;
        }
    }

    fn queue_outbound_weapon(&mut self, weapon: u16) {
        if let Some(activation) = self.pending_weapon_prediction.take() {
            self.remove_unsubmitted_weapon_prediction(activation);
        }
        self.next_weapon_prediction_id = self.next_weapon_prediction_id.wrapping_add(1).max(1);
        self.pending_weapon_prediction = self
            .networked_play
            .then_some(self.next_weapon_prediction_id);
        self.pending_weapon = weapon;

        #[cfg(target_arch = "wasm32")]
        if let Some(browser) = &mut self.browser_network {
            browser.position_timer = 0.0;
        }
    }

    fn bomb_weapon_data(&self, level: u8, mine: bool) -> u16 {
        encode_weapon_data(
            if self.ship.proximity_bombs { 4 } else { 3 },
            level,
            self.ship.bouncing_bullets,
            explosive_shrapnel_level(level),
            self.ship.shrapnel_upgrades,
            mine,
        )
    }

    fn current_explosive_shrapnel(
        &self,
        tuning: ShipTuning,
        pos: Vec2,
        vel: Vec2,
        weapon_level: u8,
        frequency: u16,
    ) -> ExplosiveShrapnel {
        let level = explosive_shrapnel_level(weapon_level);
        let count = if level >= WEAPON_LEVEL_MIN {
            shrapnel_count(self.ship.shrapnel_upgrades, tuning.shrapnel_max)
        } else {
            0
        };
        ExplosiveShrapnel {
            count,
            level,
            bouncing: self.ship.bouncing_bullets,
            rng_seed: shrapnel_rng_seed(pos, vel, count, weapon_level, frequency),
        }
    }

    fn apply_repel_effect(&mut self) {
        let origin = self.ship.pos;
        let repel_radius = self.tuning.repel_radius;
        let repel_speed = self.tuning.repel_speed;
        let frequency = self.player_frequency();
        for projectile in &mut self.remote_projectiles {
            if !repel_can_affect_frequency(frequency, projectile.frequency) {
                continue;
            }
            let repelled = repel_velocity(
                origin,
                &mut projectile.pos,
                &mut projectile.vel,
                repel_speed,
                repel_radius,
            );
            if repelled {
                if let RemoteProjectileKind::Mine { proximity } = projectile.kind {
                    projectile.kind = RemoteProjectileKind::Bomb { proximity };
                    refresh_repelled_weapon_lifetime(
                        &mut projectile.ttl,
                        weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb),
                    );
                    projectile.bounces_remaining = remote_mine_repel_bounces(projectile);
                    projectile.proximity_sensor = None;
                } else {
                    let lifetime = match projectile.kind {
                        RemoteProjectileKind::Bullet { .. }
                        | RemoteProjectileKind::Burst { .. } => {
                            weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet)
                        }
                        RemoteProjectileKind::Shrapnel { .. } => {
                            weapon_lifetime(&self.tuning, WeaponLifetimeKind::Shrapnel)
                        }
                        RemoteProjectileKind::Bomb { .. } | RemoteProjectileKind::Thor => {
                            weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb)
                        }
                        RemoteProjectileKind::Mine { .. } => {
                            weapon_lifetime(&self.tuning, WeaponLifetimeKind::Mine)
                        }
                    };
                    if matches!(projectile.kind, RemoteProjectileKind::Shrapnel { .. }) {
                        refresh_repelled_shrapnel_lifetime(
                            &mut projectile.ttl,
                            &mut projectile.age,
                            lifetime,
                        );
                    } else {
                        refresh_repelled_weapon_lifetime(&mut projectile.ttl, lifetime);
                    }
                }
            }
        }

        for bullet in &mut self.bot_bullets {
            if repel_velocity(
                origin,
                &mut bullet.pos,
                &mut bullet.vel,
                repel_speed,
                repel_radius,
            ) {
                refresh_repelled_weapon_lifetime(
                    &mut bullet.ttl,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet),
                );
            }
        }

        for decoy in &mut self.decoys {
            if repel_can_affect_frequency(frequency, decoy.frequency) {
                if repel_velocity(
                    origin,
                    &mut decoy.pos,
                    &mut decoy.vel,
                    repel_speed,
                    repel_radius,
                ) {
                    refresh_repelled_weapon_lifetime(
                        &mut decoy.ttl,
                        weapon_lifetime(&self.tuning, WeaponLifetimeKind::Decoy),
                    );
                }
            }
        }

        for bot in &mut self.bots {
            if bot.respawn_timer <= 0.0 {
                repel_velocity(
                    origin,
                    &mut bot.pos,
                    &mut bot.vel,
                    repel_speed,
                    repel_radius,
                );
            }
        }
    }
}
