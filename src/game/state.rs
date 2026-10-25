use super::*;

impl Game {
    pub(in crate::game) fn map(&self) -> &ArenaMap {
        &self.maps[self.map_index]
    }

    pub(in crate::game) fn ship_tuning(&self) -> ShipTuning {
        self.tuning.ship(self.ship.ship_type)
    }

    pub(in crate::game) fn player_frequency(&self) -> u16 {
        self.client_authority
            .view()
            .scope
            .player_id
            .and_then(|player_id| {
                self.network_world
                    .players
                    .get(&player_id)
                    .map(|player| player.frequency)
            })
            .unwrap_or(0)
    }

    pub(in crate::game) fn local_player_name(&self) -> &str {
        self.client_authority
            .view()
            .scope
            .player_id
            .and_then(|player_id| self.network_world.players.get(&player_id))
            .map(|player| player.name.as_str())
            .unwrap_or(&self.local_player_name)
    }

    pub(in crate::game) fn viewer_frequency(&self) -> u16 {
        let spectated_frequency = self
            .spectating_player_id
            .and_then(|player_id| self.network_world.players.get(&player_id))
            .map(|player| player.frequency);
        viewer_frequency_after_spectate(
            self.player_frequency(),
            self.spectator_mode,
            spectated_frequency,
        )
    }

    pub(in crate::game) fn visibility_ship_tuning(&self) -> ShipTuning {
        let spectated_ship = self
            .spectating_player_id
            .and_then(|player_id| self.network_world.players.get(&player_id))
            .map(|player| player.ship);
        self.tuning.ship(visibility_ship_type_after_spectate(
            self.ship.ship_type,
            self.spectator_mode,
            spectated_ship,
        ))
    }

    pub(in crate::game) fn flags_visible_to_local_viewer(&self) -> bool {
        flags_visible_to_viewer(
            self.spectator_mode,
            self.tuning.hide_flags_from_spectators,
            (get_time() as f32 - self.arena_enter_time).max(0.0),
            self.tuning.enter_game_flagging_delay_seconds,
        )
    }

    pub(in crate::game) fn player_spawn_position(&self) -> Vec2 {
        if !self.networked_play {
            return self.map().spawn_position();
        }
        // Match the server's immutable ZoneMap. Only installed overlays are
        // substituted; authored tiles with the same ID remain solid.
        let underlying = self
            .bricks
            .iter()
            .map(|brick| ((brick.tile.x, brick.tile.y), brick.previous_tile))
            .collect::<std::collections::HashMap<_, _>>();
        self.map().static_spawn_position(|x, y| {
            underlying
                .get(&(x, y))
                .copied()
                .unwrap_or_else(|| self.map().tile(x, y))
        })
    }

    pub(in crate::game) fn visible_hud_energy(&self) -> f32 {
        visible_hud_energy_value(
            self.ship.energy,
            self.ship.max_energy,
            self.last_self_server_energy,
        )
    }

    pub(in crate::game) fn effective_rotation_per_second(&self) -> f32 {
        self.ship.rotation_per_second
    }

    pub(in crate::game) fn effective_thrust_acceleration(&self) -> f32 {
        self.ship.thrust_acceleration
    }

    pub(in crate::game) fn effective_top_speed(&self) -> f32 {
        self.ship.top_speed
    }

    pub(in crate::game) fn flagger_active(&self) -> bool {
        self.carrying_flag || (self.powerball.carried && self.tuning.use_flagger_for_ball)
    }

    pub(in crate::game) fn effective_gun_level(&self) -> u8 {
        crate::gameplay_rules::effective_weapon_level(
            self.ship.gun_level,
            self.tuning.flagger_gun_upgrade && self.flagger_active(),
            WEAPON_LEVEL_MAX,
        )
    }

    pub(in crate::game) fn effective_bomb_level(&self) -> u8 {
        crate::gameplay_rules::effective_weapon_level(
            self.ship.bomb_level,
            self.tuning.flagger_bomb_upgrade && self.flagger_active(),
            WEAPON_LEVEL_MAX,
        )
    }

    pub(in crate::game) fn effective_fire_energy_cost(
        &self,
        base_cost: f32,
    ) -> crate::gameplay_rules::FireEnergyCost {
        crate::gameplay_rules::effective_fire_energy_cost(
            base_cost,
            crate::gameplay_rules::FireEnergyStatus {
                flagger_active: self.flagger_active(),
                flagger_cost_multiplier: self.tuning.flagger_fire_cost_multiplier,
                super_active: self.ship.super_timer > 0.0,
            },
        )
    }

    pub(in crate::game) fn effective_bullet_energy_cost(
        &self,
        tuning: ShipTuning,
    ) -> crate::gameplay_rules::FireEnergyCost {
        self.effective_fire_energy_cost(leveled_gun_energy_cost(
            tuning.bullet_energy,
            self.effective_gun_level(),
        ))
    }

    pub(in crate::game) fn effective_multifire_energy_cost(
        &self,
        tuning: ShipTuning,
    ) -> crate::gameplay_rules::FireEnergyCost {
        self.effective_fire_energy_cost(leveled_gun_energy_cost(
            tuning.multifire_energy,
            self.effective_gun_level(),
        ))
    }

    pub(in crate::game) fn effective_bomb_energy_cost(
        &self,
        tuning: ShipTuning,
    ) -> crate::gameplay_rules::FireEnergyCost {
        self.effective_fire_energy_cost(leveled_weapon_energy_cost(
            tuning.bomb_energy,
            tuning.bomb_energy_upgrade,
            self.effective_bomb_level(),
        ))
    }

    pub(in crate::game) fn effective_mine_energy_cost(
        &self,
        tuning: ShipTuning,
    ) -> crate::gameplay_rules::FireEnergyCost {
        self.effective_fire_energy_cost(leveled_weapon_energy_cost(
            tuning.mine_energy,
            tuning.mine_energy_upgrade,
            self.effective_bomb_level(),
        ))
    }

    pub(in crate::game) fn local_ship_has_attached_turrets(&self) -> bool {
        self.client_authority
            .view()
            .scope
            .player_id
            .map(|self_id| {
                self.network_world
                    .visible_attached_players(self_id)
                    .next()
                    .is_some()
            })
            .unwrap_or(false)
    }

    pub(in crate::game) fn local_ship_is_attached_to_turret(&self) -> bool {
        self.client_authority
            .view()
            .scope
            .player_id
            .and_then(|self_id| self.network_world.players.get(&self_id))
            .is_some_and(|player| player.attach_parent != INVALID_PLAYER_ID)
    }

    pub(in crate::game) fn effective_bomb_cooldown(&self, base_cooldown: f32) -> f32 {
        flagger_bomb_cooldown(
            base_cooldown,
            self.tuning.flagger_bomb_fire_delay_seconds,
            self.flagger_active(),
        )
    }

    pub(in crate::game) fn effective_energy_recharge(&self) -> f32 {
        self.ship.recharge_per_second
    }
}

fn visible_hud_energy_value(
    predicted_energy: f32,
    max_energy: f32,
    server_energy: Option<f32>,
) -> f32 {
    // Fully live: the HUD shows the live predicted energy clamped down to the
    // latest server-authoritative self energy (so prediction can't outrun the
    // server). There is no damage-result "hold" — energy drops on a hit and
    // recharges immediately, matching the server. The server value already
    // streams every snapshot, so this costs nothing extra.
    let mut energy = predicted_energy;
    if let Some(server_energy) = server_energy {
        energy = energy.min(server_energy);
    }
    // A negative Energy prize can leave the authoritative live pool above
    // its newly reduced capacity. The gauge still clamps its fill ratio, but
    // the numeric HUD and position stream must retain that exact value.
    let _ = max_energy;
    energy.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_hud_energy_uses_prediction_without_server_authority() {
        assert_eq!(visible_hud_energy_value(1600.0, 1700.0, None), 1600.0);
    }

    #[test]
    fn visible_hud_energy_clamps_to_latest_server_self_energy() {
        assert_eq!(
            visible_hud_energy_value(1700.0, 1700.0, Some(1694.0)),
            1694.0
        );
    }

    #[test]
    fn visible_hud_energy_is_fully_live_tracking_server_not_a_damage_hold() {
        // After a hit the HUD follows the live server energy as it recharges,
        // rather than freezing at the post-hit value. With predicted 1320 and
        // server already recharged to 1318, the bar reads 1318 (live), not a
        // held 1300.
        assert_eq!(
            visible_hud_energy_value(1320.0, 1700.0, Some(1318.0)),
            1318.0
        );
    }

    #[test]
    fn visible_hud_energy_preserves_authoritative_transient_above_capacity() {
        assert_eq!(
            visible_hud_energy_value(1800.0, 1700.0, Some(1800.0)),
            1800.0
        );
    }
}
