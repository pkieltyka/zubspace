use super::*;

impl Game {
    pub(super) fn update_flags(&mut self, dt: f32) {
        if self.networked_play {
            // Multiplayer flags are presentation-only NetworkWorld entities.
            // Map tiles and local carry/drop simulation cannot create them.
            self.carrying_flag =
                self.client_authority
                    .view()
                    .scope
                    .player_id
                    .is_some_and(|player_id| {
                        self.network_world
                            .flags
                            .values()
                            .any(|flag| flag.carried_by == Some(player_id))
                    });
            self.dropped_flag = None;
            self.flag_carry_timer = 0.0;
            self.flag_respawn_timer = 0.0;
            for bot in &mut self.bots {
                bot.carrying_flag = false;
            }
            let eligible = local_player_can_attempt_network_flag_claim(
                self.spectator_mode,
                self.ship.ship_type,
                self.player_respawn_timer <= 0.0,
            );
            if eligible {
                self.flag_claim_eligible_frames = self.flag_claim_eligible_frames.wrapping_add(1);
            }
            if let Some(flag_id) = eligible
                .then(|| {
                    self.network_world.pickable_flag_id_near(
                        self.ship.pos,
                        FLAG_PICKUP_RADIUS,
                        self.viewer_frequency(),
                    )
                })
                .flatten()
            {
                self.flag_claim_attempts = self.flag_claim_attempts.wrapping_add(1);
                self.send_network_flag_request(flag_id);
            }
            return;
        }
        self.flag_respawn_timer -= dt;
        let bot_carrying_flag = self.bots.iter().any(|bot| bot.carrying_flag);
        let mut expired_drop = false;
        if let Some(flag) = &mut self.dropped_flag {
            flag.age += dt;
            flag.ttl -= dt;
            expired_drop = flag.ttl <= 0.0;
        }
        if expired_drop {
            self.dropped_flag = None;
            self.push_notification(
                "Dropped flag returned.",
                Color::from_rgba(185, 205, 225, 255),
            );
        }

        if self.carrying_flag {
            if self
                .map()
                .is_touching_object(self.ship.pos, GOAL_TILE_ID, FLAG_PICKUP_RADIUS)
            {
                self.carrying_flag = false;
                self.flag_carry_timer = 0.0;
                self.dropped_flag = None;
                for bot in &mut self.bots {
                    bot.carrying_flag = false;
                }
                self.flag_score += 1;
                self.flag_respawn_timer = FLAG_RESPAWN_DELAY;
                self.push_notification("Goal!", Color::from_rgba(255, 220, 95, 255));

                if let Some(sound) = self.assets.goal_sound.as_ref() {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: false,
                            volume: self.effect_volume(0.38),
                        },
                    );
                }
            }
            if self.flag_carry_timer > 0.0 {
                self.flag_carry_timer -= dt;
                if self.flag_carry_timer <= 0.0 {
                    self.send_network_drop_flags();
                    self.drop_carried_flag_at(self.ship.pos);
                }
            }
            return;
        }

        let bot_goal = {
            let map = &self.maps[self.map_index];
            let mut bot_goal = None;
            for bot in self
                .bots
                .iter_mut()
                .filter(|bot| bot.respawn_timer <= 0.0 && bot.carrying_flag)
            {
                if bot_touches_map_object(bot.pos, map, GOAL_TILE_ID) {
                    bot.carrying_flag = false;
                    bot.flag_goals += 1;
                    bot_goal = Some(bot.id);
                    break;
                }
            }
            bot_goal
        };
        if let Some(id) = bot_goal {
            self.bot_flag_score += 1;
            self.dropped_flag = None;
            self.flag_respawn_timer = FLAG_RESPAWN_DELAY;
            self.push_notification(
                &format!("Bot {} flag goal!", id + 1),
                Color::from_rgba(255, 190, 95, 255),
            );

            if let Some(sound) = self.assets.goal_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.34),
                    },
                );
            }
            return;
        }

        let player_can_pickup_flag = self.can_pickup_carryable_flag() && !self.in_safe_zone();

        let dropped_pos = self.dropped_flag.as_ref().map(|flag| flag.pos);
        if let Some(pos) = dropped_pos {
            if player_can_pickup_flag && self.ship.pos.distance(pos) <= FLAG_PICKUP_RADIUS {
                self.dropped_flag = None;
                self.carrying_flag = true;
                self.flag_carry_timer = self.tuning.flag_drop_delay_seconds;
                self.send_network_flag_request(0);
                self.push_notification(
                    "Dropped flag picked up.",
                    Color::from_rgba(125, 230, 150, 255),
                );

                if let Some(sound) = self.assets.flag_sound.as_ref() {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: false,
                            volume: self.effect_volume(0.32),
                        },
                    );
                }
            }
            if self.tuning.carry_flags > 0 && !self.carrying_flag {
                if let Some(id) = bot_touching_pos(&self.bots, pos, FLAG_PICKUP_RADIUS) {
                    if let Some(bot) = self.bots.iter_mut().find(|bot| bot.id == id) {
                        bot.carrying_flag = true;
                    }
                    self.dropped_flag = None;
                    self.push_notification(
                        &format!("Bot {} picked up a dropped flag.", id + 1),
                        Color::from_rgba(255, 190, 95, 255),
                    );
                }
            }
            return;
        }

        if bot_carrying_flag {
            return;
        }

        if self.flag_respawn_timer > 0.0 {
            return;
        }

        if player_can_pickup_flag
            && self
                .map()
                .is_touching_object(self.ship.pos, FLAG_TILE_ID, FLAG_PICKUP_RADIUS)
        {
            self.carrying_flag = true;
            self.flag_carry_timer = self.tuning.flag_drop_delay_seconds;
            self.send_network_flag_request(0);
            self.push_notification("Flag picked up.", Color::from_rgba(125, 230, 150, 255));

            if let Some(sound) = self.assets.flag_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.38),
                    },
                );
            }
        }

        if self.tuning.carry_flags > 0 && !self.carrying_flag && !bot_carrying_flag {
            if let Some(id) = bot_touching_map_object(&self.bots, self.map(), FLAG_TILE_ID) {
                if let Some(bot) = self.bots.iter_mut().find(|bot| bot.id == id) {
                    bot.carrying_flag = true;
                }
                self.push_notification(
                    &format!("Bot {} picked up a flag.", id + 1),
                    Color::from_rgba(255, 190, 95, 255),
                );

                if let Some(sound) = self.assets.flag_sound.as_ref() {
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
    }

    pub(super) fn drop_carried_flag_at(&mut self, pos: Vec2) {
        if !self.carrying_flag {
            return;
        }

        let drop_pos = dropped_flag_position(self.map(), pos);
        self.carrying_flag = false;
        self.flag_carry_timer = 0.0;
        self.flag_respawn_timer = 0.0;
        if self.networked_play {
            return;
        }
        self.drop_flag_at(drop_pos);
    }

    pub(super) fn drop_flags_after_safe_zone_weapon_hit(&mut self) {
        if self.networked_play {
            let carries_flag =
                self.client_authority
                    .view()
                    .scope
                    .player_id
                    .is_some_and(|player_id| {
                        self.network_world
                            .flags
                            .values()
                            .any(|flag| flag.carried_by == Some(player_id))
                    });
            if carries_flag {
                self.send_network_drop_flags();
            }
            return;
        }
        if !safe_zone_weapon_hit_drops_flag(self.in_safe_zone(), self.carrying_flag, true) {
            return;
        }

        self.send_network_drop_flags();
        self.drop_carried_flag_at(self.ship.pos);
    }

    pub(super) fn can_pickup_carryable_flag(&self) -> bool {
        if self.networked_play {
            return false;
        }
        let current_flags = self
            .client_authority
            .view()
            .scope
            .player_id
            .and_then(|player_id| self.network_world.players.get(&player_id))
            .map(|player| player.flags)
            .unwrap_or_else(|| u16::from(self.carrying_flag));
        can_pickup_carryable_flag(self.tuning.carry_flags, current_flags)
    }

    pub(super) fn drop_flag_at(&mut self, pos: Vec2) {
        if self.networked_play {
            return;
        }
        let drop_pos = dropped_flag_position(self.map(), pos);
        self.dropped_flag = Some(DroppedFlag {
            pos: drop_pos,
            age: 0.0,
            ttl: DROPPED_FLAG_LIFETIME,
        });
        self.push_notification("Flag dropped.", Color::from_rgba(255, 220, 95, 255));
    }
}

fn local_player_can_attempt_network_flag_claim(
    spectator: bool,
    ship_type: usize,
    alive: bool,
) -> bool {
    !spectator && ship_type < 8 && alive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networked_play_clears_local_flag_prediction_without_server_identity() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.carrying_flag = true;
        game.flag_carry_timer = 5.0;
        game.flag_respawn_timer = 3.0;
        game.dropped_flag = Some(DroppedFlag {
            pos: game.ship.pos,
            age: 0.0,
            ttl: 10.0,
        });

        game.update_flags(1.0 / 60.0);

        assert!(!game.carrying_flag);
        assert!(game.dropped_flag.is_none());
        assert_eq!(game.flag_carry_timer, 0.0);
        assert_eq!(game.flag_respawn_timer, 0.0);
        assert!(game.network_world.flags.is_empty());
        assert!(!game.can_pickup_carryable_flag());
    }

    #[test]
    fn networked_play_derives_local_flag_presentation_from_authoritative_owner() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.install_network_test_player_identity(7);
        game.network_world.flags.insert(
            3,
            NetworkFlag {
                flag_id: 3,
                owner: 4,
                position: game.ship.pos,
                dropped: false,
                turf: false,
                carried_by: Some(7),
            },
        );

        game.update_flags(1.0 / 60.0);

        assert!(game.carrying_flag);
        assert_eq!(game.flag_carry_timer, 0.0);
    }

    #[test]
    fn network_flag_auto_claim_requires_a_live_nonspectator_ship() {
        assert!(local_player_can_attempt_network_flag_claim(false, 0, true));
        assert!(!local_player_can_attempt_network_flag_claim(true, 0, true));
        assert!(!local_player_can_attempt_network_flag_claim(false, 8, true));
        assert!(!local_player_can_attempt_network_flag_claim(
            false, 0, false
        ));
    }
}
