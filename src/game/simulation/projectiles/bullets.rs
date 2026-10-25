use super::*;

impl Game {
    pub(in crate::game) fn update_bullets(&mut self, dt: f32) {
        let map = &self.maps[self.map_index];
        let bricks = &self.bricks;
        let local_frequency = self.player_frequency();
        let mut impacts = Vec::new();
        let mut bot_hits = Vec::new();
        let mut bounced = false;

        for bullet in &mut self.bullets {
            let (event, bounced_at) = update_local_bullet_motion(
                bullet,
                dt,
                map,
                bricks,
                &self.bots,
                &self.network_world,
                local_frequency,
                self.tuning,
            );
            bounced |= bounced_at.is_some();
            match event {
                Some(LocalBulletMotionEvent::BotHit { pos, bot_id }) => {
                    impacts.push(pos);
                    bot_hits.push((bot_id, bullet.damage));
                    continue;
                }
                Some(LocalBulletMotionEvent::NetworkPlayerHit { pos }) => {
                    // Damage is applied authoritatively by the server; the
                    // shooter's client only renders the visual impact and
                    // stops the bullet so it doesn't continue past the target.
                    impacts.push(pos);
                    continue;
                }
                Some(LocalBulletMotionEvent::WallImpact(pos)) => {
                    impacts.push(pos);
                    continue;
                }
                Some(LocalBulletMotionEvent::TimedOut) => continue,
                None => {}
            }
            bullet.ttl -= dt;
            bullet.age += dt;
            bullet.trail_timer -= dt;

            if !bullet.activates_on_wall && bullet.trail_timer <= 0.0 {
                bullet.trail_timer = BULLET_TRAIL_INTERVAL;
                self.bullet_trails.push(BulletTrail {
                    pos: bullet.pos - bullet.vel * 0.01,
                    age: 0.0,
                    level: bullet.level,
                    bouncing: bullet.bouncing,
                    remote_source: None,
                });
            }

            let solid_hit = bullet.ttl > 0.0
                && !bullet.activates_on_wall
                && !bullet.bouncing
                && is_solid_world_for_frequency(map, bullet.pos, bullet.frequency, bricks);
            let timed_out = bullet.ttl <= 0.0;
            if timed_out || solid_hit {
                if projectile_cleanup_creates_impact(solid_hit, timed_out) {
                    impacts.push(bullet.pos);
                }
                bullet.ttl = 0.0;
            }
        }

        if bounced {
            if let Some(sound) = self.assets.bounce_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.24),
                    },
                );
            }
        }

        self.bullets.retain(|bullet| bullet.ttl > 0.0);

        for (bot_id, damage) in bot_hits {
            if let Some(bot) = self
                .bots
                .iter_mut()
                .find(|bot| bot.id == bot_id && bot.respawn_timer <= 0.0)
            {
                let damage = bot_damage_after_timed_protection(bot, damage);
                bot.energy -=
                    bullet_or_burst_damage_after_exactness(self.tuning.exact_damage, damage);
            }
        }

        for pos in impacts {
            self.add_bullet_impact(pos);
        }
    }

    pub(in crate::game) fn update_bullet_trails(&mut self, dt: f32) {
        for trail in &mut self.bullet_trails {
            trail.age += dt;
        }
        self.bullet_trails
            .retain(|trail| trail.age < BULLET_TRAIL_LIFETIME);
    }

    pub(in crate::game) fn update_bullet_impacts(&mut self, dt: f32) {
        for impact in &mut self.bullet_impacts {
            impact.age += dt;
        }
        self.bullet_impacts
            .retain(|impact| impact.age < BULLET_IMPACT_LIFETIME);
    }
}
