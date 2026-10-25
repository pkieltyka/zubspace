use super::*;

mod damage;
mod draw_view;
mod owner_bomb;
mod view_history;

use damage::*;
use view_history::*;

impl Game {
    pub(in crate::game) fn remote_weapon_recovery_steps(elapsed: f32) -> u32 {
        (elapsed.max(0.0) / crate::gameplay_rules::weapon_motion::STEP_SECONDS).ceil() as u32
    }

    pub(crate) fn reset_weapon_recovery_budget(&mut self) {
        self.weapon_recovery_steps_remaining = 4096;
    }

    /// Advance one newly reconstructed remote projectile from the server's
    /// capture tick to the browser's current server clock without applying
    /// target contact, damage, audio, or effects. Wall, door, brick, gravity,
    /// bounce, lifetime, and wormhole behavior still use the ordinary shared
    /// presentation simulator.
    pub(in crate::game) fn catch_up_remote_projectile_presentation(
        &mut self,
        projectile: &mut RemoteProjectile,
        elapsed: f32,
    ) {
        self.catch_up_remote_projectile_presentation_from_tick(projectile, elapsed, None);
    }

    pub(in crate::game) fn catch_up_remote_projectile_presentation_from_tick(
        &mut self,
        projectile: &mut RemoteProjectile,
        elapsed: f32,
        captured_tick: Option<u32>,
    ) {
        if elapsed >= projectile.ttl {
            projectile.age += projectile.ttl.max(0.0);
            projectile.ttl = 0.0;
            return;
        }
        let captured_tick = captured_tick.filter(|_| {
            captures_remote_weapon_view(
                self.networked_play,
                self.client_authority.view().scope,
                projectile,
            )
        });
        let start_age = projectile.age;
        // At a capture/impulse boundary TTL already belongs to that boundary;
        // the live clock can still describe the preceding Repel generation.
        let expiry_tick = captured_tick.map(|tick| {
            crate::continuum::continuum_tick_add(tick, (projectile.ttl * 100.0).round() as u32)
        });
        if projectile
            .presentation_clock
            .is_some_and(|clock| clock.motion_frozen)
        {
            // There is no flight to replay, even across a long scheduler pause.
            if let (Some(tick), Some(expiry)) = (captured_tick, expiry_tick) {
                record_weapon_view_step(
                    &mut self.remote_weapon_view_history,
                    projectile,
                    tick,
                    start_age,
                    expiry,
                    0.0,
                );
            }
            projectile.age += elapsed.max(0.0);
            projectile.ttl -= elapsed.max(0.0);
            if let (Some(tick), Some(expiry)) = (captured_tick, expiry_tick) {
                record_weapon_view_step(
                    &mut self.remote_weapon_view_history,
                    projectile,
                    tick,
                    start_age,
                    expiry,
                    elapsed.max(0.0),
                );
            }
            return;
        }
        let steps = Self::remote_weapon_recovery_steps(elapsed);
        if steps > 1000 || steps > self.weapon_recovery_steps_remaining {
            self.projectile_refresh_needed = true;
            projectile.ttl = 0.0;
            return;
        }
        self.weapon_recovery_steps_remaining -= steps;
        if let (Some(tick), Some(expiry)) = (captured_tick, expiry_tick) {
            record_weapon_view_step(
                &mut self.remote_weapon_view_history,
                projectile,
                tick,
                start_age,
                expiry,
                0.0,
            );
        }
        let map = &self.maps[self.map_index];
        let bricks = &self.bricks;
        for index in 0..steps {
            if projectile.ttl <= 0.0 {
                break;
            }
            let dt = (elapsed - index as f32 * crate::gameplay_rules::weapon_motion::STEP_SECONDS)
                .clamp(0.0, crate::gameplay_rules::weapon_motion::STEP_SECONDS);
            projectile.ttl -= dt;
            projectile.age += dt;
            projectile.trail_timer -= dt;
            match projectile.kind {
                RemoteProjectileKind::Bullet { .. }
                | RemoteProjectileKind::Shrapnel { .. }
                | RemoteProjectileKind::Burst { .. } => {
                    if update_remote_bullet_motion(
                        projectile,
                        dt,
                        map,
                        bricks,
                        Vec2::ZERO,
                        0.0,
                        false,
                    )
                    .0
                    .is_some()
                    {
                        projectile.ttl = 0.0;
                    }
                }
                RemoteProjectileKind::Bomb { .. } | RemoteProjectileKind::Mine { .. } => {
                    if update_remote_bomb_motion(
                        projectile,
                        dt,
                        map,
                        bricks,
                        Vec2::ZERO,
                        0.0,
                        self.tuning,
                        projectile.source.bomb_gravity,
                        false,
                    )
                    .0
                    .is_some()
                    {
                        projectile.ttl = 0.0;
                    }
                }
                RemoteProjectileKind::Thor => projectile.pos += projectile.vel * dt,
            }
            if weapon_times_out_on_wormhole_tile(map, projectile.pos) {
                projectile.ttl = 0.0;
            }
            if let (Some(tick), Some(expiry)) = (captured_tick, expiry_tick) {
                record_weapon_view_step(
                    &mut self.remote_weapon_view_history,
                    projectile,
                    tick,
                    start_age,
                    expiry,
                    index as f32 * crate::gameplay_rules::weapon_motion::STEP_SECONDS + dt,
                );
            }
        }
    }

    pub(in crate::game) fn update_remote_projectiles(&mut self, dt: f32) {
        self.update_remote_projectiles_with_clock(dt, None);
    }

    /// Received authoritative weapons follow elapsed server time, independently
    /// of the capped local input/ship scheduler. Only integer 100 Hz steps
    /// mutate their pose; the fractional tick is a disposable draw projection.
    pub(in crate::game) fn update_remote_weapons_at_server_time(&mut self) {
        if let Some(tick) = self.browser_server_tick() {
            self.update_remote_projectiles_with_clock(0.0, Some(tick));
        }
    }

    fn update_remote_projectiles_with_clock(&mut self, dt: f32, server_tick: Option<u32>) {
        let render_partial = self.weapon_render_partial_seconds();
        let mut recovery_steps_left = 2048_u32;
        let player_pos = self.ship.pos;
        // The server owns player contacts. A current local hull/proximity
        // prediction must not erase a remote copy before its terminal arrives:
        // server geometry uses the admitted observer timeline, not this frame.
        let predict_player_contacts = !self.networked_play;
        let player_frequency = self.player_frequency();
        let player_safe = self.in_safe_zone();
        let player_respawning = self.player_respawn_timer > 0.0 || self.spectator_mode;
        let player_tuning = self.ship_tuning();
        let player_radius = player_tuning.radius_pixels;
        let player_shield_lifetime = player_tuning.shield_lifetime;
        let flagger_damage_multiplier = self.tuning.flagger_damage_multiplier;
        let flagger_active = self.flagger_active();
        let networked = self.networked_play;
        let view_scope = self.client_authority.view().scope;
        let mut impacts = Vec::new();
        let mut explosions = Vec::new();
        let mut remote_damage = 0.0;
        let mut hit_by: Option<RemoteWeaponSourceIdentity> = None;
        let mut bounce_sound = None;
        let mut safe_zone_flag_drop = false;
        let mut expiry_view_tails = Vec::new();

        {
            let map = &self.maps[self.map_index];
            let bricks = &self.bricks;

            for projectile in &mut self.remote_projectiles {
                let initial_clock = projectile.presentation_clock;
                let initial_age = projectile.age;
                let capture_view = captures_remote_weapon_view(networked, view_scope, projectile);
                let dt = match (&mut projectile.presentation_clock, server_tick) {
                    (Some(clock), Some(now)) => {
                        if crate::continuum::continuum_tick_reached(now, clock.expiry_tick) {
                            let expiry = clock.expiry_tick;
                            let last_live = crate::continuum::continuum_tick_sub(expiry, 1);
                            if capture_view
                                && self.door_clock_fresh
                                && let Some(initial) = initial_clock
                                && (1..=20).contains(&crate::continuum::continuum_tick_diff(
                                    last_live,
                                    initial.pose_tick,
                                ))
                                && self
                                    .remote_weapon_view_history
                                    .contains_projectile(projectile)
                            {
                                expiry_view_tails.push((*projectile, initial.pose_tick, last_live));
                            }
                            projectile.ttl = 0.0;
                            continue;
                        }
                        let signed_ticks =
                            crate::continuum::continuum_tick_diff(now, clock.pose_tick);
                        if signed_ticks < 0 {
                            clock.render_partial_seconds = 0.0;
                            continue;
                        }
                        let ticks = signed_ticks as u32;
                        if clock.motion_frozen {
                            let before = *projectile;
                            if capture_view && let Some(initial) = initial_clock {
                                record_weapon_view_step(
                                    &mut self.remote_weapon_view_history,
                                    &before,
                                    initial.pose_tick,
                                    initial_age,
                                    initial.expiry_tick,
                                    0.0,
                                );
                            }
                            let clock = projectile.presentation_clock.as_mut().unwrap();
                            clock.pose_tick = now;
                            clock.render_partial_seconds = 0.0;
                            projectile.age += crate::continuum::ticks_to_seconds_i32(signed_ticks);
                            projectile.ttl = crate::continuum::ticks_to_seconds_i32(
                                crate::continuum::continuum_tick_diff(clock.expiry_tick, now)
                                    .max(0),
                            );
                            if capture_view {
                                self.remote_weapon_view_history.record(projectile, now);
                            }
                            continue;
                        }
                        if ticks > 20 || ticks > recovery_steps_left {
                            // A long suspension cannot monopolize a frame or
                            // replay terrain against a stale door generation.
                            // Drop only this visual and recover a fresh scoped
                            // server pose; authority and hit readiness stay intact.
                            self.projectile_refresh_needed = true;
                            projectile.ttl = 0.0;
                            continue;
                        }
                        if self.networked_play && !self.door_clock_fresh {
                            clock.render_partial_seconds = 0.0;
                            continue;
                        }
                        recovery_steps_left -= ticks;
                        clock.pose_tick = now;
                        clock.render_partial_seconds = render_partial;
                        if ticks == 0 {
                            continue;
                        }
                        ticks as f32 / 100.0
                    }
                    (None, None) => dt,
                    _ => continue,
                };
                if capture_view && let Some(initial) = initial_clock {
                    record_weapon_view_step(
                        &mut self.remote_weapon_view_history,
                        projectile,
                        initial.pose_tick,
                        initial_age,
                        initial.expiry_tick,
                        0.0,
                    );
                }
                if let Some(clock) = projectile.presentation_clock {
                    projectile.ttl = crate::continuum::ticks_to_seconds_i32(
                        crate::continuum::continuum_tick_diff(clock.expiry_tick, clock.pose_tick)
                            .max(0),
                    ) + dt;
                }
                projectile.ttl -= dt;
                projectile.age += dt;
                projectile.trail_timer -= dt;
                let mut consumed_by_player_hit = false;
                let mut direct_explosion = false;
                let mut direct_explosion_pos = None;
                let mut stepped_player_hit_pos = None;
                let mut observe = |projectile: &RemoteProjectile, elapsed: f32| {
                    if capture_view && let Some(initial) = initial_clock {
                        record_weapon_view_step(
                            &mut self.remote_weapon_view_history,
                            projectile,
                            initial.pose_tick,
                            initial_age,
                            initial.expiry_tick,
                            elapsed,
                        );
                    }
                };

                match projectile.kind {
                    RemoteProjectileKind::Bullet { .. }
                    | RemoteProjectileKind::Shrapnel { .. }
                    | RemoteProjectileKind::Burst { .. } => {
                        let check_player_contact =
                            predict_player_contacts && projectile.frequency != player_frequency;
                        let (event, bounced_at) =
                            crate::gameplay::update_remote_bullet_motion_observed(
                                projectile,
                                dt,
                                map,
                                bricks,
                                player_pos,
                                player_radius,
                                check_player_contact,
                                &mut observe,
                            );
                        if let Some(bounced_at) = bounced_at {
                            bounce_sound =
                                nearest_sound_position(bounce_sound, player_pos, bounced_at);
                        }
                        match event {
                            Some(RemoteBulletMotionEvent::WallImpact(pos)) => {
                                impacts.push((pos, projectile.source.identity));
                                continue;
                            }
                            Some(RemoteBulletMotionEvent::TimedOut) => continue,
                            Some(RemoteBulletMotionEvent::PlayerHit(pos)) => {
                                stepped_player_hit_pos = Some(pos);
                            }
                            None => {}
                        }
                    }
                    RemoteProjectileKind::Bomb { .. } => {
                        if projectile.trail_timer <= 0.0 {
                            projectile.trail_timer = BOMB_TRAIL_INTERVAL;
                            if capture_view
                                && let Some(key) =
                                    crate::remote_weapon::RemoteWeaponViewKey::from_projectile(
                                        projectile,
                                    )
                                && let Some(clock) = projectile.presentation_clock
                            {
                                if self.remote_weapon_trails.len() < MAX_REMOTE_WEAPON_TRAILS {
                                    self.remote_weapon_trails.push_back(RemoteWeaponTrail {
                                        key: key.group_key(),
                                        source: projectile.source,
                                        born_tick: clock.pose_tick,
                                        position: projectile.pos - projectile.vel * 0.01,
                                        kind: RemoteWeaponTrailKind::Bomb,
                                        presented: false,
                                    });
                                } else {
                                    self.remote_weapon_view_effect_drops =
                                        self.remote_weapon_view_effect_drops.saturating_add(1);
                                }
                            } else {
                                self.bomb_trails.push(BombTrail {
                                    pos: projectile.pos - projectile.vel * 0.01,
                                    age: 0.0,
                                    remote_source: Some(projectile.source.identity),
                                });
                            }
                        }

                        let check_player_contact =
                            predict_player_contacts && projectile.frequency != player_frequency;
                        let (event, bounced_at) =
                            crate::gameplay::update_remote_bomb_motion_observed(
                                projectile,
                                dt,
                                map,
                                bricks,
                                player_pos,
                                player_radius,
                                self.tuning,
                                projectile.source.bomb_gravity,
                                check_player_contact,
                                &mut observe,
                            );

                        if let Some(bounced_at) = bounced_at {
                            bounce_sound =
                                nearest_sound_position(bounce_sound, player_pos, bounced_at);
                        }

                        match event {
                            Some(
                                event @ (BombMotionEvent::DirectHit {
                                    explosion_pos,
                                    damage_pos,
                                }
                                | BombMotionEvent::ProximityExplosion {
                                    explosion_pos,
                                    damage_pos,
                                }),
                            ) => {
                                direct_explosion = true;
                                direct_explosion_pos =
                                    Some((explosion_pos, damage_pos, event.spawns_shrapnel()));
                            }
                            Some(BombMotionEvent::WallExplosion(pos)) => {
                                explosions.push((
                                    pos,
                                    pos,
                                    projectile.source,
                                    projectile.weapon_data,
                                    projectile.damage,
                                    projectile.radius,
                                    projectile.emp_shutdown_time,
                                    projectile.shrapnel_rng_seed,
                                    true,
                                ));
                                projectile.ttl = 0.0;
                                continue;
                            }
                            None => {}
                        }
                    }
                    RemoteProjectileKind::Mine { .. } => {
                        let (event, _) = crate::gameplay::update_remote_bomb_motion_observed(
                            projectile,
                            dt,
                            map,
                            bricks,
                            player_pos,
                            player_radius,
                            self.tuning,
                            projectile.source.bomb_gravity,
                            false,
                            &mut observe,
                        );
                        if let Some(BombMotionEvent::WallExplosion(pos)) = event {
                            explosions.push((
                                pos,
                                pos,
                                projectile.source,
                                projectile.weapon_data,
                                projectile.damage,
                                projectile.radius,
                                projectile.emp_shutdown_time,
                                projectile.shrapnel_rng_seed,
                                true,
                            ));
                        }
                    }
                    RemoteProjectileKind::Thor => {}
                }

                if weapon_times_out_on_wormhole_tile(map, projectile.pos) {
                    projectile.ttl = 0.0;
                    continue;
                }

                if matches!(
                    projectile.kind,
                    RemoteProjectileKind::Bullet { .. } | RemoteProjectileKind::Shrapnel { .. }
                ) && projectile.trail_timer <= 0.0
                {
                    projectile.trail_timer = BULLET_TRAIL_INTERVAL;
                    let bouncing = matches!(
                        projectile.kind,
                        RemoteProjectileKind::Bullet { bouncing: true }
                            | RemoteProjectileKind::Shrapnel { bouncing: true }
                    );
                    if capture_view
                        && let Some(key) =
                            crate::remote_weapon::RemoteWeaponViewKey::from_projectile(projectile)
                        && let Some(clock) = projectile.presentation_clock
                    {
                        if self.remote_weapon_trails.len() < MAX_REMOTE_WEAPON_TRAILS {
                            self.remote_weapon_trails.push_back(RemoteWeaponTrail {
                                key: key.group_key(),
                                source: projectile.source,
                                born_tick: clock.pose_tick,
                                position: projectile.pos - projectile.vel * 0.01,
                                kind: RemoteWeaponTrailKind::Bullet {
                                    level: projectile.level,
                                    bouncing,
                                },
                                presented: false,
                            });
                        } else {
                            self.remote_weapon_view_effect_drops =
                                self.remote_weapon_view_effect_drops.saturating_add(1);
                        }
                    } else {
                        self.bullet_trails.push(BulletTrail {
                            pos: projectile.pos - projectile.vel * 0.01,
                            age: 0.0,
                            level: projectile.level,
                            bouncing,
                            remote_source: Some(projectile.source.identity),
                        });
                    }
                }

                match projectile.kind {
                    RemoteProjectileKind::Bullet { .. }
                    | RemoteProjectileKind::Shrapnel { .. }
                    | RemoteProjectileKind::Burst { .. } => {
                        if let Some(hit_pos) = stepped_player_hit_pos {
                            impacts.push((hit_pos, projectile.source.identity));
                            consumed_by_player_hit = true;
                            safe_zone_flag_drop |= safe_zone_weapon_hit_drops_flag(
                                player_safe,
                                self.carrying_flag,
                                true,
                            );
                            if !player_safe && !player_respawning {
                                let damage = remote_projectile_damage_after_protection(
                                    remote_projectile_contact_damage(projectile),
                                    projectile.weapon_data,
                                    self.tuning.exact_damage,
                                    self.ship.super_timer,
                                    self.ship.shield_timer,
                                    player_shield_lifetime,
                                    flagger_damage_multiplier,
                                    flagger_active,
                                );
                                if damage > 0.0 {
                                    remote_damage += damage;
                                    hit_by = Some(projectile.source.identity);
                                }
                            }
                        }
                    }
                    RemoteProjectileKind::Bomb { .. } => {}
                    RemoteProjectileKind::Thor => {
                        if projectile.ttl > 0.0 {
                            if predict_player_contacts && projectile.frequency != player_frequency {
                                match update_remote_thor_player_contact(
                                    projectile,
                                    dt,
                                    player_pos,
                                    player_radius,
                                    self.tuning,
                                ) {
                                    Some(event) => {
                                        direct_explosion = true;
                                        let pos = match event {
                                            RemoteExplosivePlayerEvent::DirectHit(pos)
                                            | RemoteExplosivePlayerEvent::ProximityExplosion(pos) => {
                                                pos
                                            }
                                        };
                                        direct_explosion_pos =
                                            Some((pos, pos, event.spawns_shrapnel()));
                                    }
                                    None => {}
                                }
                            } else {
                                let initial_position = projectile.pos;
                                projectile.pos += projectile.vel * dt;
                                if capture_view {
                                    let steps = (dt * 100.0).round().max(0.0) as u32;
                                    for step in 1..=steps {
                                        let elapsed = step as f32 / 100.0;
                                        let mut sample = *projectile;
                                        if step != steps {
                                            sample.pos = initial_position + sample.vel * elapsed;
                                        }
                                        observe(&sample, elapsed);
                                    }
                                }
                            }
                            if weapon_times_out_on_wormhole_tile(map, projectile.pos) {
                                projectile.ttl = 0.0;
                                continue;
                            }
                        }
                    }
                    RemoteProjectileKind::Mine { proximity } => {
                        direct_explosion = predict_player_contacts
                            && projectile.ttl > 0.0
                            && projectile.frequency != player_frequency
                            && if proximity {
                                let proximity_trigger = proximity_sensor_player_trigger(
                                    projectile.proximity_sensor,
                                    projectile.pos,
                                    0,
                                    player_pos,
                                    player_radius,
                                    proximity_bomb_trigger_radius(
                                        self.tuning.proximity_distance,
                                        projectile.level,
                                    ),
                                );
                                proximity_bomb_sensor_exploded(
                                    &mut projectile.proximity_sensor,
                                    proximity_trigger,
                                    projectile.age,
                                    dt,
                                    self.tuning.bomb_explode_delay_seconds,
                                )
                            } else {
                                explosive_contact_overlaps_ship(
                                    projectile.pos,
                                    player_pos,
                                    player_radius,
                                )
                            };
                    }
                }
                if direct_explosion {
                    let (pos, damage_pos, spawn_shrapnel) =
                        direct_explosion_pos.unwrap_or((projectile.pos, projectile.pos, true));
                    explosions.push((
                        pos,
                        damage_pos,
                        projectile.source,
                        projectile.weapon_data,
                        projectile.damage,
                        projectile.radius,
                        projectile.emp_shutdown_time,
                        projectile.shrapnel_rng_seed,
                        spawn_shrapnel,
                    ));
                    projectile.ttl = 0.0;
                    consumed_by_player_hit = true;
                }

                if consumed_by_player_hit {
                    continue;
                }

                let solid_hit = remote_projectile_uses_solid_cleanup(projectile.kind)
                    && is_solid_world_for_frequency(
                        map,
                        projectile.pos,
                        projectile.frequency,
                        bricks,
                    );
                let timed_out = projectile.ttl <= 0.0;
                if timed_out || solid_hit {
                    match projectile.kind {
                        RemoteProjectileKind::Bullet { .. }
                        | RemoteProjectileKind::Burst { .. } => {
                            if projectile_cleanup_creates_impact(solid_hit, timed_out) {
                                impacts.push((projectile.pos, projectile.source.identity));
                            }
                        }
                        RemoteProjectileKind::Shrapnel { .. } => {
                            if shrapnel_cleanup_creates_impact(solid_hit, timed_out, projectile.age)
                            {
                                impacts.push((projectile.pos, projectile.source.identity));
                            }
                        }
                        RemoteProjectileKind::Bomb { .. } | RemoteProjectileKind::Thor => {
                            if explosive_cleanup_creates_explosion(solid_hit, timed_out) {
                                explosions.push((
                                    projectile.pos,
                                    projectile.pos,
                                    projectile.source,
                                    projectile.weapon_data,
                                    projectile.damage,
                                    projectile.radius,
                                    projectile.emp_shutdown_time,
                                    projectile.shrapnel_rng_seed,
                                    true,
                                ));
                            }
                        }
                        RemoteProjectileKind::Mine { .. } => {}
                    }
                    projectile.ttl = 0.0;
                }
            }
        }

        if let Some(source_pos) = bounce_sound {
            if let Some(sound) = self.assets.bounce_sound.as_ref() {
                if let Some(volume) = self.positional_sound_volume(source_pos, 0.20) {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: false,
                            volume,
                        },
                    );
                }
            }
        }

        self.remote_projectiles
            .retain(|projectile| projectile.ttl > 0.0);

        for (pos, source) in impacts {
            self.add_remote_bullet_impact(pos, source);
        }
        let mut spawned_remote_shrapnel = Vec::new();
        for (
            pos,
            damage_pos,
            source,
            weapon_data,
            base_damage,
            radius,
            emp_shutdown_time,
            shrapnel_rng_seed,
            spawn_shrapnel,
        ) in explosions
        {
            let shooter_distance = Some(source.fire_position.distance(damage_pos));
            let mut shrapnel_projectiles = if spawn_shrapnel && !self.networked_play {
                remote_shrapnel_projectiles(
                    source,
                    weapon_data,
                    pos,
                    shrapnel_rng_seed,
                    &self.tuning,
                )
            } else {
                Vec::new()
            };
            {
                let map = &self.maps[self.map_index];
                let bricks = &self.bricks;
                shrapnel_projectiles.retain(|projectile| {
                    shrapnel_spawn_allowed(map, projectile.pos, projectile.frequency, bricks)
                });
            }
            // Network explosions and fragment fans arrive from the server's
            // committed transaction. Prediction must not draw a second fan
            // or a second explosion when that authenticated terminal arrives.
            if !self.networked_play {
                self.add_remote_visual_explosion(pos, emp_shutdown_time, source.identity);
            }
            spawned_remote_shrapnel.extend(shrapnel_projectiles);
            safe_zone_flag_drop |= safe_zone_weapon_hit_drops_flag(
                player_safe,
                self.carrying_flag,
                !player_respawning
                    && radial_damage(damage_pos.distance(player_pos), radius, base_damage) > 0.0,
            );
            if !player_safe && !player_respawning {
                let raw_radial = radial_damage_with_shooter_proximity_reduction(
                    damage_pos.distance(player_pos),
                    shooter_distance,
                    radius,
                    base_damage,
                );
                let emp_seconds = emp_shutdown_seconds_from_raw_damage(
                    emp_shutdown_time,
                    raw_radial,
                    base_damage,
                );
                self.ship.emp_shutdown_timer = self.ship.emp_shutdown_timer.max(emp_seconds);
                let damage = remote_projectile_damage_after_protection(
                    raw_radial,
                    weapon_data,
                    self.tuning.exact_damage,
                    self.ship.super_timer,
                    self.ship.shield_timer,
                    player_shield_lifetime,
                    flagger_damage_multiplier,
                    flagger_active,
                );
                if damage > 0.0 {
                    remote_damage += damage;
                    hit_by = Some(source.identity);
                }
            }
        }
        self.remote_projectiles.extend(spawned_remote_shrapnel);

        // Live simulation stays expired. Fill only the final bounded historical
        // flight of an already observed weapon, using the remaining recovery
        // budget and no damage/effects. Late expired births never enter here.
        for (mut projectile, start_tick, last_live_tick) in expiry_view_tails {
            let elapsed = crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(last_live_tick, start_tick),
            );
            self.catch_up_remote_projectile_presentation_from_tick(
                &mut projectile,
                elapsed,
                Some(start_tick),
            );
        }

        if safe_zone_flag_drop {
            self.drop_flags_after_safe_zone_weapon_hit();
        }

        if remote_damage > 0.0 && !self.networked_play {
            let fatal = player_damage_is_fatal(self.ship.energy, remote_damage);
            if !fatal {
                self.apply_damage_prize_loss(remote_damage);
            }
            self.ship.energy -= remote_damage;
            if fatal {
                let death_pos = self.ship.pos;
                self.player_deaths += 1;
                let defeated_bounty = self.player_bounty;
                self.drop_carried_flag_at(death_pos);
                // A remote visual is not authoritative damage in networked
                // play, and an offline roster cannot prove the source
                // session. Never attribute a historical projectile to a
                // newer occupant of the same numeric player id.
                let killer_label =
                    hit_by.map(|identity| format!("Player {}", identity.player_id()));
                self.push_notification(
                    &player_destroyed_notification(
                        self.local_player_name(),
                        defeated_bounty,
                        killer_label.as_deref(),
                    ),
                    Color::from_rgba(255, 120, 90, 255),
                );
                self.start_player_respawn(death_pos);
            }
        }
    }
}

#[cfg(test)]
mod tests;
