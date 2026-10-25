use super::*;

impl Game {
    pub(in crate::game) fn update_shrapnel(&mut self, dt: f32) {
        let map = &self.maps[self.map_index];
        let bricks = &self.bricks;
        let local_frequency = self.player_frequency();
        let mut impacts = Vec::new();
        let mut bot_hits = Vec::new();
        let mut bounced = false;

        for shrap in &mut self.shrapnel {
            let (event, bounced_at) = update_local_shrapnel_motion(
                shrap,
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
                Some(LocalShrapnelMotionEvent::BotHit { pos, bot_id }) => {
                    impacts.push(pos);
                    bot_hits.push((bot_id, shrap.age, shrap.inactive_damage, shrap.damage));
                    continue;
                }
                Some(LocalShrapnelMotionEvent::NetworkPlayerHit { pos }) => {
                    // Server applies damage authoritatively; locally we just
                    // stop the shrapnel and render the impact effect.
                    if shrapnel_cleanup_creates_impact(true, false, shrap.age) {
                        impacts.push(pos);
                    }
                    continue;
                }
                Some(LocalShrapnelMotionEvent::WallImpact(pos)) => {
                    if shrapnel_cleanup_creates_impact(true, false, shrap.age) {
                        impacts.push(pos);
                    }
                    continue;
                }
                Some(LocalShrapnelMotionEvent::TimedOut) => continue,
                None => {}
            }
            shrap.ttl -= dt;
            shrap.age += dt;

            let timed_out = shrap.ttl <= 0.0;
            if timed_out {
                if shrapnel_cleanup_creates_impact(false, true, shrap.age) {
                    impacts.push(shrap.pos);
                }
                shrap.ttl = 0.0;
            }
        }

        self.shrapnel.retain(|shrap| shrap.ttl > 0.0);

        for (bot_id, age, inactive_damage, damage) in bot_hits {
            if let Some(bot) = self
                .bots
                .iter_mut()
                .find(|bot| bot.id == bot_id && bot.respawn_timer <= 0.0)
            {
                let damage = shrapnel_damage(age, inactive_damage, damage);
                let damage = bot_damage_after_timed_protection(bot, damage);
                bot.energy -=
                    bullet_or_burst_damage_after_exactness(self.tuning.exact_damage, damage);
            }
        }

        if bounced {
            if let Some(sound) = self.assets.bounce_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.20),
                    },
                );
            }
        }

        for pos in impacts {
            self.add_bullet_impact(pos);
        }
    }
}
