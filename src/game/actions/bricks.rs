use super::*;

impl Game {
    pub(in crate::game) fn retire_expired_network_brick_lines(&mut self) {
        if let Some(now) = self.browser_server_tick() {
            let lifetime = self.tuning.brick_lifetime;
            self.network_world.brick_lines.retain(|_, drop| {
                crate::continuum::ticks_to_seconds_i32(
                    crate::continuum::continuum_tick_diff(now, drop.timestamp).max(0),
                ) < lifetime
            });
        }
    }

    pub(in crate::game) fn rebuild_network_brick_tiles(&mut self) {
        self.retire_expired_network_brick_lines();
        let lines = self
            .network_world
            .brick_lines
            .values()
            .copied()
            .collect::<Vec<_>>();
        for line in lines {
            self.install_server_brick_line(line);
        }
    }

    pub(in crate::game) fn drop_brick(&mut self, heading: Vec2) {
        let tuning = self.ship_tuning();
        let ship_pos = self.ship.pos;
        let pos = self.ship.pos - heading * (tuning.radius_pixels + TILE_SIZE * 0.5);
        let tile = world_to_tile(pos);
        // The server chooses the complete multiplayer line. A predicted single
        // tile can outlive a rejection or fall outside the accepted interval.
        let installed = !self.networked_play
            && self.install_brick_tile(tile, self.tuning.brick_lifetime, self.player_frequency());
        if installed {
            self.play_wall_sound(0.34);
        }
        self.send_network_drop_brick(ship_pos);
        let cooldown = self.effective_bomb_cooldown(tuning.brick_cooldown);
        self.ship.bricks = self.ship.bricks.saturating_sub(1);
        self.ship.bomb_cooldown = cooldown;
        self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(cooldown);
    }

    pub(in crate::game) fn install_server_brick_line(&mut self, drop: BrickDrop) {
        // BrickTime starts at the authoritative drop, not packet arrival.
        // Clear/replay and late joins must not restart an existing wall.
        let age = self.browser_server_tick().map_or(0.0, |now| {
            crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(now, drop.timestamp).max(0),
            )
        });
        let remaining = (self.tuning.brick_lifetime - age).max(0.0);
        if remaining <= 0.0 {
            return;
        }
        let start = vec2(drop.x1 as f32, drop.y1 as f32);
        let end = vec2(drop.x2 as f32, drop.y2 as f32);
        let mut enemy_overlap = false;
        let mut installed_any = false;
        let frequency = self.player_frequency();
        let ship_radius = self.ship_tuning().radius_pixels;
        for tile in brick_line_tiles(start, end) {
            enemy_overlap |= !self.spectator_mode
                && drop.team != frequency
                && brick_tile_overlaps_ship_aabb(tile, self.ship.pos, ship_radius);
            if self.install_brick_tile(tile, remaining, drop.team) {
                // Preserve the animation phase on replays as well as expiry.
                if let Some(brick) = self.bricks.iter_mut().find(|brick| brick.tile == tile) {
                    brick.age = age;
                    brick.server_timestamp = Some(drop.timestamp);
                }
                installed_any = true;
            }
        }
        if installed_any {
            self.play_wall_sound(0.28);
        }
        if enemy_overlap && !self.networked_play {
            self.spawn_after_enemy_brick_overlap();
        }
    }

    pub(in crate::game) fn install_brick_tile(&mut self, tile: IVec2, ttl: f32, team: u16) -> bool {
        if let Some(brick) = self.bricks.iter_mut().find(|brick| brick.tile == tile) {
            brick.ttl = ttl;
            brick.age = 0.0;
            brick.team = team;
            return true;
        }

        let previous_tile = self.map().tile(tile.x, tile.y);
        if previous_tile != 0 {
            return false;
        }

        self.maps[self.map_index].set_tile(tile.x, tile.y, BRICK_TILE_ID);
        self.bricks.push(Brick {
            tile,
            previous_tile,
            team,
            ttl,
            age: 0.0,
            server_timestamp: None,
        });
        true
    }

    fn spawn_after_enemy_brick_overlap(&mut self) {
        let from = self.ship.pos;
        self.warps.push(WarpEffect {
            pos: from,
            age: 0.0,
        });
        self.ship.pos = self.player_spawn_position();
        self.ship.vel = Vec2::ZERO;
        self.ship.flash = true;
        self.warps.push(WarpEffect {
            pos: self.ship.pos,
            age: 0.0,
        });
    }

    fn play_wall_sound(&self, volume: f32) {
        #[cfg(test)]
        let _ = volume;
        #[cfg(not(test))]
        if let Some(sound) = self.assets.wall_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(volume),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_brick_request_cannot_create_or_refresh_unaccepted_collision_tiles() {
        let mut game = timing_fixture(1000);
        game.networked_play = true;
        game.ship.pos = vec2(120.0, 120.0);
        game.ship.bricks = 3;
        let heading = vec2(0.0, -1.0);
        game.drop_brick(heading);
        assert!(game.bricks.is_empty());
        assert_eq!(game.map().tile(7, 8), 0);
        game.install_server_brick_line(brick_drop(900));
        assert_eq!(game.bricks.len(), 10);
        let before = game
            .bricks
            .iter()
            .map(|b| (b.tile, b.team, b.age, b.ttl))
            .collect::<Vec<_>>();
        game.drop_brick(heading);
        assert_eq!(
            game.bricks
                .iter()
                .map(|b| (b.tile, b.team, b.age, b.ttl))
                .collect::<Vec<_>>(),
            before
        );
        let mut offline = timing_fixture(1000);
        offline.networked_play = false;
        offline.ship.pos = vec2(120.0, 120.0);
        offline.ship.bricks = 1;
        offline.drop_brick(heading);
        assert_eq!(
            offline.bricks.len(),
            1,
            "offline local placement remains available"
        );
    }

    fn timing_fixture(now: u32) -> Game {
        let mut game = Game::new_network_authority_test();
        game.tuning.brick_lifetime = 120.0;
        game.last_observed_browser_server_tick = Some(now);
        for x in 7..17 {
            game.maps[game.map_index].set_tile(x, 8, 0);
        }
        game
    }

    fn brick_drop(timestamp: u32) -> BrickDrop {
        BrickDrop {
            x1: 7,
            y1: 8,
            x2: 16,
            y2: 8,
            team: 5,
            brick_id: 1,
            timestamp,
        }
    }

    #[test]
    fn network_brick_overlap_does_not_invent_a_local_teleport() {
        let mut game = timing_fixture(6000);
        game.networked_play = true;
        game.ship.pos = vec2(120.0, 136.0);
        game.ship.vel = vec2(5.0, 2.0);
        let before = game.ship.pos;
        let mut drop = brick_drop(5000);
        drop.team = game.player_frequency().wrapping_add(1);
        game.install_server_brick_line(drop);
        assert_eq!(game.bricks.len(), 10);
        assert_eq!(
            game.ship.pos, before,
            "await the server's exact correction receipt"
        );
        assert_eq!(game.ship.vel, vec2(5.0, 2.0));
        assert!(game.warps.is_empty());
    }

    #[test]
    fn network_brick_replay_preserves_creation_age_and_remaining_lifetime() {
        let mut game = timing_fixture(6000);
        game.install_server_brick_line(brick_drop(1000));
        assert_eq!(game.bricks.len(), 10);
        assert!(
            game.bricks
                .iter()
                .all(|brick| brick.age == 50.0 && brick.ttl == 70.0)
        );
        game.last_observed_browser_server_tick = Some(7000);
        game.install_server_brick_line(brick_drop(1000));
        assert_eq!(game.bricks.len(), 10);
        assert!(
            game.bricks
                .iter()
                .all(|brick| brick.age == 60.0 && brick.ttl == 60.0)
        );
    }

    #[test]
    fn network_brick_timing_rejects_expired_and_handles_wrapped_or_ahead_ticks() {
        let mut expired = timing_fixture(13000);
        expired.install_server_brick_line(brick_drop(1000));
        assert!(expired.bricks.is_empty());
        assert_eq!(expired.map().tile(7, 8), 0);
        let mut wrapped = timing_fixture(50);
        wrapped.install_server_brick_line(brick_drop(crate::continuum::CONTINUUM_TICK_MASK - 49));
        assert!(
            wrapped
                .bricks
                .iter()
                .all(|brick| brick.age == 1.0 && brick.ttl == 119.0)
        );
        let mut ahead = timing_fixture(1000);
        ahead.install_server_brick_line(brick_drop(1001));
        assert!(
            ahead
                .bricks
                .iter()
                .all(|brick| brick.age == 0.0 && brick.ttl == 120.0)
        );
    }

    #[test]
    fn network_brick_expiry_follows_server_clock_without_gameplay_steps() {
        let mut game = timing_fixture(1000);
        game.networked_play = true;
        game.install_server_brick_line(brick_drop(1000));
        game.network_world.brick_lines.insert(1, brick_drop(1000));
        game.last_observed_browser_server_tick = Some(12999);
        game.update_bricks_at_server_time();
        assert_eq!(game.bricks.len(), 10);
        assert!(
            game.bricks
                .iter()
                .all(|brick| brick.ttl > 0.0 && brick.ttl < 0.02)
        );
        game.last_observed_browser_server_tick = Some(13000);
        game.update_bricks_at_server_time();
        assert!(
            game.bricks.is_empty(),
            "paused gameplay must not retain expired collision/render tiles"
        );
        for x in 7..17 {
            assert_eq!(game.map().tile(x, 8), 0);
        }
        assert!(game.network_world.brick_lines.is_empty());
    }

    #[test]
    fn network_brick_clock_is_not_advanced_twice_by_gameplay_or_replay() {
        let mut game = timing_fixture(1100);
        game.networked_play = true;
        game.install_server_brick_line(brick_drop(1000));
        game.update_bricks(0.5);
        assert!(
            game.bricks
                .iter()
                .all(|brick| brick.age == 1.0 && brick.ttl == 119.0)
        );
        game.last_observed_browser_server_tick = Some(12990);
        game.install_server_brick_line(brick_drop(1000));
        game.update_bricks(0.5);
        assert_eq!(
            game.bricks.len(),
            10,
            "replay retains the original nearly expired wall"
        );
        game.last_observed_browser_server_tick = Some(13000);
        game.update_bricks(0.0);
        assert!(game.bricks.is_empty());
    }

    #[test]
    fn offline_bricks_keep_their_elapsed_time_lifetime() {
        let mut game = timing_fixture(1000);
        assert!(game.install_brick_tile(ivec2(7, 8), 2.0, 5));
        game.last_observed_browser_server_tick = Some(13000);
        game.update_bricks(1.0);
        assert_eq!(game.bricks.len(), 1);
        assert_eq!(game.bricks[0].ttl, 1.0);
        assert_eq!(game.bricks[0].age, 1.0);
        game.update_bricks(1.0);
        assert!(game.bricks.is_empty());
        assert_eq!(game.map().tile(7, 8), 0);
    }

    #[test]
    fn network_brick_expiry_crosses_tick_wrap_without_restarting_lifetime() {
        let mut game = timing_fixture(49);
        game.tuning.brick_lifetime = 1.0;
        game.install_server_brick_line(brick_drop(crate::continuum::CONTINUUM_TICK_MASK - 49));
        game.update_bricks_at_server_time();
        assert_eq!(game.bricks.len(), 10);
        assert!(
            game.bricks
                .iter()
                .all(|brick| brick.ttl > 0.0 && brick.ttl < 0.02)
        );
        game.last_observed_browser_server_tick = Some(50);
        game.update_bricks_at_server_time();
        assert!(game.bricks.is_empty());
    }

    #[test]
    fn network_bricks_wait_for_the_authoritative_clock_instead_of_spending_local_dt() {
        let mut game = timing_fixture(1000);
        game.install_server_brick_line(brick_drop(1000));
        game.last_observed_browser_server_tick = None;
        game.update_bricks(121.0);
        assert_eq!(game.bricks.len(), 10);
        assert!(
            game.bricks
                .iter()
                .all(|brick| brick.age == 0.0 && brick.ttl == 120.0)
        );
        game.last_observed_browser_server_tick = Some(13000);
        game.update_bricks_at_server_time();
        assert!(game.bricks.is_empty());
    }
}
