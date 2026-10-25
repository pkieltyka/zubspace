use super::*;

impl Game {
    pub(in crate::game) fn update_bomb_flashes(&mut self, dt: f32) {
        for flash in &mut self.bomb_flashes {
            flash.age += dt;
        }
        self.bomb_flashes
            .retain(|flash| flash.age < BOMB_FLASH_LIFETIME);
    }

    pub(in crate::game) fn update_bombs(&mut self, dt: f32) {
        let bomb_gravity = self
            .tuning
            .gravity_bombs
            .then_some(self.ship_tuning().wormhole_gravity);
        let map = &self.maps[self.map_index];
        let bricks = &self.bricks;
        let mut explosions = Vec::new();
        let local_frequency = self.player_frequency();

        for bomb in &mut self.bombs {
            bomb.trail_timer -= dt;

            if bomb.trail_timer <= 0.0 {
                bomb.trail_timer = BOMB_TRAIL_INTERVAL;
                self.bomb_trails.push(BombTrail {
                    pos: bomb.pos - bomb.vel * 0.01,
                    age: 0.0,
                    remote_source: None,
                });
            }

            if let Some(event) = update_bomb_motion(
                bomb,
                dt,
                map,
                bricks,
                &self.bots,
                &self.network_world,
                local_frequency,
                self.tuning,
                bomb_gravity,
            ) {
                let (pos, damage_pos) = match event {
                    BombMotionEvent::DirectHit {
                        explosion_pos,
                        damage_pos,
                    }
                    | BombMotionEvent::ProximityExplosion {
                        explosion_pos,
                        damage_pos,
                    } => (explosion_pos, damage_pos),
                    BombMotionEvent::WallExplosion(pos) => (pos, pos),
                };
                let shrapnel = explosion_shrapnel_for_bomb_event(event, bomb.shrapnel);
                explosions.push((
                    pos,
                    damage_pos,
                    bomb.level,
                    bomb.damage,
                    bomb.emp_shutdown_time,
                    shrapnel,
                    bomb.prediction,
                ));
            }
        }

        self.bombs.retain(|bomb| bomb.ttl > 0.0);

        for (pos, damage_pos, level, damage, emp_shutdown_time, shrapnel, prediction) in explosions
        {
            self.add_predicted_explosion(
                pos,
                damage_pos,
                level,
                damage,
                emp_shutdown_time,
                shrapnel,
                prediction,
            );
        }
    }

    pub(in crate::game) fn update_bomb_trails(&mut self, dt: f32) {
        for trail in &mut self.bomb_trails {
            trail.age += dt;
        }
        self.bomb_trails
            .retain(|trail| trail.age < BOMB_TRAIL_LIFETIME);
    }

    pub(in crate::game) fn update_mines(&mut self, dt: f32) {
        let map = &self.maps[self.map_index];
        let mut explosions = Vec::new();
        let local_frequency = self.player_frequency();

        for mine in &mut self.mines {
            mine.age += dt;
            mine.ttl -= dt;

            let (wall_impact, wormhole_timeout) = update_mine_terrain_motion(
                &mut mine.pos,
                &mut mine.vel,
                dt,
                map,
                &self.bricks,
                mine.frequency,
                mine.bomb_gravity,
            );
            if wormhole_timeout {
                mine.ttl = 0.0;
                continue;
            }

            let expired = mine.ttl <= 0.0;
            let triggered = if mine.proximity {
                let radius =
                    proximity_bomb_trigger_radius(self.tuning.proximity_distance, mine.level);
                let proximity_trigger = proximity_bomb_trigger_by_bot(
                    mine.proximity_sensor,
                    mine.pos,
                    &self.bots,
                    self.tuning,
                    radius,
                )
                .or_else(|| {
                    proximity_bomb_trigger_by_network_player(
                        mine.proximity_sensor,
                        mine.pos,
                        &self.network_world,
                        local_frequency,
                        self.tuning,
                        radius,
                    )
                });
                proximity_bomb_sensor_exploded(
                    &mut mine.proximity_sensor,
                    proximity_trigger,
                    mine.age,
                    dt,
                    self.tuning.bomb_explode_delay_seconds,
                )
            } else {
                mine_triggered_by_bot(mine.pos, &self.bots, self.tuning)
                    || explosive_contact_triggered_by_network_player(
                        mine.pos,
                        &self.network_world,
                        local_frequency,
                        self.tuning,
                    )
            };

            if wall_impact || explosive_cleanup_creates_explosion(triggered, expired) {
                explosions.push((
                    mine.pos,
                    mine.level,
                    mine.damage,
                    mine.emp_shutdown_time,
                    mine.shrapnel,
                    mine.prediction,
                ));
                mine.ttl = 0.0;
            } else if expired {
                mine.ttl = 0.0;
            }
        }

        self.mines.retain(|mine| mine.ttl > 0.0);

        for (pos, level, damage, emp_shutdown_time, shrapnel, prediction) in explosions {
            self.add_predicted_explosion(
                pos,
                pos,
                level,
                damage,
                emp_shutdown_time,
                shrapnel,
                prediction,
            );
        }
    }
}
