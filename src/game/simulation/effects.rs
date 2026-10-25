use super::*;

impl Game {
    /// A wall's lifetime must advance even when a lifecycle barrier pauses
    /// gameplay. Run before ship/weapon collision and rendering each frame.
    pub(crate) fn update_bricks_at_server_time(&mut self) {
        self.update_bricks(0.0);
    }

    pub(in crate::game) fn update_bricks(&mut self, dt: f32) {
        let before = self.bricks.len();
        let now = self.browser_server_tick();
        let lifetime = self.tuning.brick_lifetime;
        let map = &mut self.maps[self.map_index];
        for brick in &mut self.bricks {
            if let Some(timestamp) = brick.server_timestamp {
                if let Some(now) = now {
                    brick.age = crate::continuum::ticks_to_seconds_i32(
                        crate::continuum::continuum_tick_diff(now, timestamp).max(0),
                    );
                    brick.ttl = (lifetime - brick.age).max(0.0);
                }
            } else {
                brick.age += dt;
                brick.ttl -= dt;
            }
            if brick.ttl <= 0.0 {
                map.set_tile(brick.tile.x, brick.tile.y, brick.previous_tile);
            }
        }
        self.bricks.retain(|brick| brick.ttl > 0.0);
        if self.bricks.len() != before {
            self.retire_expired_network_brick_lines();
        }
    }

    pub(in crate::game) fn update_decoys(&mut self, dt: f32) {
        self.update_decoys_with_clock(dt, None);
    }

    pub(in crate::game) fn update_decoys_at_server_time(&mut self) {
        if let Some(tick) = self.browser_server_tick() {
            self.update_decoys_with_clock(0.0, Some(tick));
        }
    }

    fn update_decoys_with_clock(&mut self, dt: f32, server_tick: Option<u32>) {
        let partial = self.weapon_render_partial_seconds();
        let local_orientation = self.ship.orientation;
        let local_player_id = self.client_authority.view().scope.player_id;
        let remote_players = &self.network_world.players;
        let map = &self.maps[self.map_index];
        let bricks = &self.bricks;
        for decoy in &mut self.decoys {
            let dt = match (&mut decoy.presentation_clock, server_tick) {
                (Some(clock), Some(now)) => {
                    if crate::continuum::continuum_tick_reached(now, clock.expiry_tick) {
                        decoy.ttl = 0.0;
                        continue;
                    }
                    let ticks = crate::continuum::continuum_tick_diff(now, clock.pose_tick);
                    if ticks < 0 {
                        clock.render_partial_seconds = 0.0;
                        continue;
                    }
                    if ticks > 20 || ticks as u32 > self.weapon_recovery_steps_remaining {
                        self.projectile_refresh_needed = true;
                        decoy.ttl = 0.0;
                        continue;
                    }
                    if self.networked_play && !self.door_clock_fresh {
                        clock.render_partial_seconds = 0.0;
                        continue;
                    }
                    self.weapon_recovery_steps_remaining -= ticks as u32;
                    clock.pose_tick = now;
                    clock.render_partial_seconds = partial;
                    decoy.ttl = crate::continuum::ticks_to_seconds_i32(
                        crate::continuum::continuum_tick_diff(clock.expiry_tick, now).max(0),
                    ) + ticks as f32 / 100.0;
                    ticks as f32 / 100.0
                }
                (None, None) => dt,
                _ => continue,
            };
            update_decoy_motion(map, bricks, decoy, dt);
            let owner_orientation = decoy
                .remote_source
                .filter(|source| Some(source.identity.player_id()) != local_player_id)
                .map(|source| {
                    remote_decoy_owner_orientation(
                        source,
                        remote_players.get(&source.identity.player_id()),
                    )
                })
                .unwrap_or(local_orientation);
            decoy.orientation =
                decoy_mirrored_orientation(decoy.initial_orientation, owner_orientation);
        }
        self.decoys.retain(|decoy| decoy.ttl > 0.0);
    }

    pub(in crate::game) fn update_repels(&mut self, dt: f32) {
        for repel in &mut self.repels {
            repel.age += dt;
        }
        self.repels.retain(|repel| repel_visual_active(repel.age));
    }

    pub(in crate::game) fn update_warps(&mut self, dt: f32) {
        for warp in &mut self.warps {
            warp.age += dt;
        }
        self.warps.retain(|warp| warp.age < WARP_LIFETIME);
    }

    pub(in crate::game) fn update_portal(&mut self, dt: f32) {
        if self.networked_play {
            self.reconcile_authoritative_portal();
            return;
        }
        if let Some(portal) = &mut self.portal {
            portal.age += dt;
            portal.ttl -= dt;
            if portal.ttl <= 0.0 {
                self.portal = None;
            }
        }
    }

    /// Return the endpoint only while its server deadline can be verified.
    /// Losing the browser clock hides and disables the endpoint fail-closed;
    /// retaining the exact record lets it become visible again when the clock
    /// resynchronizes before expiry.
    pub(in crate::game) fn active_portal(&self) -> Option<&Portal> {
        let portal = self.portal.as_ref()?;
        if !self.networked_play {
            return Some(portal);
        }
        let action_id = portal.server_action_id?;
        let expires_at_tick = portal.server_expires_at_tick?;
        if self.portal_endpoint_authority != Some(PortalEndpointAuthority::Active(action_id)) {
            return None;
        }
        if !self.door_clock_fresh {
            return None;
        }
        let current_tick = self.browser_server_tick()?;
        (!crate::continuum::continuum_tick_reached(current_tick, expires_at_tick)).then_some(portal)
    }

    pub(in crate::game) fn active_portal_ttl(&self) -> Option<f32> {
        let portal = self.active_portal()?;
        if !self.networked_play {
            return Some(portal.ttl.max(0.0));
        }
        Some(crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(
                portal.server_expires_at_tick?,
                self.browser_server_tick()?,
            )
            .max(0),
        ))
    }

    pub(in crate::game) fn install_authoritative_portal(
        &mut self,
        action_id: u32,
        pos: Vec2,
        placed_at_tick: u32,
        expires_at_tick: u32,
    ) -> bool {
        if !portal_action_advances(self.portal_endpoint_authority, action_id) {
            return false;
        }
        self.portal_endpoint_authority = Some(PortalEndpointAuthority::Active(action_id));
        self.portal = Some(Portal {
            pos,
            ttl: crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(expires_at_tick, placed_at_tick).max(0),
            ),
            age: 0.0,
            server_action_id: Some(action_id),
            server_placed_at_tick: Some(placed_at_tick),
            server_expires_at_tick: Some(expires_at_tick),
        });
        self.reconcile_authoritative_portal();
        true
    }

    pub(in crate::game) fn consume_authoritative_portal(&mut self, action_id: u32) -> bool {
        if !portal_action_advances(self.portal_endpoint_authority, action_id) {
            return false;
        }
        self.portal_endpoint_authority = Some(PortalEndpointAuthority::Consumed(action_id));
        self.portal = None;
        true
    }

    pub(in crate::game) fn reconcile_authoritative_portal(&mut self) {
        if !self.networked_play {
            return;
        }
        let current_tick = self
            .door_clock_fresh
            .then(|| self.browser_server_tick())
            .flatten();
        let Some(portal) = self.portal.as_mut() else {
            return;
        };
        let (Some(action_id), Some(placed_at_tick), Some(expires_at_tick)) = (
            portal.server_action_id,
            portal.server_placed_at_tick,
            portal.server_expires_at_tick,
        ) else {
            self.portal = None;
            return;
        };
        if self.portal_endpoint_authority != Some(PortalEndpointAuthority::Active(action_id)) {
            self.portal = None;
            return;
        }
        let Some(current_tick) = current_tick else {
            // Keep the exact endpoint record for clock resynchronization, but
            // active_portal() makes it unavailable while the clock is absent.
            return;
        };
        if crate::continuum::continuum_tick_reached(current_tick, expires_at_tick) {
            self.portal_endpoint_authority = Some(PortalEndpointAuthority::Expired(action_id));
            self.portal = None;
            return;
        }
        portal.ttl = crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(expires_at_tick, current_tick).max(0),
        );
        portal.age = crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(current_tick, placed_at_tick).max(0),
        );
    }

    pub(in crate::game) fn update_timed_prize_statuses(&mut self, dt: f32) {
        self.super_animation_age = if self.ship.super_timer > 0.0 {
            self.super_animation_age + dt
        } else {
            0.0
        };
        self.shield_animation_age = if self.ship.shield_timer > 0.0 {
            self.shield_animation_age + dt
        } else {
            0.0
        };
        tick_timed_status(&mut self.ship.super_timer, dt);
        tick_timed_status(&mut self.ship.shield_timer, dt);
        tick_timed_status(&mut self.ship.engine_shutdown_timer, dt);
        tick_timed_status(&mut self.ship.emp_shutdown_timer, dt);
        tick_timed_status(&mut self.ship.repel_timer, dt);
        for bot in &mut self.bots {
            tick_timed_status(&mut bot.super_timer, dt);
            tick_timed_status(&mut bot.shield_timer, dt);
            tick_timed_status(&mut bot.engine_shutdown_timer, dt);
            tick_timed_status(&mut bot.emp_shutdown_timer, dt);
        }
    }

    pub(in crate::game) fn update_explosions(&mut self, dt: f32) {
        let safe_zone_flag_drop = self.explosions.iter().any(|explosion| {
            safe_zone_weapon_hit_drops_flag(
                self.in_safe_zone(),
                self.carrying_flag,
                explosion_bot_damage_active(explosion.damages_bots, explosion.age)
                    && explosion_bot_damage(explosion, self.ship.pos) > 0.0,
            )
        });

        if safe_zone_flag_drop {
            self.drop_flags_after_safe_zone_weapon_hit();
        }

        let player_damage =
            if self.spectator_mode || self.player_respawn_timer > 0.0 || self.in_safe_zone() {
                0.0
            } else {
                let player_pos = self.ship.pos;
                let shield_lifetime = self.ship_tuning().shield_lifetime;
                let flagger_damage_multiplier = self.tuning.flagger_damage_multiplier;
                let flagger_active = self.flagger_active();
                self.explosions
                    .iter()
                    .filter(|explosion| {
                        explosion_bot_damage_active(explosion.damages_bots, explosion.age)
                    })
                    .map(|explosion| {
                        let damage = explosion_bot_damage(explosion, player_pos);
                        let damage = damage_after_timed_protection(
                            damage,
                            self.ship.super_timer,
                            self.ship.shield_timer,
                            shield_lifetime,
                        );
                        flagger_damage_received(damage, flagger_damage_multiplier, flagger_active)
                    })
                    .sum::<f32>()
            };

        if local_visual_damage_applies(self.networked_play, player_damage) {
            if !player_damage_is_fatal(self.ship.energy, player_damage) {
                self.apply_damage_prize_loss(player_damage);
            }
            self.ship.energy = local_bomb_self_damage_energy_after(self.ship.energy, player_damage);
        }

        self.advance_explosion_visuals(dt);
    }

    /// Animation retirement only; never evaluate contact, splash, energy or
    /// prize state while the local authoritative gameplay scope is closed.
    fn advance_explosion_visuals(&mut self, dt: f32) {
        for explosion in &mut self.explosions {
            explosion.age += dt;
        }
        self.explosions
            .retain(|explosion| explosion.age < explosion_visual_lifetime(explosion.kind));

        for explosion in &mut self.ship_explosions {
            explosion.age += dt;
        }
        self.ship_explosions
            .retain(|explosion| explosion.age < SHIP_EXPLOSION_LIFETIME);
    }

    pub(in crate::game) fn advance_waiting_visual_effects(&mut self, dt: f32) {
        self.update_bullet_trails(dt);
        self.update_bullet_impacts(dt);
        self.update_bomb_trails(dt);
        self.update_bomb_flashes(dt);
        self.advance_explosion_visuals(dt);
        self.update_burst_effects(dt);
        self.update_repels(dt);
        self.update_warps(dt);
        self.update_exhausts(dt);
        self.update_rocket_particles(dt);
        self.network_world.lvz.update(dt, &self.lvz_render_cache);
    }

    pub(in crate::game) fn update_burst_effects(&mut self, dt: f32) {
        for effect in &mut self.burst_effects {
            effect.age += dt;
        }
        self.burst_effects
            .retain(|effect| effect.age < BURST_EFFECT_LIFETIME);
    }

    pub(in crate::game) fn update_exhausts(&mut self, dt: f32) {
        for exhaust in &mut self.exhausts {
            exhaust.pos += exhaust.vel * dt;
            exhaust.age += dt;
        }
        self.exhausts
            .retain(|exhaust| exhaust.age < EXHAUST_LIFETIME);
    }

    pub(in crate::game) fn update_rocket_particles(&mut self, dt: f32) {
        for particle in &mut self.rocket_particles {
            particle.pos += particle.vel * dt;
            particle.age += dt;
        }
        self.rocket_particles
            .retain(|particle| particle.age < ROCKET_PARTICLE_LIFETIME);
    }
}

fn portal_action_advances(
    current: Option<PortalEndpointAuthority>,
    candidate_action_id: u32,
) -> bool {
    current.is_none_or(|current| {
        let delta = candidate_action_id.wrapping_sub(current.action_id());
        delta != 0 && delta < (1_u32 << 31)
    })
}

fn remote_decoy_owner_orientation(
    source: RemoteWeaponSource,
    current_player: Option<&NetworkPlayer>,
) -> f32 {
    current_player
        .filter(|player| {
            source.identity.matches_authenticated_runtime(
                player.player_id,
                player.source_session_id,
                player.life_seq,
            )
        })
        .map_or(source.fire_orientation, |player| player.orientation)
}

fn repel_visual_active(age: f32) -> bool {
    age < REPEL_LIFETIME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repel_visual_runs_one_sprite_sequence_not_arena_repel_time() {
        assert!(repel_visual_active(REPEL_LIFETIME - 0.001));
        assert!(!repel_visual_active(REPEL_LIFETIME));
        assert!(!repel_visual_active(REPEL_LIFETIME * 2.0));
    }

    #[test]
    fn authoritative_portal_fails_closed_without_clock_resyncs_and_expires_by_server_tick() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        assert!(game.install_authoritative_portal(7, vec2(1_000.0, 2_000.0), 100, 2_100,));
        assert!(
            game.portal.is_some(),
            "exact endpoint is retained for resync"
        );
        assert!(
            game.active_portal().is_none(),
            "an unavailable authoritative clock fails closed"
        );

        game.last_observed_browser_server_tick = Some(150);
        game.door_clock_fresh = true;
        game.update_portal(1_000.0);
        assert_eq!(
            game.active_portal().map(|portal| portal.pos),
            Some(vec2(1_000.0, 2_000.0))
        );
        assert_eq!(game.active_portal_ttl(), Some(19.5));

        game.door_clock_fresh = false;
        assert!(
            game.last_observed_browser_server_tick.is_some(),
            "the stale-clock regression retains a stale Some tick"
        );
        assert!(game.active_portal().is_none());
        game.door_clock_fresh = true;
        assert_eq!(
            game.active_portal().map(|portal| portal.pos),
            Some(vec2(1_000.0, 2_000.0)),
            "a genuinely fresh predeadline clock resynchronizes the retained endpoint"
        );
        game.last_observed_browser_server_tick = Some(2_100);
        game.update_portal(0.0);
        assert!(game.portal.is_none());
        assert_eq!(
            game.portal_endpoint_authority,
            Some(PortalEndpointAuthority::Expired(7))
        );
    }

    #[test]
    fn portal_endpoint_generation_accepts_wrap_forward_only() {
        assert!(portal_action_advances(None, 9));
        assert!(portal_action_advances(
            Some(PortalEndpointAuthority::Active(u32::MAX)),
            1
        ));
        assert!(!portal_action_advances(
            Some(PortalEndpointAuthority::Consumed(10)),
            10
        ));
        assert!(!portal_action_advances(
            Some(PortalEndpointAuthority::Expired(10)),
            9
        ));
    }

    #[test]
    fn old_decoy_never_tracks_replacement_orientation_after_session_life_wrap() {
        let identity = RemoteWeaponSourceIdentity::authenticated(9, u64::MAX, u32::MAX).unwrap();
        let source = RemoteWeaponSource::capture(
            identity,
            RemoteWeaponArenaScope::authenticated(u64::MAX, u64::MAX),
            vec2(100.0, 200.0),
            Vec2::ZERO,
            0.75,
            7,
            0,
            ShipTuning::continuum_skirmish_default(),
            true,
        )
        .unwrap();
        let mut player = NetworkPlayer::from_roster(&PlayerRosterEntry {
            player_id: 9,
            ship: 0,
            audio: 0,
            frequency: 7,
            name: "replacement".to_string(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            wins: 0,
            losses: 0,
            attach_parent: u16::MAX,
            flags: 0,
            koth: 0,
        });
        player.set_life_seq(Some(u32::MAX));
        assert!(player.bind_source_session(u64::MAX, u32::MAX));
        player.orientation = 0.25;
        assert_eq!(remote_decoy_owner_orientation(source, Some(&player)), 0.25);

        player.set_life_seq(Some(1));
        assert!(player.bind_source_session(1, 1));
        player.orientation = 0.5;
        assert_eq!(
            remote_decoy_owner_orientation(source, Some(&player)),
            source.fire_orientation
        );
        assert_eq!(remote_decoy_owner_orientation(source, None), 0.75);
    }
}
