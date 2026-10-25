use super::*;

impl Game {
    pub(in crate::game) fn try_wormhole_warp(&mut self) -> bool {
        if self.ship.wormhole_cooldown > 0.0
            || self.map().tile_at_world(self.ship.pos) != WORMHOLE_TILE_ID
        {
            return false;
        }

        let from = self.ship.pos;
        self.warps.push(WarpEffect {
            pos: from,
            age: 0.0,
        });

        self.ship.pos = self.player_spawn_position();
        self.ship.vel = Vec2::ZERO;
        self.ship.energy = crate::gameplay_rules::wormhole_energy_after(self.ship.energy);
        self.ship.wormhole_cooldown = WORMHOLE_COOLDOWN;
        self.ship.flash = true;

        self.warps.push(WarpEffect {
            pos: self.ship.pos,
            age: 0.0,
        });

        #[cfg(not(test))]
        let warp_sound = self.assets.warp_sound.as_ref();
        #[cfg(test)]
        let warp_sound = self
            .assets
            .0
            .as_ref()
            .and_then(|assets| assets.warp_sound.as_ref());
        if let Some(sound) = warp_sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.36),
                },
            );
        }

        true
    }

    pub(in crate::game) fn place_portal(&mut self, position_sequence: u16) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = position_sequence;
        if !portal_place_available(self.ship.portals, self.ship.fake_antiwarp_timer) {
            return;
        }
        // The reference game's `PlayerManager::IsAntiwarped` blocks portal placement when
        // an enemy with antiwarp status is in range.
        if is_antiwarped_by_enemy(
            &self.network_world,
            self.ship.pos,
            self.player_frequency(),
            self.tuning.antiwarp_radius,
        ) {
            return;
        }
        if self.networked_play {
            #[cfg(target_arch = "wasm32")]
            if !self.try_send_network_portal_placement(position_sequence) {
                return;
            }
            #[cfg(not(target_arch = "wasm32"))]
            return;
        }
        self.ship.portals -= 1;

        self.portal = Some(Portal {
            pos: self.ship.pos,
            ttl: self.tuning.portal_lifetime,
            age: 0.0,
            server_action_id: None,
            server_placed_at_tick: None,
            server_expires_at_tick: None,
        });

        if let Some(sound) = self.assets.portal_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.34),
                },
            );
        }
    }

    pub(in crate::game) fn activate_warp_prize(&mut self) {
        self.reconcile_authoritative_portal();
        let from = self.ship.pos;
        let destination = warp_prize_destination(
            self.player_spawn_position(),
            self.active_portal().map(|portal| portal.pos),
        );

        self.warps.push(WarpEffect {
            pos: from,
            age: 0.0,
        });

        self.ship.pos = destination;
        self.ship.vel = warp_prize_velocity_after(self.ship.vel);
        self.ship.flash = true;
        if self.map().tile_at_world(destination) != WORMHOLE_TILE_ID {
            self.ship.wormhole_cooldown = WORMHOLE_COOLDOWN;
        }

        self.warps.push(WarpEffect {
            pos: self.ship.pos,
            age: 0.0,
        });

        #[cfg(not(test))]
        let warp_sound = self.assets.warp_sound.as_ref();
        #[cfg(test)]
        let warp_sound = self
            .assets
            .0
            .as_ref()
            .and_then(|assets| assets.warp_sound.as_ref());
        if let Some(sound) = warp_sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.36),
                },
            );
        }
    }

    pub(in crate::game) fn activate_warp(
        &mut self,
        drain_without_portal: bool,
        position_sequence: u16,
    ) {
        self.activate_warp_mode(drain_without_portal, position_sequence, false);
    }

    pub(in crate::game) fn activate_warp_mode(
        &mut self,
        drain_without_portal: bool,
        position_sequence: u16,
        force_random: bool,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = position_sequence;
        self.reconcile_authoritative_portal();
        let has_portal = !force_random && self.active_portal().is_some();
        if !warp_available(
            has_portal,
            self.ship.energy,
            self.ship.max_energy,
            self.ship.fake_antiwarp_timer,
            self.ship.bomb_cooldown,
        ) {
            return;
        }
        // The reference game blocks warp when an enemy's antiwarp aura covers the ship.
        if is_antiwarped_by_enemy(
            &self.network_world,
            self.ship.pos,
            self.player_frequency(),
            self.tuning.antiwarp_radius,
        ) {
            return;
        }

        if self.networked_play {
            #[cfg(target_arch = "wasm32")]
            self.try_send_network_portal_warp(position_sequence, force_random);
            return;
        }

        let from = self.ship.pos;
        let portal = if force_random {
            None
        } else {
            self.portal.take()
        };
        let used_portal = portal.is_some();
        let destination = portal
            .map(|portal| portal.pos)
            .unwrap_or_else(|| self.player_spawn_position());

        self.warps.push(WarpEffect {
            pos: from,
            age: 0.0,
        });

        self.ship.pos = destination;
        self.ship.vel = warp_velocity_after(self.ship.vel, used_portal);
        self.ship.flash = true;
        self.ship.bomb_cooldown =
            manual_warp_bomb_cooldown_after(self.ship.bomb_cooldown, REPEL_REUSE_COOLDOWN);
        if self.map().tile_at_world(destination) != WORMHOLE_TILE_ID {
            self.ship.wormhole_cooldown = WORMHOLE_COOLDOWN;
        }

        self.ship.energy = warp_energy_after(
            self.ship.energy,
            drain_without_portal,
            used_portal,
            from,
            destination,
        );
        self.ship.fake_antiwarp_timer = self.tuning.antiwarp_settle_seconds;

        self.warps.push(WarpEffect {
            pos: self.ship.pos,
            age: 0.0,
        });

        if let Some(sound) = self.assets.warp_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.36),
                },
            );
        }
    }
}
