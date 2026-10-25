use super::*;

impl Game {
    /// Prepare once after the actor cursor. Drawing, sprite evidence and muzzle
    /// diagnostics all consume these exact copies; runtime contact stays current.
    /// Returns the number of recent firing sound cues dispatched this frame.
    pub(in crate::game) fn prepare_remote_weapon_views(&mut self, actor_tick: Option<u32>) -> u32 {
        let mut sound_cues = 0;
        self.prune_owner_bomb_draw_bridges();
        self.prepared_remote_projectiles.clear();
        self.prepared_remote_bullet_trails.clear();
        self.prepared_remote_bomb_trails.clear();
        let now = self.browser_server_tick();
        let partial = self.weapon_render_partial_seconds();
        let scope = self.client_authority.view().scope;
        let viewer_frequency = self.viewer_frequency();
        self.prepared_remote_weapon_groups.clear();
        for live in &self.remote_projectiles {
            let view = if captures_remote_weapon_view(self.networked_play, scope, live)
                && let Some(now) = now
            {
                // Birth alignment needs the exact ship this viewer can draw,
                // not just a global actor cursor. A shot can outlive its owner
                // or arrive after that owner's death/replacement. Preserve its
                // current flight instead of replaying a muzzle on an absent
                // or invisible predecessor. None permanently joins current
                // time through the existing birth-clock fallback.
                let source_actor_tick = actor_tick.and_then(|tick| {
                    let RemoteWeaponSourceIdentity::Authenticated {
                        player_id,
                        session_id,
                        life_seq,
                    } = live.source.identity
                    else {
                        return None;
                    };
                    self.network_world
                        .players
                        .get(&player_id)
                        .filter(|player| {
                            player.source_session_id == Some(session_id)
                                && player.life_seq == Some(life_seq)
                                && player.ship < SPECTATOR_SHIP
                                && player.attach_parent == INVALID_PLAYER_ID
                                && player.position.is_some()
                                && player.display_position().is_some()
                                && network_player_world_visible(
                                    player.togglables,
                                    player.frequency,
                                    viewer_frequency,
                                    self.ship.xradar,
                                )
                        })
                        .and_then(|player| {
                            if !player.directed_presentation {
                                return Some(tick);
                            }
                            player.directed_render_time.map(|time| {
                                let (tick, third) = time.parts();
                                // A fractional held sample already follows
                                // this integer tick's muzzle. Never identify
                                // that later pose as an exact birth sample.
                                crate::continuum::continuum_tick_add(tick, u32::from(third > 0))
                            })
                        })
                });
                let view = self.remote_weapon_view_history.prepare_live_view(
                    live,
                    now,
                    source_actor_tick,
                    partial,
                );
                if let Some(birth) = live.source.realtime_server_tick
                    && source_actor_tick == Some(birth)
                    && view
                        .and_then(|view| view.presentation_clock)
                        .is_some_and(|clock| clock.pose_tick == birth)
                    && !matches!(live.kind, RemoteProjectileKind::Shrapnel { .. })
                    && let RemoteWeaponSourceIdentity::Authenticated {
                        player_id,
                        session_id,
                        life_seq,
                    } = live.source.identity
                    && let Some(player) = self.network_world.players.get_mut(&player_id)
                {
                    player.present_firing_heading(
                        session_id,
                        life_seq,
                        birth,
                        live.source.fire_orientation,
                    );
                }
                view
            } else {
                Some(*live)
            };
            if let Some(key) = crate::remote_weapon::RemoteWeaponViewKey::from_projectile(live) {
                self.prepared_remote_weapon_groups.push((
                    key.group_key(),
                    view.and_then(|p| {
                        p.presentation_clock
                            .map(|clock| (clock.pose_tick, clock.render_partial_seconds))
                    }),
                ));
            }
            if let Some(view) = view {
                self.prepared_remote_projectiles.push(view);
            }
        }

        self.prepared_remote_weapon_groups
            .sort_unstable_by_key(|entry| entry.0);
        self.prepared_remote_weapon_groups
            .dedup_by(|later, earlier| {
                if later.0 != earlier.0 {
                    return false;
                }
                if earlier.1.is_none() {
                    earlier.1 = later.1;
                } else if let (Some(a), Some(b)) = (later.1, earlier.1)
                    && (crate::continuum::continuum_tick_diff(a.0, b.0) < 0
                        || (a.0 == b.0 && a.1 < b.1))
                {
                    earlier.1 = Some(a);
                }
                true
            });
        let effect_count = self.pending_remote_birth_effects.len();
        for _ in 0..effect_count {
            let Some(effect) = self.pending_remote_birth_effects.pop_front() else {
                break;
            };
            if self.remote_source_cancelled_by_owner(effect.source) {
                continue;
            }
            let Ok(index) = self
                .prepared_remote_weapon_groups
                .binary_search_by_key(&effect.key, |entry| entry.0)
            else {
                // An admitted shot can hit a wall or player in its birth
                // tick, before any render frame prepares its flight. That
                // terminal removes flight/flash/trails immediately, but is
                // not a cancellation of the firing sound. The reference game plays
                // that cue on FireWeapons, independently of collision.
                if let Some((now, birth)) = now.zip(effect.source.realtime_server_tick) {
                    let elapsed = crate::continuum::continuum_tick_diff(now, birth);
                    if elapsed >= 0 {
                        sound_cues += self.dispatch_remote_birth_sound(
                            effect,
                            crate::continuum::ticks_to_seconds_i32(elapsed) + partial,
                        );
                    }
                }
                continue;
            };
            let Some((tick, fraction)) = self.prepared_remote_weapon_groups[index].1 else {
                self.pending_remote_birth_effects.push_back(effect);
                continue;
            };
            let Some(birth) = effect.source.realtime_server_tick else {
                continue;
            };
            let age = crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(tick, birth).max(0),
            ) + fraction;
            let tuning = self.tuning.ship(effect.source.ship as usize);
            sound_cues += self.dispatch_remote_birth_sound(effect, age);
            if let Some((visual, pos)) = remote_weapon_visual_position(
                effect.weapon_data,
                effect.source.fire_position,
                effect.source.fire_orientation,
                tuning.radius_pixels,
            ) {
                match visual {
                    RemoteWeaponVisual::BombFlash if age < BOMB_FLASH_LIFETIME => {
                        self.bomb_flashes.push(BombFlash {
                            pos,
                            age,
                            remote_source: Some(effect.source.identity),
                        })
                    }
                    RemoteWeaponVisual::Burst if age < BURST_EFFECT_LIFETIME => {
                        self.burst_effects.push(BurstEffect {
                            pos,
                            age,
                            remote_source: Some(effect.source.identity),
                        })
                    }
                    RemoteWeaponVisual::Repel => {
                        unreachable!("Repel has its existing immediate causal path")
                    }
                    _ => {}
                }
            }
        }

        let trail_count = self.remote_weapon_trails.len();
        for _ in 0..trail_count {
            let Some(mut trail) = self.remote_weapon_trails.pop_front() else {
                break;
            };
            if self.remote_source_cancelled_by_owner(trail.source) {
                continue;
            }
            let lifetime = match trail.kind {
                RemoteWeaponTrailKind::Bullet { .. } => BULLET_TRAIL_LIFETIME,
                RemoteWeaponTrailKind::Bomb => BOMB_TRAIL_LIFETIME,
            };
            let Some(now) = now else {
                continue;
            };
            let server_age = crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(now, trail.born_tick),
            );
            if server_age
                > lifetime
                    + crate::combat_timeline::PROJECTILE_PRESENTATION_MIN_LEAD_TICKS as f32 / 100.0
            {
                continue;
            }
            let cursor = match self
                .prepared_remote_weapon_groups
                .binary_search_by_key(&trail.key, |entry| entry.0)
            {
                Ok(index) => self.prepared_remote_weapon_groups[index].1,
                // A previously drawn trail may fade after its weapon retires.
                // An unseen birth must not suddenly reveal a trail at terminal.
                Err(_) if trail.presented => Some((now, partial)),
                Err(_) => continue,
            };
            let Some((tick, fraction)) = cursor else {
                self.remote_weapon_trails.push_back(trail);
                continue;
            };
            let elapsed = crate::continuum::continuum_tick_diff(tick, trail.born_tick);
            if elapsed < 0 {
                self.remote_weapon_trails.push_back(trail);
                continue;
            }
            let age = crate::continuum::ticks_to_seconds_i32(elapsed) + fraction;
            if age >= lifetime {
                continue;
            }
            match trail.kind {
                RemoteWeaponTrailKind::Bullet { level, bouncing } => {
                    self.prepared_remote_bullet_trails.push(BulletTrail {
                        pos: trail.position,
                        age,
                        level,
                        bouncing,
                        remote_source: Some(trail.source.identity),
                    })
                }
                RemoteWeaponTrailKind::Bomb => self.prepared_remote_bomb_trails.push(BombTrail {
                    pos: trail.position,
                    age,
                    remote_source: Some(trail.source.identity),
                }),
            }
            trail.presented = true;
            self.remote_weapon_trails.push_back(trail);
        }
        sound_cues
    }

    fn dispatch_remote_birth_sound(&self, effect: PendingRemoteWeaponBirthEffect, age: f32) -> u32 {
        if age < 0.0
            || age >= crate::combat_timeline::PROJECTILE_PRESENTATION_MIN_LEAD_TICKS as f32 / 100.0
        {
            return 0;
        }
        #[cfg(not(test))]
        self.play_remote_weapon_sound(
            effect.weapon_data,
            self.tuning.ship(effect.source.ship as usize),
            effect.source.fire_position,
        );
        #[cfg(test)]
        let _ = effect;
        1
    }
}
