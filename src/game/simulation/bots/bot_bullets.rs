use super::*;

impl Game {
    pub(in crate::game) fn update_bot_bullets(&mut self, dt: f32) {
        let map = &self.maps[self.map_index];
        let bricks = &self.bricks;
        let player_safe = self.in_safe_zone();
        let player_respawning = self.player_respawn_timer > 0.0;
        let player_tuning = self.ship_tuning();
        let player_radius = player_tuning.radius_pixels;
        let player_shield_lifetime = player_tuning.shield_lifetime;
        let flagger_damage_multiplier = self.tuning.flagger_damage_multiplier;
        let flagger_active = self.flagger_active();
        let tuning = self.tuning;
        let mut impacts = Vec::new();
        let mut player_damage = 0.0;
        let mut hit_by = None;
        let mut safe_zone_flag_drop = false;
        let mut bounce_sound = None;
        let decoy_targets = self
            .decoys
            .iter()
            .enumerate()
            .map(|(decoy_index, decoy)| BotBulletContactTarget {
                decoy_index,
                pos: decoy.pos,
                radius: tuning.ship(decoy.ship_type).radius_pixels,
            })
            .collect::<Vec<_>>();
        let mut hit_decoys = Vec::new();

        for bullet in &mut self.bot_bullets {
            let (event, bounced_at) = update_bot_bullet_motion_with_contacts(
                map,
                bricks,
                bullet,
                dt,
                Some((self.ship.pos, player_radius)),
                &decoy_targets,
            );
            if let Some(bounced_at) = bounced_at {
                bounce_sound = nearest_sound_position(bounce_sound, self.ship.pos, bounced_at);
            }

            if bullet.trail_timer <= 0.0 {
                bullet.trail_timer = BULLET_TRAIL_INTERVAL;
                self.bullet_trails.push(BulletTrail {
                    pos: bullet.pos - bullet.vel * 0.01,
                    age: 0.0,
                    level: bullet.level,
                    bouncing: bullet.bouncing,
                    remote_source: None,
                });
            }

            match event {
                Some(BotBulletMotionEvent::WallImpact(pos)) => {
                    if projectile_cleanup_creates_impact(true, false) {
                        impacts.push(pos);
                    }
                    continue;
                }
                Some(BotBulletMotionEvent::TimedOut) => continue,
                Some(BotBulletMotionEvent::DecoyHit { pos, decoy_index }) => {
                    impacts.push(pos);
                    hit_decoys.push(decoy_index);
                    continue;
                }
                Some(BotBulletMotionEvent::PlayerHit(pos)) => {
                    impacts.push(pos);
                    safe_zone_flag_drop |=
                        safe_zone_weapon_hit_drops_flag(player_safe, self.carrying_flag, true);
                    if !player_safe && !player_respawning {
                        let damage = damage_after_timed_protection(
                            bullet.damage,
                            self.ship.super_timer,
                            self.ship.shield_timer,
                            player_shield_lifetime,
                        );
                        let damage = bullet_or_burst_damage_after_exactness(
                            self.tuning.exact_damage,
                            damage,
                        );
                        let damage = flagger_damage_received(
                            damage,
                            flagger_damage_multiplier,
                            flagger_active,
                        );
                        if damage > 0.0 {
                            player_damage += damage;
                        }
                        hit_by = Some(bullet.owner_id);
                    }
                    continue;
                }
                None => {}
            }

            let timed_out = bullet.ttl <= 0.0;
            if timed_out {
                if projectile_cleanup_creates_impact(false, true) {
                    impacts.push(bullet.pos);
                }
                bullet.ttl = 0.0;
            }
        }

        for decoy_index in hit_decoys {
            if let Some(decoy) = self.decoys.get_mut(decoy_index) {
                decoy.ttl = 0.0;
            }
        }
        self.bot_bullets.retain(|bullet| bullet.ttl > 0.0);
        self.decoys.retain(|decoy| decoy.ttl > 0.0);
        for pos in impacts {
            self.add_bullet_impact(pos);
        }
        if let Some(source_pos) = bounce_sound {
            if let Some(sound) = self.assets.bounce_sound.as_ref() {
                if let Some(volume) = self.positional_sound_volume(source_pos, 0.18) {
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

        if safe_zone_flag_drop {
            self.drop_flags_after_safe_zone_weapon_hit();
        }

        if local_visual_damage_applies(self.networked_play, player_damage) {
            let fatal = player_damage_is_fatal(self.ship.energy, player_damage);
            if !fatal {
                self.apply_damage_prize_loss(player_damage);
            }
            self.ship.energy -= player_damage;
            if fatal {
                let death_pos = self.ship.pos;
                self.player_deaths += 1;
                let defeated_bounty = self.player_bounty;
                self.drop_carried_flag_at(death_pos);
                if let Some(owner_id) = hit_by {
                    self.send_network_death(
                        owner_id.min(u16::MAX as usize) as u16,
                        defeated_bounty.min(u16::MAX as u32) as u16,
                    );
                    if let Some(bot) = self.bots.iter_mut().find(|bot| bot.id == owner_id) {
                        bot.kills += 1;
                        if defeated_bounty > 0 {
                            bot.bounty = bot
                                .bounty
                                .saturating_add(self.tuning.bounty_increase_for_kill);
                        }
                    }
                }
                let killer_label = hit_by.map(|id| format!("Bot {}", id + 1));
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
