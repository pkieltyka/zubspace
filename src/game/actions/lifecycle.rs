use super::*;

fn wrapped_map_index(current: usize, count: usize, direction: i32) -> usize {
    if count == 0 {
        return 0;
    }

    (current as i32 + direction).rem_euclid(count as i32) as usize
}

fn zone_key(path: &str) -> &str {
    zone_directory(path).unwrap_or(path)
}

fn zone_label(path: &str) -> &str {
    zone_key(path).rsplit('/').next().unwrap_or(path)
}

pub(in crate::game) fn zone_settings_path_for_map<'a>(
    map_name: &str,
    settings_paths: &'a [&'a str],
) -> Option<&'a str> {
    let map_zone = zone_directory(map_name)?;
    settings_paths
        .iter()
        .copied()
        .find(|settings_path| zone_directory(settings_path) == Some(map_zone))
}

fn zone_start_index_for_names(names: &[&str], current: usize, direction: i32) -> usize {
    if names.is_empty() {
        return 0;
    }

    let mut zones = Vec::new();
    let mut first_indices = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let zone = zone_key(name);
        if !zones.contains(&zone) {
            zones.push(zone);
            first_indices.push(index);
        }
    }

    let current_name = names[current.min(names.len() - 1)];
    let current_zone = zone_key(current_name);
    let current_zone_index = zones
        .iter()
        .position(|zone| *zone == current_zone)
        .unwrap_or(0);
    let next_zone_index = wrapped_map_index(current_zone_index, zones.len(), direction);
    first_indices[next_zone_index]
}

fn zone_start_index_for_maps(maps: &[ArenaMap], current: usize, direction: i32) -> usize {
    let names = maps.iter().map(|map| map.name.as_str()).collect::<Vec<_>>();
    zone_start_index_for_names(&names, current, direction)
}

pub(in crate::game) fn merge_local_zone_lvz(
    render_cache: &mut LvzRenderCache,
    archives: &[(String, LvzArchive)],
    map_name: &str,
) {
    let Some(map_zone) = zone_directory(map_name) else {
        return;
    };

    for (path, archive) in archives {
        if zone_directory(path.as_str()) == Some(map_zone) {
            render_cache
                .merge_archive_deferred(archive)
                .expect("embedded LVZ was validated during asset loading");
        }
    }
    // One whole-cache resolve/band pass for the zone's archives instead of one per archive: banding
    // walks every object, so the per-archive form was O(archives x objects) on this startup/switch
    // path (fourteen Chaos archives), the measured 112-116 ms worst single merge.
    render_cache.finalize_deferred_merges();
}

pub(in crate::game) fn merge_local_zone_lvz_sounds(
    sounds: &mut BTreeMap<u8, Sound>,
    archive_sounds: &[(String, u8, Sound)],
    map_name: &str,
) {
    let Some(map_zone) = zone_directory(map_name) else {
        return;
    };

    for (path, sound_id, sound) in archive_sounds {
        if zone_directory(path.as_str()) == Some(map_zone) {
            sounds.entry(*sound_id).or_insert_with(|| sound.clone());
        }
    }
}

pub(in crate::game) fn local_zone_tuning(
    tunings: &[(String, ArenaTuning)],
    map_name: &str,
) -> Option<ArenaTuning> {
    let map_zone = zone_directory(map_name)?;
    tunings
        .iter()
        .find(|(path, _)| zone_directory(path.as_str()) == Some(map_zone))
        .map(|(_, tuning)| *tuning)
}

fn zone_directory(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(directory, _)| directory)
}

impl Game {
    pub(in crate::game) fn reset_ship(&mut self) {
        self.reset_ship_state(true);
    }

    pub(in crate::game) fn reset_ship_after_death(&mut self) {
        self.reset_ship_state(false);
        if self.networked_play && self.server_maxed_spawn_loadout == Some(true) {
            // Respawn submits its neutral packet before the server commits a
            // successor life. Use the explicit entry policy, never previously
            // earned upgrades, to match that transaction's canonical energy.
            self.ship.max_energy = self.ship_tuning().energy_max;
            self.ship.energy = self.ship.max_energy;
            self.last_self_server_energy = Some(self.ship.energy);
        }
    }

    fn reset_ship_state(&mut self, clear_owned_weapons: bool) {
        self.require_forward_self_server_rocket_life =
            self.last_self_server_rocket_life_seq.is_some();
        let tuning = self.ship_tuning();
        self.player_respawn_timer = 0.0;
        self.ship.pos = self.player_spawn_position();
        self.ship.vel = Vec2::ZERO;
        self.ship.orientation = 0.0;
        self.ship.flash = false;
        self.ship.max_energy = tuning.energy_initial.min(tuning.energy_max);
        self.ship.energy = self.ship.max_energy;
        self.last_self_server_energy = Some(self.ship.energy);
        self.ship.stealth_capable = tuning.initial_stealth;
        self.ship.stealth = false;
        self.ship.cloak_capable = tuning.initial_cloak;
        self.ship.cloak = false;
        self.ship.xradar_capable = tuning.initial_xradar;
        self.ship.xradar = false;
        self.ship.antiwarp_capable = tuning.initial_antiwarp;
        self.ship.antiwarp = false;
        self.ship.multifire_capable = false;
        self.ship.multifire = false;
        self.ship.proximity_bombs = false;
        // Reset to default-off on ship reset / respawn (matching
        // every other capability prize). Was `true` previously,
        // which made warbird default-fire bouncing bullets.
        self.ship.bouncing_bullets = false;
        self.ship.super_timer = 0.0;
        self.ship.shield_timer = 0.0;
        self.ship.engine_shutdown_timer = 0.0;
        self.ship.emp_shutdown_timer = 0.0;
        self.ship.repel_timer = 0.0;
        self.ship.fake_antiwarp_timer = 0.0;
        self.ship.repels = tuning.initial_repels.min(tuning.max_repels);
        self.ship.bursts = tuning.initial_bursts.min(tuning.max_bursts);
        self.ship.bricks = tuning.initial_bricks.min(tuning.max_bricks);
        self.ship.decoys = tuning.initial_decoys.min(tuning.max_decoys);
        self.ship.thors = tuning.initial_thors.min(tuning.max_thors);
        self.ship.rockets = tuning.initial_rockets.min(tuning.max_rockets);
        self.ship.portals = tuning.initial_portals.min(tuning.max_portals);
        self.ship.gun_level = tuning.initial_gun_level.min(tuning.max_gun_level);
        self.ship.bomb_level = tuning.initial_bomb_level.min(tuning.max_bomb_level);
        self.ship.rotation_per_second = tuning
            .rotation_per_second
            .min(tuning.rotation_max_per_second);
        self.ship.thrust_acceleration = tuning
            .thrust_acceleration
            .min(tuning.thrust_max_acceleration);
        self.ship.top_speed = tuning.top_speed.min(tuning.top_speed_max);
        self.ship.recharge_per_second = tuning.energy_recharge.min(tuning.energy_recharge_max);
        self.ship.shrapnel_upgrades = 0;
        self.apply_initial_bounty_prizes();
        self.ship.repel_cooldown = 0.0;
        self.ship.wormhole_cooldown = WORMHOLE_COOLDOWN;
        self.ship.rocket_timer = 0.0;
        self.ship.rocket_cooldown = 0.0;
        self.pending_rocket_activation_sequence = None;
        self.ship.fake_antiwarp_timer = 0.0;
        self.pending_weapon = 0;
        self.cancel_pending_weapon_prediction();
        self.carrying_flag = false;
        self.flag_respawn_timer = 0.0;
        self.flag_carry_timer = 0.0;
        self.portal = None;
        self.portal_endpoint_authority = None;
        if clear_owned_weapons {
            self.clear_local_weapons();
            self.explosions
                .retain(|effect| effect.remote_source.is_some());
        }
    }

    pub(in crate::game) fn apply_initial_bounty_prizes(&mut self) {
        let tuning = self.ship_tuning();
        self.player_bounty = 0;
        for kind in initial_bounty_prizes(
            &self.tuning.prize_weights,
            self.tuning.prize_weight_total,
            tuning.initial_bounty,
            self.prize_rng_seed,
        ) {
            self.apply_prize_effect(kind, false, false);
        }
        self.player_bounty = tuning.initial_bounty;
    }

    pub(in crate::game) fn clear_active_weapons(&mut self) {
        self.clear_local_weapons();
        self.bot_bullets.clear();
        self.bullet_trails.clear();
        self.bullet_impacts.clear();
        self.bomb_flashes.clear();
        self.bomb_trails.clear();
        self.decoys.clear();
        self.repels.clear();
        self.burst_effects.clear();
    }

    pub(in crate::game) fn clear_local_weapons(&mut self) {
        self.predicted_explosion_receipts.clear();
        let scope = self.client_authority.view().scope;
        self.owned_projectile_life_floor = scope
            .server_session_id
            .zip(scope.arena_stream_id)
            .zip(scope.player_id)
            .zip(scope.life_seq)
            .map(|(((server, arena), player), life)| (server, arena, player, life));
        self.remote_projectiles
            .retain(|projectile| Some(projectile.source.identity.player_id()) != scope.player_id);
        self.bullets.clear();
        self.bullet_trails
            .retain(|effect| effect.remote_source.is_some());
        self.bullet_impacts
            .retain(|effect| effect.remote_source.is_some());
        self.powerball.carried = false;
        self.bombs.clear();
        self.bomb_flashes
            .retain(|effect| effect.remote_source.is_some());
        self.bomb_trails
            .retain(|effect| effect.remote_source.is_some());
        self.mines.clear();
        self.shrapnel.clear();
        self.thors.clear();
        self.decoys.retain(|effect| effect.remote_source.is_some());
        self.repels.retain(|effect| effect.remote_source.is_some());
        self.burst_effects
            .retain(|effect| effect.remote_source.is_some());
        self.exhausts.clear();
        self.rocket_particles.clear();
    }

    pub(in crate::game) fn clear_bricks(&mut self) {
        let map = &mut self.maps[self.map_index];
        for brick in self.bricks.drain(..) {
            map.set_tile(brick.tile.x, brick.tile.y, brick.previous_tile);
        }
    }

    pub(in crate::game) fn change_map(&mut self, direction: i32) {
        self.clear_bricks();
        let index = wrapped_map_index(self.map_index, self.maps.len(), direction);
        self.switch_to_map(index);
    }

    pub(in crate::game) fn change_zone(&mut self, direction: i32) {
        self.clear_bricks();
        let index = zone_start_index_for_maps(&self.maps, self.map_index, direction);
        self.switch_to_map(index);
        let message = format!("Zone: {}", zone_label(&self.maps[self.map_index].name));
        self.push_notification(&message, Color::from_rgba(120, 220, 255, 255));
    }

    pub(in crate::game) fn switch_to_map(&mut self, index: usize) {
        self.clear_active_weapons();
        self.explosions.clear();
        self.ship_explosions.clear();
        self.warps.clear();
        // Collectible IDs are scoped to an authoritative arena/session. A map
        // or zone transition establishes a new arena scope; ordinary death
        // and respawn intentionally leave this history intact.
        self.applied_collectible_acks.clear();
        let _ = self.client_authority.cancel_correction();
        self.pending_same_life_movement_boundary_position = false;
        self.clear_prize_effect_application_state();
        self.pending_warp_relocation_collectible_id = None;
        self.pending_repel_application = None;
        self.pending_repel_application_expiry_tick = None;
        self.expired_repel_applications.clear();
        self.consumed_repel_delivery_generations.clear();
        self.client_authority.clear_deferred_warp_pickup_acks();
        self.repel_movement_sequence_window = None;
        self.pending_warp_relocation_life_seq = None;
        self.pending_warp_relocation_sequences.clear();
        self.network_security_challenge = None;
        self.network_security_challenge_generation = None;
        self.network_security_generation = None;
        self.network_security_synchronized = false;
        self.outbound_input_ring.clear();
        self.map_index = index.min(self.maps.len().saturating_sub(1));
        if self.uses_local_zone_tuning() {
            self.tuning =
                local_zone_tuning(&self.assets.zone_tunings, &self.maps[self.map_index].name)
                    .unwrap_or_else(ArenaTuning::continuum_skirmish_default);
        }
        self.lvz_render_cache = LvzRenderCache::default();
        merge_local_zone_lvz(
            &mut self.lvz_render_cache,
            &self.assets.lvz_archives,
            &self.maps[self.map_index].name,
        );
        self.lvz_chat_sounds.clear();
        merge_local_zone_lvz_sounds(
            &mut self.lvz_chat_sounds,
            &self.assets.lvz_chat_sounds,
            &self.maps[self.map_index].name,
        );
        self.door_timer = 0.0;
        self.door_clock_server_tick = None;
        self.door_clock_observed_server_tick = None;
        self.door_clock_fresh = false;
        self.flag_score = 0;
        self.bot_flag_score = 0;
        self.ball_score = 0;
        self.bot_ball_score = 0;
        self.bot_score = 0;
        self.player_deaths = 0;
        // Arena-scoped objects survive ship changes and respawns, but not an
        // actual map/zone transition. Keeping this cleanup out of reset_ship
        // also preserves server-owned greens long enough for the selected
        // ship's newly reset runtime to claim them normally.
        self.prizes.clear();
        self.clear_bricks();
        self.prize_spawn_timer = 0.0;
        self.reset_ship();
        self.dropped_flag = None;
        self.reset_powerball();
        self.bots = if self.uses_local_bots() {
            spawn_initial_bots(&self.maps[self.map_index], self.ship.pos, self.tuning)
        } else {
            Vec::new()
        };
        self.bot_bullets.clear();
    }

    fn uses_local_bots(&self) -> bool {
        !self.networked_play
    }

    fn uses_local_zone_tuning(&self) -> bool {
        !self.networked_play
    }
}

#[cfg(test)]
mod tests;
