use super::*;
use crate::math::canonical_discrete_heading;

impl Game {
    pub(in crate::game) fn update_powerball(&mut self, dt: f32) {
        if self.networked_play {
            // Multiplayer objective existence comes only from NetworkWorld,
            // which is populated by authoritative server packets. The local
            // ball remains an offline simulation detail.
            let was_carried = self.powerball.carried;
            let carried_ball = self
                .client_authority
                .view()
                .scope
                .player_id
                .and_then(|player_id| {
                    self.network_world
                        .powerballs
                        .values()
                        .find(|ball| ball.carried && ball.owner_id == Some(player_id))
                        .cloned()
                });
            self.powerball.carried = carried_ball.is_some();
            if let Some(ball) = &carried_ball {
                self.powerball.ball_id = ball.ball_id;
            }
            if !was_carried && self.powerball.carried {
                let pickup_delay = self.ship_tuning().bomb_cooldown;
                (self.ship.bullet_cooldown, self.ship.bomb_cooldown) =
                    powerball_pickup_weapon_cooldowns_after(
                        self.ship.bullet_cooldown,
                        self.ship.bomb_cooldown,
                        pickup_delay,
                    );
            }
            self.powerball.vel = Vec2::ZERO;
            self.powerball.carry_timer = 0.0;
            self.powerball.pass_cooldown = 0.0;
            self.powerball_trails.clear();
            let pickup_radius = self.ship_tuning().soccer_ball_proximity;
            let current_server_tick = self.browser_server_tick();
            if let Some(player_id) = self.client_authority.view().scope.player_id
                && local_player_can_attempt_powerball_pickup(
                    self.spectator_mode,
                    self.ship.ship_type,
                    self.local_ship_is_attached_to_turret(),
                    self.powerball.carried,
                    self.player_respawn_timer <= 0.0,
                )
                && let Some(ball) = self
                    .network_world
                    .powerballs
                    .values()
                    .filter(|ball| !ball.carried)
                    .filter(|ball| {
                        !ball.objective_pickup_locked_for(player_id, current_server_tick)
                    })
                    .filter(|ball| !ball.objective_pickup_blocked_for_last_thrower(player_id))
                    .filter(|ball| {
                        (ball.position.x - self.ship.pos.x).abs() < pickup_radius
                            && (ball.position.y - self.ship.pos.y).abs() < pickup_radius
                    })
                    .min_by(|left, right| {
                        left.position
                            .distance_squared(self.ship.pos)
                            .total_cmp(&right.position.distance_squared(self.ship.pos))
                            .then_with(|| left.ball_id.cmp(&right.ball_id))
                    })
                    .cloned()
            {
                self.send_network_ball_pickup(ball.ball_id, ball.timestamp);
            }
            return;
        }
        // Non-soccer arenas (e.g. chaos) have no powerball — skip the entire
        // local simulation so trails, sounds, and goal checks don't fire.
        if self.tuning.soccer_mode == 0 {
            return;
        }
        self.powerball.age += dt;
        self.powerball.trail_timer -= dt;
        self.powerball.pass_cooldown = (self.powerball.pass_cooldown - dt).max(0.0);

        if self.powerball.carried {
            let heading = canonical_discrete_heading(self.ship.orientation);
            self.powerball.pos = self.ship.pos + heading * 24.0;
            self.powerball.vel = self.ship.vel;
            self.powerball.frequency = self.player_frequency();
            if self.powerball.carry_timer > 0.0 {
                self.powerball.carry_timer -= dt;
                if self.powerball.carry_timer <= 0.0 {
                    self.release_powerball_after_carry_timer(heading);
                }
            }
            return;
        }

        let bounced = {
            let map = &self.maps[self.map_index];
            move_bouncing_point_for_frequency(
                map,
                &mut self.powerball.pos,
                &mut self.powerball.vel,
                dt,
                self.powerball.frequency,
                &self.bricks,
            )
        };
        if bounced {
            if let Some(sound) = self.assets.ball_bounce_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.26),
                    },
                );
            }
        }

        if let Some((bot_id, pos, vel, friction)) = bot_powerball_kick(
            self.powerball.pos,
            &self.bots,
            self.map(),
            &self.tuning,
            self.tuning.disable_ball_through_walls,
            self.powerball.pass_cooldown,
        ) {
            self.powerball.pos = pos;
            self.powerball.vel = vel;
            self.powerball.trail_timer = 0.0;
            self.powerball.pass_cooldown = self.tuning.pass_delay_seconds;
            self.powerball.friction = friction;
            self.powerball.last_touch = PowerballTouch::Bot(bot_id);
            self.powerball.frequency = 1;
            if let Some(sound) = self.assets.ball_bounce_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.24),
                    },
                );
            }
        }

        self.powerball.vel *= self.powerball.friction.powf(dt * 60.0);
        if self.powerball.trail_timer <= 0.0 && self.powerball.vel.length_squared() > 900.0 {
            self.powerball.trail_timer = POWERBALL_TRAIL_INTERVAL;
            self.powerball_trails.push(PowerballTrail {
                pos: self.powerball.pos,
                age: 0.0,
            });
        }

        let touching_goal =
            self.map()
                .is_touching_object(self.powerball.pos, GOAL_TILE_ID, FLAG_PICKUP_RADIUS)
                || self.map().tile_at_world(self.powerball.pos) == GOAL_TILE_ID;
        let scorer_frequency = match self.powerball.last_touch {
            PowerballTouch::Player => self.player_frequency(),
            PowerballTouch::Bot(_) => 1,
        };
        if touching_goal
            && soccer_goal_counts(
                self.powerball.pos,
                TILE_SIZE,
                scorer_frequency,
                self.tuning.soccer_mode,
            )
        {
            let message = match self.powerball.last_touch {
                PowerballTouch::Player => {
                    self.ball_score += 1;
                    self.send_network_ball_goal(0, self.powerball.pos);
                    "Powerball goal!"
                }
                PowerballTouch::Bot(id) => {
                    self.bot_ball_score += 1;
                    if let Some(bot) = self.bots.iter_mut().find(|bot| bot.id == id) {
                        bot.ball_goals += 1;
                    }
                    "Bot powerball goal!"
                }
            };
            self.push_notification(message, Color::from_rgba(255, 220, 95, 255));
            self.reset_powerball();
            if let Some(sound) = self.assets.goal_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.38),
                    },
                );
            }
            return;
        }

        let tuning = self.ship_tuning();
        if local_player_can_attempt_powerball_pickup(
            self.spectator_mode,
            self.ship.ship_type,
            self.local_ship_is_attached_to_turret(),
            self.powerball.carried,
            self.player_respawn_timer <= 0.0,
        ) && !self.in_safe_zone()
            && self.ship.pos.distance(self.powerball.pos) <= tuning.soccer_ball_proximity
            && can_touch_powerball(
                self.map(),
                self.ship.pos,
                self.powerball.pos,
                self.tuning.disable_ball_through_walls,
                self.powerball.pass_cooldown,
            )
        {
            self.powerball.carried = true;
            self.powerball.vel = Vec2::ZERO;
            self.powerball.carry_timer = tuning.soccer_ball_throw_timer;
            self.powerball.last_touch = PowerballTouch::Player;
            self.powerball.frequency = self.player_frequency();
            (self.ship.bullet_cooldown, self.ship.bomb_cooldown) =
                powerball_pickup_weapon_cooldowns_after(
                    self.ship.bullet_cooldown,
                    self.ship.bomb_cooldown,
                    tuning.bomb_cooldown,
                );
            self.powerball.ball_id = 0;
            self.send_network_ball_pickup(self.powerball.ball_id, 0);
            self.push_notification("Powerball picked up.", Color::from_rgba(120, 220, 255, 255));
            if let Some(sound) = self.assets.ball_catch_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.26),
                    },
                );
            }
        }
    }

    pub(in crate::game) fn update_powerball_trails(&mut self, dt: f32) {
        for trail in &mut self.powerball_trails {
            trail.age += dt;
        }
        self.powerball_trails
            .retain(|trail| trail.age < POWERBALL_TRAIL_LIFETIME);
    }

    pub(in crate::game) fn kick_powerball(&mut self, heading: Vec2) {
        if self.networked_play {
            if let Some(player_id) = self.client_authority.view().scope.player_id
                && let Some(ball) = self
                    .network_world
                    .powerballs
                    .values()
                    .find(|ball| ball.carried && ball.owner_id == Some(player_id))
                    .cloned()
            {
                self.send_network_ball_fire(ball.ball_id, self.ship.pos, self.ship.vel);
            }
            return;
        }
        self.powerball.carried = false;
        let tuning = self.ship_tuning();
        self.powerball.vel =
            manual_powerball_fire_velocity(self.ship.vel, heading, tuning.soccer_ball_speed);
        self.powerball.pos = self.ship.pos + heading * 28.0;
        self.powerball.trail_timer = 0.0;
        self.powerball.pass_cooldown = self.tuning.pass_delay_seconds;
        self.powerball.carry_timer = 0.0;
        self.powerball.friction = tuning.soccer_ball_friction;
        self.powerball.last_touch = PowerballTouch::Player;
        self.powerball.frequency = self.player_frequency();
        self.send_network_ball_fire(
            self.powerball.ball_id,
            manual_powerball_fire_request_position(self.ship.pos),
            self.powerball.vel,
        );
        self.play_powerball_throw_sound();
    }

    pub(in crate::game) fn release_powerball_after_carry_timer(&mut self, heading: Vec2) {
        if self.networked_play {
            return;
        }
        self.powerball.carried = false;
        let tuning = self.ship_tuning();
        self.powerball.vel =
            timed_powerball_release_velocity(self.ship.vel, heading, tuning.soccer_ball_speed);
        self.powerball.trail_timer = 0.0;
        self.powerball.pass_cooldown = self.tuning.pass_delay_seconds;
        self.powerball.carry_timer = 0.0;
        self.powerball.friction = tuning.soccer_ball_friction;
        self.powerball.last_touch = PowerballTouch::Player;
        self.powerball.frequency = self.player_frequency();
        self.send_network_ball_fire(
            self.powerball.ball_id,
            self.powerball.pos,
            self.powerball.vel,
        );
        self.play_powerball_throw_sound();
    }

    pub(in crate::game) fn kick_powerball_with_weapon_delay(&mut self, heading: Vec2) {
        self.kick_powerball(heading);
        self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(POWERBALL_WEAPON_DELAY);
        self.ship.bomb_cooldown = self.ship.bomb_cooldown.max(POWERBALL_WEAPON_DELAY);
    }

    pub(in crate::game) fn reset_powerball(&mut self) {
        self.powerball.ball_id = 0;
        self.powerball.pos = self.map().powerball_spawn_position(self.ship.pos);
        self.powerball.vel = Vec2::ZERO;
        self.powerball.carried = false;
        self.powerball.trail_timer = 0.0;
        self.powerball.pass_cooldown = 0.0;
        self.powerball.carry_timer = 0.0;
        self.powerball.friction = POWERBALL_FRICTION;
        self.powerball.last_touch = PowerballTouch::Player;
        self.powerball.frequency = INVALID_PLAYER_ID;
        self.powerball_trails.clear();
    }

    fn play_powerball_throw_sound(&self) {
        if let Some(sound) = self.assets.ball_throw_sound.as_ref() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networked_play_clears_local_powerball_prediction_without_server_identity() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.tuning.soccer_mode = 3;
        game.powerball.carried = true;
        game.powerball.vel = vec2(20.0, -10.0);
        game.powerball.carry_timer = 5.0;
        game.powerball.pass_cooldown = 3.0;
        game.powerball_trails.push(PowerballTrail {
            pos: game.powerball.pos,
            age: 0.0,
        });

        game.update_powerball(1.0 / 60.0);

        assert!(!game.powerball.carried);
        assert_eq!(game.powerball.vel, Vec2::ZERO);
        assert_eq!(game.powerball.carry_timer, 0.0);
        assert_eq!(game.powerball.pass_cooldown, 0.0);
        assert!(game.powerball_trails.is_empty());
        assert!(game.network_world.powerballs.is_empty());
    }

    #[test]
    fn networked_play_derives_local_ball_presentation_from_authoritative_owner() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.install_network_test_player_identity(7);
        game.network_world.powerballs.insert(
            2,
            NetworkPowerball {
                ball_id: 2,
                position: game.ship.pos,
                velocity: Vec2::ZERO,
                owner_id: Some(7),
                timestamp: 4,
                carried: true,
                pickup_unlock_tick: None,
                killer_id: None,
                killer_pickup_unlock_tick: None,
                authoritative_unlock_provenance: true,
                age_since_update: 0.0,
            },
        );

        game.update_powerball(1.0 / 60.0);

        assert!(game.powerball.carried);
        assert_eq!(game.powerball.ball_id, 2);
        assert_eq!(game.powerball.carry_timer, 0.0);
    }

    #[test]
    fn networked_pickup_applies_authoritative_bomb_delay_once() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.install_network_test_player_identity(7);
        game.network_world.powerballs.insert(
            2,
            NetworkPowerball {
                ball_id: 2,
                position: game.ship.pos,
                velocity: Vec2::ZERO,
                owner_id: Some(7),
                timestamp: 4,
                carried: true,
                pickup_unlock_tick: None,
                killer_id: None,
                killer_pickup_unlock_tick: None,
                authoritative_unlock_provenance: true,
                age_since_update: 0.0,
            },
        );

        game.update_powerball(1.0 / 60.0);
        let expected = game.ship_tuning().bomb_cooldown;
        assert_eq!(game.ship.bullet_cooldown, expected);
        assert_eq!(game.ship.bomb_cooldown, expected);

        game.ship.bullet_cooldown = 0.1;
        game.ship.bomb_cooldown = 0.2;
        game.update_powerball(1.0 / 60.0);
        assert_eq!(game.ship.bullet_cooldown, 0.1);
        assert_eq!(game.ship.bomb_cooldown, 0.2);
    }

    #[test]
    fn networked_manual_fire_applies_fifty_tick_weapon_delay() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.install_network_test_player_identity(7);
        game.network_world.powerballs.insert(
            2,
            NetworkPowerball {
                ball_id: 2,
                position: game.ship.pos,
                velocity: Vec2::ZERO,
                owner_id: Some(7),
                timestamp: 4,
                carried: true,
                pickup_unlock_tick: None,
                killer_id: None,
                killer_pickup_unlock_tick: None,
                authoritative_unlock_provenance: true,
                age_since_update: 0.0,
            },
        );

        game.kick_powerball_with_weapon_delay(Vec2::X);

        assert_eq!(game.ship.bullet_cooldown, POWERBALL_WEAPON_DELAY);
        assert_eq!(game.ship.bomb_cooldown, POWERBALL_WEAPON_DELAY);
    }
}
