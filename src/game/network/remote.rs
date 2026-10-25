use super::*;

#[cfg(test)]
#[path = "remote/terminal_pipeline_tests.rs"]
mod terminal_pipeline_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteWeaponPresentationAdmission {
    Ready,
    Pending(RemoteWeaponPresentationSuppressionReason),
    Terminal(RemoteWeaponPresentationSuppressionReason),
}

impl Game {
    pub(in crate::game) fn clear_remote_weapon_presentation(&mut self) {
        self.projectile_refresh_needed = false;
        self.last_projectile_refresh_request_tick = None;
        self.remote_projectiles.clear();
        self.owner_bomb_draw_bridges.clear();
        self.remote_weapon_view_history.clear();
        self.prepared_remote_projectiles.clear();
        self.prepared_remote_weapon_groups.clear();
        self.pending_remote_birth_effects.clear();
        self.remote_weapon_trails.clear();
        self.prepared_remote_bullet_trails.clear();
        self.prepared_remote_bomb_trails.clear();
        self.remote_projectile_terminals.clear();
        self.remote_owner_weapon_cancellations.clear();
        self.remote_repel_presentation_history.clear();
        self.remote_repel_authoritative_history.clear();
        self.latest_remote_realtime_event_id = None;
        self.remote_weapon_presentations.cancel_all();
        self.decoys.retain(|decoy| decoy.remote_source.is_none());
        self.bullet_trails
            .retain(|effect| effect.remote_source.is_none());
        self.bullet_impacts
            .retain(|effect| effect.remote_source.is_none());
        self.bomb_flashes
            .retain(|effect| effect.remote_source.is_none());
        self.bomb_trails
            .retain(|effect| effect.remote_source.is_none());
        self.repels.retain(|effect| effect.remote_source.is_none());
        self.burst_effects
            .retain(|effect| effect.remote_source.is_none());
        self.explosions
            .retain(|effect| effect.remote_source.is_none());
    }

    /// Apply the exact target-private Repel origin/frequency captured by the
    /// server when the hostile weapon was admitted. Mutable render position
    /// or roster visibility can never choose the impulse that gets marked.
    pub(in crate::game) fn apply_private_remote_repel(
        &mut self,
        player_id: u16,
        origin: Vec2,
        source_frequency: u16,
        expected_velocity: Vec2,
    ) -> bool {
        if Some(player_id) == self.client_authority.view().scope.player_id {
            return false;
        }
        // The server already decided applicability at the immutable event
        // tick. This private record owns only target physics; the public
        // event owns sound/visual exactly once and is never suppressed by a
        // retry from an older event with the same numeric source id.
        self.apply_local_weapon_repel_effect(origin, source_frequency);
        self.ship.vel = expected_velocity;
        true
    }

    /// Apply one remote weapon event and report whether a Repel actually
    /// replaced the local ship velocity. Presentation alone never arms the
    /// authoritative application marker.
    pub(in crate::game) fn apply_remote_weapon_visual(
        &mut self,
        player_id: u16,
        weapon_data: u16,
    ) -> bool {
        if weapon_data == 0 || Some(player_id) == self.client_authority.view().scope.player_id {
            return false;
        }

        let Some(player) = self.network_world.players.get(&player_id) else {
            return false;
        };
        let Some(pos) = player.position else {
            return false;
        };
        let orientation = player.orientation;
        let velocity = player.velocity;
        let frequency = player.frequency;
        let ship = player.ship;
        let identity = RemoteWeaponSourceIdentity::legacy(player_id, player.life_seq);
        let Some(source) = self.capture_remote_weapon_source(
            identity,
            None,
            pos,
            velocity,
            orientation,
            frequency,
            ship,
        ) else {
            return false;
        };
        self.apply_remote_weapon_visual_from_source(source, weapon_data)
    }

    /// Render an authenticated one-shot from its immutable server-captured
    /// source state. Newer snapshot pose, death/respawn, player-id reuse, or
    /// cross-channel reordering cannot change this event's origin or heading.
    pub(in crate::game) fn apply_realtime_weapon_visual(
        &mut self,
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        source: crate::realtime_event::RealtimeWeaponEvent,
    ) -> bool {
        self.observe_remote_realtime_event(event_id);
        let weapon_type = (source.weapon_data & 0x1f) as u8;
        let echoed_local_repel = Some(source.source_player_id)
            == self.client_authority.view().scope.player_id
            && weapon_type == 5;
        if Some(source.source_player_id) == self.client_authority.view().scope.player_id
            && !echoed_local_repel
        {
            return false;
        }
        let Some(identity) = RemoteWeaponSourceIdentity::authenticated(
            source.source_player_id,
            source.source_session_id,
            source.source_life_seq,
        ) else {
            return false;
        };
        let Some(arena_scope) =
            RemoteWeaponArenaScope::authenticated(server_session_id, arena_stream_id)
        else {
            return false;
        };
        if !(1..=8).contains(&weapon_type) {
            return false;
        }
        // A cancelled carrier must not occupy an actor-dependency slot when
        // the departing source is already absent. Committed instantaneous
        // Repel keeps its existing causal presentation semantics.
        if weapon_type != 5 && self.realtime_source_cancelled_by_owner(arena_scope, source) {
            return false;
        }
        let authority_scope = self.client_authority.view().scope;
        let actor_merge_barrier_active = authority_scope.server_session_id
            == Some(server_session_id)
            && authority_scope.arena_stream_id == Some(arena_stream_id);
        if actor_merge_barrier_active && !echoed_local_repel {
            match self.remote_weapon_presentation_admission(source) {
                RemoteWeaponPresentationAdmission::Ready => {}
                RemoteWeaponPresentationAdmission::Pending(reason) => {
                    self.enqueue_pending_remote_weapon_presentation(
                        arena_scope,
                        event_id,
                        server_tick,
                        source,
                        reason,
                    );
                    return false;
                }
                RemoteWeaponPresentationAdmission::Terminal(reason) => {
                    self.remote_weapon_presentations.mark_terminal(reason);
                    return false;
                }
            }
        }
        self.materialize_realtime_weapon_visual(
            arena_scope,
            event_id,
            server_tick,
            source,
            identity,
            echoed_local_repel,
        )
    }

    fn materialize_realtime_weapon_visual(
        &mut self,
        arena_scope: RemoteWeaponArenaScope,
        event_id: u32,
        server_tick: u32,
        source: crate::realtime_event::RealtimeWeaponEvent,
        identity: RemoteWeaponSourceIdentity,
        echoed_local_repel: bool,
    ) -> bool {
        let Some(remote_source) = self
            .capture_remote_weapon_source(
                identity,
                Some(arena_scope),
                vec2(source.x, source.y),
                vec2(source.velocity_x, source.velocity_y),
                source.orientation,
                source.frequency,
                source.ship,
            )
            .and_then(|captured| {
                captured.with_realtime_order(event_id, server_tick, source.shot_group_id)
            })
        else {
            return false;
        };
        if echoed_local_repel {
            // The firing client already predicted the mutation. Retain only
            // the authenticated causal fact so an older weapon carrier that
            // arrives later is born in the post-Repel state without applying
            // the echo to already-mutated entities a second time.
            self.remember_remote_repel_fact(remote_source);
            return false;
        }
        if source.weapon_data & 0x1f != 5 && self.remote_source_cancelled_by_owner(remote_source) {
            return false;
        }
        if let Some(player) = self.network_world.players.get_mut(&source.source_player_id) {
            let _ = player.bind_source_session(source.source_session_id, source.source_life_seq);
        }
        self.apply_remote_weapon_visual_from_source(remote_source, source.weapon_data)
    }

    /// Install or converge one exact server projectile independently of the
    /// mutable source-actor projection. This is the late-AOI path: the source
    /// life remains authenticated, but an off-screen or already-superseded
    /// sprite is not fabricated merely to make its still-live projectile
    /// visible.
    pub(in crate::game) fn apply_realtime_projectile_state(
        &mut self,
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        state: crate::realtime_event::RealtimeProjectileStateEvent,
    ) -> bool {
        self.observe_remote_realtime_event(event_id);
        let authority = self.client_authority.view().scope;
        let own_state = Some(state.source.source_player_id) == authority.player_id;
        let reconciled_own = own_state && state.weapon_data & 0x1f != 15;
        if authority.server_session_id != Some(server_session_id)
            || authority.arena_stream_id != Some(arena_stream_id)
            || (reconciled_own && state.source_input_sequence.is_none())
            || (own_state
                && self
                    .owned_projectile_life_floor
                    .is_some_and(|(server, arena, player, life)| {
                        server == server_session_id
                            && arena == arena_stream_id
                            && player == state.source.source_player_id
                            && (state.source.source_life_seq.wrapping_sub(life) as i32) < 0
                    }))
        {
            return false;
        }
        let Some(identity) = RemoteWeaponSourceIdentity::authenticated(
            state.source.source_player_id,
            state.source.source_session_id,
            state.source.source_life_seq,
        ) else {
            return false;
        };
        let Some(arena_scope) =
            RemoteWeaponArenaScope::authenticated(server_session_id, arena_stream_id)
        else {
            return false;
        };
        let Some(remote_source) = self
            .capture_remote_weapon_source(
                identity,
                Some(arena_scope),
                vec2(state.source.x, state.source.y),
                vec2(state.source.velocity_x, state.source.velocity_y),
                state.source.orientation,
                state.source.frequency,
                state.source.ship,
            )
            .and_then(|source| {
                source.with_realtime_order(
                    event_id,
                    state.source_server_tick,
                    state.source.shot_group_id,
                )
            })
        else {
            return false;
        };

        if !state.terminal && self.remote_source_cancelled_by_owner(remote_source) {
            return false;
        }
        if state.terminal {
            let weapon_type = (state.weapon_data & 0x1f) as u8;
            if !matches!(weapon_type, 1 | 2 | 3 | 4 | 7 | 8 | 15) {
                return false;
            }
            if self.remote_projectile_was_terminated(
                remote_source,
                state.weapon_data,
                state.source_projectile_index,
            ) {
                return false;
            }
            if self.networked_play
                && !own_state
                && let Some(key) = crate::remote_weapon::RemoteWeaponViewKey::from_source(
                    remote_source,
                    state.weapon_data,
                    state.source_projectile_index,
                )
            {
                self.remote_weapon_view_history.terminate(key, server_tick);
            }
            if reconciled_own {
                let _ = self.retire_authoritative_weapon_prediction(state, server_tick);
            }
            // The authenticated terminal owns removal, even if the source
            // actor is off screen, dead, or already in another life.
            self.remote_projectile_terminals
                .push_back(RemoteProjectileTerminal {
                    event_id,
                    arena_scope,
                    source_identity: identity,
                    shot_group_id: state.source.shot_group_id,
                    weapon_type,
                    child_index: state.source_projectile_index,
                });
            // Scope admission and event-id replay filtering happen in the
            // protocol owner. Evict by sequence age, never arrival order: a
            // recent terminal may arrive before a whole window of older ones.
            debug_assert!(
                self.remote_projectile_terminals.len()
                    <= crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS
            );
            let keep = |projectile: &RemoteProjectile| {
                !(projectile.source.arena_scope == remote_source.arena_scope
                    && projectile.source.identity == remote_source.identity
                    && projectile.source.shot_group_id == remote_source.shot_group_id
                    && (projectile.weapon_data & 0x1f) as u8 == weapon_type
                    && (matches!(weapon_type, 1 | 2)
                        || projectile.source_projectile_index == state.source_projectile_index))
            };
            self.remote_projectiles.retain(keep);
            self.prepared_remote_projectiles.retain(keep);
            if matches!(weapon_type, 3 | 4 | 8) {
                let tuning = self.tuning.ship(state.source.ship as usize);
                let shutdown_time = emp_bomb_shutdown_time(
                    weapon_type != 8 && tuning.emp_bomb,
                    self.tuning.emp_bomb_shutdown_seconds,
                );
                if !reconciled_own
                    || !self.reconcile_predicted_explosion(state, identity, shutdown_time)
                {
                    self.add_remote_visual_explosion(
                        vec2(state.position_x, state.position_y),
                        shutdown_time,
                        identity,
                    );
                }
            }
            return true;
        }
        if self.remote_projectile_was_terminated(
            remote_source,
            state.weapon_data,
            state.source_projectile_index,
        ) {
            return false;
        }

        if state.weapon_data & 0x1f == 6 {
            return self.apply_recovered_decoy(
                state,
                remote_source,
                server_tick,
                self.browser_server_tick()
                    .filter(|now| crate::continuum::continuum_tick_diff(*now, server_tick) >= 0)
                    .unwrap_or(server_tick),
                reconciled_own,
            );
        }

        let existing = self.remote_projectiles.iter().position(|projectile| {
            projectile
                .authority_binding
                .is_some_and(|binding| binding.projectile_id == state.projectile_id)
                || (projectile.source.identity == identity
                    && projectile.source.shot_group_id == Some(state.source.shot_group_id)
                    && projectile.weapon_data & 0x1f == state.weapon_data & 0x1f
                    && projectile.source_projectile_index == state.source_projectile_index)
        });
        if existing.is_some_and(|index| {
            recovery_precedes_applied_repel(
                remote_source.realtime_event_id,
                self.remote_projectiles[index].last_repel_mutation_event_id,
            ) || self.remote_projectiles[index]
                .authority_binding
                .is_some_and(|binding| {
                    let capture_delta =
                        crate::continuum::continuum_tick_diff(server_tick, binding.state_tick);
                    // A held group can resume within the same server tick.
                    // Capture time still rejects older poses; the scoped,
                    // wrap-safe event order distinguishes flight transitions.
                    // Ordinary same-tick recovery remains a duplicate.
                    capture_delta < 0
                        || (capture_delta == 0
                            && (self.remote_projectiles[index]
                                .presentation_clock
                                .is_some_and(|clock| clock.motion_frozen)
                                == state.motion_frozen
                                || !self.remote_projectiles[index]
                                    .source
                                    .realtime_event_id
                                    .zip(remote_source.realtime_event_id)
                                    .is_some_and(|(previous, incoming)| {
                                        realtime_event_precedes(previous, incoming)
                                    })))
                })
        }) {
            return false;
        }

        let tuning = self.tuning.ship(state.source.ship as usize);
        let mut projectile = if state.weapon_data as u8 & 0x1f == 15 {
            remote_shrapnel_projectiles(
                remote_source,
                state.source.weapon_data,
                vec2(state.position_x, state.position_y),
                0,
                &self.tuning,
            )
            .into_iter()
            .nth(usize::from(state.source_projectile_index))
        } else {
            remote_weapon_projectiles(remote_source, state.weapon_data, tuning, &self.tuning)
                .into_iter()
                .nth(usize::from(state.source_projectile_index))
        };
        let Some(mut projectile) = projectile.take() else {
            return false;
        };
        if let Some(previous) = existing.map(|index| self.remote_projectiles[index]) {
            if recovery_includes_repel_witness(
                remote_source,
                previous.source,
                previous.last_repel_mutation_event_id,
            ) {
                projectile.last_repel_mutation_event_id = previous.last_repel_mutation_event_id;
                projectile.authoritative_repel_state = previous.authoritative_repel_state;
            }
        }
        if projectile.last_repel_mutation_event_id.is_none() {
            if let Some(witness) = self.retained_repel_witness_in_recovery(
                remote_source,
                server_tick,
                crate::realtime_event::REPEL_MUTATION_PROJECTILE,
                state.weapon_data,
                state.source_projectile_index,
            ) {
                projectile.last_repel_mutation_event_id = Some(witness.event_id);
                projectile.authoritative_repel_state = Some(witness.state);
            }
        }
        projectile.source_projectile_index = state.source_projectile_index;
        projectile.authority_binding = Some(RemoteProjectileAuthorityBinding {
            projectile_id: state.projectile_id,
            state_tick: server_tick,
        });
        projectile.weapon_data = state.weapon_data;
        projectile.pos = vec2(state.position_x, state.position_y);
        projectile.vel = vec2(state.velocity_x, state.velocity_y);
        projectile.bounces_remaining = state.bounces_remaining;
        if let RemoteProjectileKind::Burst { active } = &mut projectile.kind {
            *active = state.behavior == crate::realtime_event::PROJECTILE_BEHAVIOR_BURST_ACTIVE;
        }
        let age_ticks =
            crate::continuum::continuum_tick_diff(server_tick, state.source_server_tick).max(0);
        projectile.age = crate::continuum::ticks_to_seconds_i32(age_ticks);
        if let Some(expiry_tick) = state.expiry_tick {
            projectile.ttl = crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(expiry_tick, server_tick).max(0),
            );
        }
        if matches!(projectile.kind, RemoteProjectileKind::Shrapnel { .. }) {
            // Repel restarts fragment inactivity; the immutable carrier tick
            // identifies the original shot, while its deadline owns this age.
            projectile.age = (weapon_lifetime(&self.tuning, WeaponLifetimeKind::Shrapnel)
                - projectile.ttl)
                .max(0.0);
        }
        let observed_tick = self.browser_server_tick().unwrap_or(server_tick);
        if !own_state
            && self.networked_play
            && self.door_clock_fresh
            && let Some(mut previous) = existing.map(|index| self.remote_projectiles[index])
            && let Some(clock) = previous.presentation_clock
        {
            let last_gap_tick = crate::continuum::continuum_tick_sub(server_tick, 1);
            let gap_ticks = crate::continuum::continuum_tick_diff(last_gap_tick, clock.pose_tick);
            let gap_seconds = crate::continuum::ticks_to_seconds_i32(gap_ticks);
            let incoming_seconds = crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(observed_tick, server_tick),
            );
            // Reserve the simulator's exact rounded cost, including a possible
            // tiny final substep from f32 division, rather than integer ticks.
            let incoming_steps = Self::remote_weapon_recovery_steps(incoming_seconds);
            let gap_steps = Self::remote_weapon_recovery_steps(gap_seconds);
            if (1..=20).contains(&gap_ticks)
                && gap_steps.saturating_add(incoming_steps) <= self.weapon_recovery_steps_remaining
            {
                // Fill only a proven old pose's bounded forward interval. The
                // incoming canonical capture then owns its tick and future;
                // its recovery budget is reserved before this cosmetic work.
                self.catch_up_remote_projectile_presentation_from_tick(
                    &mut previous,
                    gap_seconds,
                    Some(clock.pose_tick),
                );
            }
        }
        projectile.proximity_sensor = state
            .proximity
            .filter(|proximity| {
                authority.player_id == Some(proximity.target_player_id)
                    && authority.life_seq == Some(proximity.target_life_seq)
            })
            .map(|proximity| ProximitySensorState {
                target_id: proximity.target_player_id,
                highest_axis_offset: proximity.highest_axis_offset,
                timer: crate::continuum::ticks_to_seconds_i32(
                    crate::continuum::continuum_tick_diff(proximity.sensor_end_tick, observed_tick)
                        .max(0),
                ),
            });
        let catch_up_ticks =
            crate::continuum::continuum_tick_diff(observed_tick, server_tick).max(0);
        if state.motion_frozen {
            projectile.bind_presentation_clock(server_tick);
            projectile
                .presentation_clock
                .as_mut()
                .unwrap()
                .motion_frozen = true;
        }
        self.catch_up_remote_projectile_presentation_from_tick(
            &mut projectile,
            crate::continuum::ticks_to_seconds_i32(catch_up_ticks),
            Some(server_tick),
        );
        let pose_tick = if crate::continuum::continuum_tick_diff(observed_tick, server_tick) >= 0 {
            observed_tick
        } else {
            server_tick
        };
        projectile.bind_presentation_clock(pose_tick);
        projectile
            .presentation_clock
            .as_mut()
            .unwrap()
            .motion_frozen = state.motion_frozen;
        if let (Some(clock), Some(expiry)) = (&mut projectile.presentation_clock, state.expiry_tick)
        {
            clock.expiry_tick = expiry;
        }
        if projectile.ttl > 0.0 {
            self.record_remote_weapon_view(&projectile, pose_tick);
        }
        let retired_bomb = if reconciled_own {
            self.retire_authoritative_weapon_prediction(state, server_tick)
        } else {
            None
        };
        if projectile.ttl <= 0.0 {
            if let Some(index) = existing {
                self.remote_projectiles.remove(index);
            }
            return false;
        }
        // A newer active-state copy converges the same immutable
        // shot-group/child that the original carrier already materialized.
        // Replacing that local projection is an update, not a second logical
        // spawn. Count only a genuinely absent projectile so diagnostics and
        // browser gates remain arrival-order invariant.
        if existing.is_none() && !reconciled_own {
            self.remote_projectile_spawns = self.remote_projectile_spawns.saturating_add(1);
            match &projectile.kind {
                RemoteProjectileKind::Bomb { .. } => {
                    self.remote_bomb_spawns = self.remote_bomb_spawns.saturating_add(1);
                }
                RemoteProjectileKind::Bullet { .. } => {
                    self.remote_bullet_spawns = self.remote_bullet_spawns.saturating_add(1);
                }
                RemoteProjectileKind::Mine { .. } => {
                    self.remote_mine_spawns = self.remote_mine_spawns.saturating_add(1);
                }
                _ => {}
            }
        }
        if let Some(index) = existing {
            self.remote_projectiles.remove(index);
        }
        let first_projectile = self.remote_projectiles.len();
        self.remote_projectiles.push(projectile);
        self.apply_retained_repels_to_delayed_weapon(
            remote_source,
            first_projectile,
            self.decoys.len(),
        );
        self.apply_retained_authoritative_repels_to_delayed_weapon(remote_source);
        self.remove_fail_closed_remote_repel_entities();
        if let Some(prediction) = retired_bomb
            && let Some(installed) = self.remote_projectiles.iter().copied().find(|installed| {
                installed
                    .authority_binding
                    .is_some_and(|binding| binding.projectile_id == state.projectile_id)
                    && crate::remote_weapon::RemoteWeaponViewKey::from_projectile(installed)
                        == crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&projectile)
            })
        {
            self.begin_owner_bomb_draw_bridge(prediction, installed);
        }
        true
    }

    fn retained_repel_witness_in_recovery(
        &self,
        source: RemoteWeaponSource,
        capture_tick: u32,
        entity_kind: u8,
        weapon_data: u16,
        child_index: u8,
    ) -> Option<RemoteRepelAuthoritativeMutation> {
        let delivery = source.realtime_event_id?;
        // A fresh snapshot can be the first materialized copy after bounded
        // catch-up discarded old carriers. Attach the matching committed fact
        // without applying its old impulse to the snapshot's newer pose.
        self.remote_repel_authoritative_history
            .iter()
            .copied()
            .filter(|mutation| {
                source.arena_scope == Some(mutation.arena_scope)
                    && remote_source_matches_repel_mutation(source, mutation.state)
                    && mutation.state.entity_kind == entity_kind
                    && mutation.state.result_weapon_data == weapon_data
                    && mutation.state.source_projectile_index == child_index
                    && realtime_event_precedes(mutation.event_id, delivery)
                    && crate::continuum::continuum_tick_diff(capture_tick, mutation.server_tick)
                        >= 0
            })
            .reduce(|latest, candidate| {
                if realtime_event_precedes(latest.event_id, candidate.event_id) {
                    candidate
                } else {
                    latest
                }
            })
    }

    fn catch_up_remote_decoy_presentation(&mut self, decoy: &mut Decoy, elapsed: f32) {
        if elapsed >= decoy.ttl {
            decoy.ttl = 0.0;
            return;
        }
        let steps = (elapsed.max(0.0) * 100.0).ceil() as u32;
        if steps > 1000 || steps > self.weapon_recovery_steps_remaining {
            self.projectile_refresh_needed = true;
            decoy.ttl = 0.0;
            return;
        }
        self.weapon_recovery_steps_remaining -= steps;
        update_decoy_motion(&self.maps[self.map_index], &self.bricks, decoy, elapsed);
    }

    fn apply_recovered_decoy(
        &mut self,
        state: crate::realtime_event::RealtimeProjectileStateEvent,
        source: RemoteWeaponSource,
        capture_tick: u32,
        observed: u32,
        own: bool,
    ) -> bool {
        if state.terminal
            || state.source_projectile_index != 0
            || state.bounces_remaining != 0
            || state.proximity.is_some()
        {
            return false;
        }
        let Some(expiry) = state.expiry_tick else {
            return false;
        };
        let existing = self.decoys.iter().position(|decoy| {
            decoy.remote_source.is_some_and(|old| {
                old.identity == source.identity
                    && old.arena_scope == source.arena_scope
                    && old.shot_group_id == source.shot_group_id
            })
        });
        if existing.is_some_and(|index| {
            recovery_precedes_applied_repel(
                source.realtime_event_id,
                self.decoys[index].last_repel_mutation_event_id,
            ) || self.decoys[index].authority_binding.is_some_and(|binding| {
                crate::continuum::continuum_tick_diff(capture_tick, binding.state_tick) <= 0
            })
        }) {
            return false;
        }
        let owner = self
            .network_world
            .players
            .get(&source.identity.player_id())
            .filter(|player| {
                source.identity.matches_authenticated_runtime(
                    player.player_id,
                    player.source_session_id,
                    player.life_seq,
                )
            });
        let name = existing
            .map(|index| self.decoys[index].owner_name.clone())
            .unwrap_or_else(|| owner.map_or("", |player| player.name.as_str()).to_owned());
        let bounty = existing
            .map(|index| self.decoys[index].owner_bounty)
            .unwrap_or_else(|| owner.map_or(0, |player| u32::from(player.bounty)));
        let mut decoy = remote_decoy_from_source(source, &self.tuning, name, bounty);
        if let Some(previous) = existing.map(|index| &self.decoys[index]) {
            if recovery_includes_repel_witness(
                source,
                previous.remote_source.unwrap(),
                previous.last_repel_mutation_event_id,
            ) {
                decoy.last_repel_mutation_event_id = previous.last_repel_mutation_event_id;
                decoy.authoritative_repel_state = previous.authoritative_repel_state;
            }
        }
        if decoy.last_repel_mutation_event_id.is_none() {
            if let Some(witness) = self.retained_repel_witness_in_recovery(
                source,
                capture_tick,
                crate::realtime_event::REPEL_MUTATION_DECOY,
                state.weapon_data,
                state.source_projectile_index,
            ) {
                decoy.last_repel_mutation_event_id = Some(witness.event_id);
                decoy.authoritative_repel_state = Some(witness.state);
            }
        }
        decoy.authority_binding = Some(RemoteProjectileAuthorityBinding {
            projectile_id: state.projectile_id,
            state_tick: capture_tick,
        });
        decoy.pos = vec2(state.position_x, state.position_y);
        decoy.vel = vec2(state.velocity_x, state.velocity_y);
        decoy.age = crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(capture_tick, state.source_server_tick).max(0),
        );
        decoy.ttl = crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(expiry, capture_tick).max(0),
        );
        self.catch_up_remote_decoy_presentation(
            &mut decoy,
            crate::continuum::ticks_to_seconds_i32(
                crate::continuum::continuum_tick_diff(observed, capture_tick).max(0),
            ),
        );
        decoy.presentation_clock = Some(crate::remote_weapon::RemoteProjectileClock {
            pose_tick: observed,
            expiry_tick: expiry,
            render_partial_seconds: 0.0,
            motion_frozen: false,
        });
        if own {
            let _ = self.retire_authoritative_weapon_prediction(state, capture_tick);
        }
        if let Some(index) = existing {
            self.decoys.remove(index);
        }
        if decoy.ttl <= 0.0 {
            return false;
        }
        let first_decoy = self.decoys.len();
        self.decoys.push(decoy);
        self.apply_retained_repels_to_delayed_weapon(
            source,
            self.remote_projectiles.len(),
            first_decoy,
        );
        self.apply_retained_authoritative_repels_to_delayed_weapon(source);
        self.remove_fail_closed_remote_repel_entities();
        true
    }

    /// A realtime weapon carrier may overtake its source actor's snapshot on
    /// the independent DataChannel. Presentation opens only after the exact
    /// actor life has actually installed a renderable snapshot proxy. A newer
    /// life is terminal: a reusable numeric player id cannot resurrect the
    /// stale shot.
    fn remote_weapon_presentation_admission(
        &self,
        source: crate::realtime_event::RealtimeWeaponEvent,
    ) -> RemoteWeaponPresentationAdmission {
        let Some(player) = self.network_world.players.get(&source.source_player_id) else {
            return RemoteWeaponPresentationAdmission::Pending(
                RemoteWeaponPresentationSuppressionReason::ActorMissing,
            );
        };
        match player.life_seq {
            Some(life_seq) if life_seq == source.source_life_seq => {
                if player
                    .source_session_id
                    .is_some_and(|session_id| session_id != source.source_session_id)
                {
                    return RemoteWeaponPresentationAdmission::Terminal(
                        RemoteWeaponPresentationSuppressionReason::SessionSuperseded,
                    );
                }
                if player.ship != source.ship {
                    return RemoteWeaponPresentationAdmission::Pending(
                        RemoteWeaponPresentationSuppressionReason::ShipMismatch,
                    );
                }
                if player.last_seen_snapshot_id.is_none() {
                    return RemoteWeaponPresentationAdmission::Pending(
                        RemoteWeaponPresentationSuppressionReason::SnapshotMissing,
                    );
                }
                if player.position.is_none() {
                    return RemoteWeaponPresentationAdmission::Pending(
                        RemoteWeaponPresentationSuppressionReason::PositionMissing,
                    );
                }
                if player.display_position().is_none() {
                    return RemoteWeaponPresentationAdmission::Pending(
                        RemoteWeaponPresentationSuppressionReason::DisplayPositionMissing,
                    );
                }
                if source.weapon_data & 0x1f != 5 && self.browser_server_tick().is_none() {
                    return RemoteWeaponPresentationAdmission::Pending(
                        RemoteWeaponPresentationSuppressionReason::ServerClockUnavailable,
                    );
                }
                RemoteWeaponPresentationAdmission::Ready
            }
            Some(life_seq)
                if life_seq != 0
                    && life_seq.wrapping_sub(source.source_life_seq) < (1_u32 << 31) =>
            {
                RemoteWeaponPresentationAdmission::Terminal(
                    RemoteWeaponPresentationSuppressionReason::LifeSuperseded,
                )
            }
            _ => RemoteWeaponPresentationAdmission::Pending(
                RemoteWeaponPresentationSuppressionReason::LifeUnresolved,
            ),
        }
    }

    fn enqueue_pending_remote_weapon_presentation(
        &mut self,
        arena_scope: RemoteWeaponArenaScope,
        event_id: u32,
        server_tick: u32,
        source: crate::realtime_event::RealtimeWeaponEvent,
        suppression_reason: RemoteWeaponPresentationSuppressionReason,
    ) {
        self.remote_weapon_presentations
            .enqueue(PendingRemoteWeaponPresentation {
                key: RemoteWeaponPresentationKey {
                    arena_scope,
                    event_id,
                    source_player_id: source.source_player_id,
                    source_session_id: source.source_session_id,
                    source_life_seq: source.source_life_seq,
                },
                server_tick,
                source,
                suppression_reason,
            });
    }

    pub(in crate::game) fn drain_pending_remote_weapon_presentations(&mut self) {
        let pending_count = self.remote_weapon_presentations.len();
        for _ in 0..pending_count {
            let Some(pending) = self.remote_weapon_presentations.pop_front() else {
                break;
            };
            match self.remote_weapon_presentation_admission(pending.source) {
                RemoteWeaponPresentationAdmission::Ready => {
                    self.remote_weapon_presentations.mark_released();
                    let Some(identity) = RemoteWeaponSourceIdentity::authenticated(
                        pending.source.source_player_id,
                        pending.source.source_session_id,
                        pending.source.source_life_seq,
                    ) else {
                        self.remote_weapon_presentations.mark_terminal(
                            RemoteWeaponPresentationSuppressionReason::InvalidIdentity,
                        );
                        continue;
                    };
                    let _ = self.materialize_realtime_weapon_visual(
                        pending.key.arena_scope,
                        pending.key.event_id,
                        pending.server_tick,
                        pending.source,
                        identity,
                        false,
                    );
                }
                RemoteWeaponPresentationAdmission::Pending(reason) => {
                    self.remote_weapon_presentations.requeue(pending, reason)
                }
                RemoteWeaponPresentationAdmission::Terminal(reason) => {
                    self.remote_weapon_presentations.mark_terminal(reason);
                }
            }
        }
    }

    pub(in crate::game) fn apply_realtime_repel_mutations(
        &mut self,
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        batch: crate::realtime_event::RealtimeRepelMutationBatch,
    ) {
        self.observe_remote_realtime_event(event_id);
        let Some(arena_scope) =
            RemoteWeaponArenaScope::authenticated(server_session_id, arena_stream_id)
        else {
            return;
        };
        for &state in batch.entries() {
            let mutation = RemoteRepelAuthoritativeMutation {
                arena_scope,
                event_id,
                server_tick,
                state,
            };
            self.apply_remote_authoritative_repel_mutation(mutation);
            self.remote_repel_authoritative_history.push_back(mutation);
        }
        self.remove_fail_closed_remote_repel_entities();
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_remote_weapon_source(
        &self,
        identity: RemoteWeaponSourceIdentity,
        arena_scope: Option<RemoteWeaponArenaScope>,
        pos: Vec2,
        velocity: Vec2,
        orientation: f32,
        frequency: u16,
        ship: u8,
    ) -> Option<RemoteWeaponSource> {
        if ship >= 8 {
            return None;
        }
        let tuning = self.tuning.ship(ship as usize);
        RemoteWeaponSource::capture(
            identity,
            arena_scope,
            pos,
            velocity,
            orientation,
            frequency,
            ship,
            tuning,
            self.tuning.gravity_bombs,
        )
    }

    fn remote_projectile_was_terminated(
        &self,
        source: RemoteWeaponSource,
        weapon_data: u16,
        child_index: u8,
    ) -> bool {
        let weapon_type = (weapon_data & 0x1f) as u8;
        self.remote_projectile_terminals.iter().any(|terminal| {
            Some(terminal.arena_scope) == source.arena_scope
                && terminal.source_identity == source.identity
                && Some(terminal.shot_group_id) == source.shot_group_id
                && terminal.weapon_type == weapon_type
                && (matches!(weapon_type, 1 | 2) || terminal.child_index == child_index)
        })
    }

    fn apply_remote_weapon_visual_from_source(
        &mut self,
        source: RemoteWeaponSource,
        weapon_data: u16,
    ) -> bool {
        // A committed Repel is an earlier instantaneous impulse, not flight
        // that safety may undo; delayed causal facts must remain applicable.
        if weapon_data & 0x1f != 5 && self.remote_source_cancelled_by_owner(source) {
            return false;
        }
        let weapon_type = (weapon_data & 0x1f) as u8;
        if !(1..=8).contains(&weapon_type) {
            return false;
        }
        let tuning = self.tuning.ship(source.ship as usize);
        let pos = source.fire_position;
        if weapon_type == 6 {
            if source.shot_group_id.is_some()
                && self.decoys.iter().any(|decoy| {
                    decoy.remote_source.is_some_and(|old| {
                        old.identity == source.identity
                            && old.arena_scope == source.arena_scope
                            && old.shot_group_id == source.shot_group_id
                    })
                })
            {
                return false;
            }
            let owner = self
                .network_world
                .players
                .get(&source.identity.player_id())
                .filter(|player| {
                    source.identity.matches_authenticated_runtime(
                        player.player_id,
                        player.source_session_id,
                        player.life_seq,
                    )
                });
            let owner_name = owner.map_or("", |player| player.name.as_str()).to_owned();
            let owner_bounty = owner.map_or(0, |player| u32::from(player.bounty));
            let first_decoy = self.decoys.len();
            self.decoys.push(remote_decoy_from_source(
                source,
                &self.tuning,
                owner_name,
                owner_bounty,
            ));
            self.reconstruct_remote_weapon_presentation(
                source,
                self.remote_projectiles.len(),
                first_decoy,
            );
            #[cfg(not(test))]
            if self.decoys.len() > first_decoy {
                self.play_remote_weapon_sound(weapon_data, tuning, pos);
            }
            return false;
        }

        let first_projectile = self.remote_projectiles.len();
        let first_decoy = self.decoys.len();
        let mut projectiles = remote_weapon_projectiles(source, weapon_data, tuning, &self.tuning);
        for (index, projectile) in projectiles.iter_mut().enumerate() {
            projectile.source_projectile_index = index as u8;
        }
        projectiles.retain(|projectile| {
            !self.remote_projectile_was_terminated(
                source,
                weapon_data,
                projectile.source_projectile_index,
            ) && !self.remote_projectiles.iter().any(|existing| {
                existing.source.identity == projectile.source.identity
                    && existing.source.shot_group_id == projectile.source.shot_group_id
                    && existing.weapon_data & 0x1f == projectile.weapon_data & 0x1f
                    && existing.source_projectile_index == projectile.source_projectile_index
            })
        });
        self.remote_projectile_spawns = self
            .remote_projectile_spawns
            .saturating_add(projectiles.len() as u32);
        self.remote_bomb_spawns = self.remote_bomb_spawns.saturating_add(
            projectiles
                .iter()
                .filter(|projectile| matches!(projectile.kind, RemoteProjectileKind::Bomb { .. }))
                .count() as u32,
        );
        self.remote_bullet_spawns = self.remote_bullet_spawns.saturating_add(
            projectiles
                .iter()
                .filter(|projectile| matches!(projectile.kind, RemoteProjectileKind::Bullet { .. }))
                .count() as u32,
        );
        self.remote_mine_spawns = self.remote_mine_spawns.saturating_add(
            projectiles
                .iter()
                .filter(|projectile| matches!(projectile.kind, RemoteProjectileKind::Mine { .. }))
                .count() as u32,
        );
        self.remote_projectiles.extend(projectiles);

        if weapon_type != 5 {
            self.reconstruct_remote_weapon_presentation(source, first_projectile, first_decoy);
            if self.remote_projectiles.len() == first_projectile {
                return false;
            }
            if self.networked_play
                && source.arena_scope.is_some()
                && let Some(key) =
                    crate::remote_weapon::RemoteWeaponViewKey::from_source(source, weapon_data, 0)
            {
                for projectile in &self.remote_projectiles[first_projectile..] {
                    if let Some(child) =
                        crate::remote_weapon::RemoteWeaponViewKey::from_projectile(projectile)
                    {
                        self.remote_weapon_view_history.mark_original_carrier(child);
                    }
                }
                if self.pending_remote_birth_effects.len() < MAX_REMOTE_WEAPON_BIRTH_EFFECTS {
                    self.pending_remote_birth_effects
                        .push_back(PendingRemoteWeaponBirthEffect {
                            key: key.group_key(),
                            source,
                            weapon_data,
                        });
                } else {
                    self.remote_weapon_view_effect_drops =
                        self.remote_weapon_view_effect_drops.saturating_add(1);
                }
                return false;
            }
        }

        #[cfg(not(test))]
        self.play_remote_weapon_sound(weapon_data, tuning, pos);
        let Some((visual, visual_pos)) = remote_weapon_visual_position(
            weapon_data,
            pos,
            source.fire_orientation,
            tuning.radius_pixels,
        ) else {
            return false;
        };

        match visual {
            RemoteWeaponVisual::BombFlash => self.bomb_flashes.push(BombFlash {
                pos: visual_pos,
                age: 0.0,
                remote_source: Some(source.identity),
            }),
            RemoteWeaponVisual::Repel => {
                // This authenticated public fact owns third-party
                // presentation. The source client's echo retains only causal
                // history because its current entities were already mutated
                // by prediction; a directly affected target applies its owned
                // entities through the private fact. Observers still need the
                // server-authoritative projectile and Decoy trajectories.
                if let (Some(arena_scope), Some(event_id), Some(server_tick)) = (
                    source.arena_scope,
                    source.realtime_event_id,
                    source.realtime_server_tick,
                ) {
                    self.apply_remote_presentation_repel_fact(
                        RemoteRepelPresentationFact {
                            arena_scope,
                            event_id,
                            server_tick,
                            origin: source.fire_position,
                            source_frequency: source.frequency,
                        },
                        0,
                        0,
                        true,
                    );
                    self.remember_remote_repel_fact(source);
                } else {
                    self.apply_remote_presentation_repel_effect(visual_pos, source.frequency);
                }
                self.remote_repel_spawns = self.remote_repel_spawns.saturating_add(1);
                self.repels.push(RepelEffect {
                    pos: visual_pos,
                    age: 0.0,
                    remote_source: Some(source.identity),
                });
            }
            RemoteWeaponVisual::Burst => self.burst_effects.push(BurstEffect {
                pos: visual_pos,
                age: 0.0,
                remote_source: Some(source.identity),
            }),
        }

        // Public Repel never mutates the observer's ship or owned entities.
        // Only the target-private event may do that.
        false
    }

    #[cfg(not(test))]
    pub(in crate::game) fn play_remote_weapon_sound(
        &self,
        weapon_data: u16,
        tuning: ShipTuning,
        source_pos: Vec2,
    ) {
        let Some(sound_kind) = remote_weapon_sound(weapon_data, tuning.emp_bomb) else {
            return;
        };
        let (sound, volume) = match sound_kind {
            RemoteWeaponSoundKind::Gun(level) => (
                self.assets.gun_fire[weapon_level_index(level)].as_ref(),
                0.20,
            ),
            RemoteWeaponSoundKind::Bomb(level) => (
                self.assets.bomb_fire[weapon_level_index(level)].as_ref(),
                0.30,
            ),
            RemoteWeaponSoundKind::EmpBomb(level) => (
                self.assets.emp_bomb_fire[weapon_level_index(level)].as_ref(),
                0.30,
            ),
            RemoteWeaponSoundKind::Mine(level) => (
                self.assets.mine_fire[weapon_level_index(level)].as_ref(),
                0.28,
            ),
            RemoteWeaponSoundKind::Repel => (self.assets.repel_sound.as_ref(), 0.30),
            RemoteWeaponSoundKind::Decoy => (self.assets.decoy_sound.as_ref(), 0.26),
            RemoteWeaponSoundKind::Burst => (self.assets.burst_sound.as_ref(), 0.28),
            RemoteWeaponSoundKind::Thor => (self.assets.thor_sound.as_ref(), 0.30),
        };
        let Some(volume) = self.positional_sound_volume(source_pos, volume) else {
            return;
        };

        if let Some(sound) = sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume,
                },
            );
        }
    }

    fn apply_local_weapon_repel_effect(&mut self, origin: Vec2, remote_frequency: u16) {
        let repel_radius = self.tuning.repel_radius;
        let repel_speed = self.tuning.repel_speed;
        let local_bomb_bounces = self.ship_tuning().bomb_bounces;
        for bullet in &mut self.bullets {
            if repel_velocity(
                origin,
                &mut bullet.pos,
                &mut bullet.vel,
                repel_speed,
                repel_radius,
            ) {
                refresh_repelled_weapon_lifetime(
                    &mut bullet.ttl,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet),
                );
            }
        }

        for bomb in &mut self.bombs {
            if repel_velocity(
                origin,
                &mut bomb.pos,
                &mut bomb.vel,
                repel_speed,
                repel_radius,
            ) {
                refresh_repelled_weapon_lifetime(
                    &mut bomb.ttl,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb),
                );
            }
        }

        for mine in &mut self.mines {
            let mut vel = Vec2::ZERO;
            if repel_velocity(origin, &mut mine.pos, &mut vel, repel_speed, repel_radius) {
                self.bombs.push(Bomb {
                    prediction: mine.prediction,
                    pos: mine.pos,
                    vel,
                    frequency: mine.frequency,
                    ttl: weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb),
                    age: 0.0,
                    trail_timer: 0.0,
                    bounces_remaining: local_bomb_bounces,
                    level: mine.level,
                    damage: mine.damage,
                    emp_shutdown_time: mine.emp_shutdown_time,
                    shrapnel: mine.shrapnel,
                    proximity: mine.proximity,
                    proximity_sensor: None,
                });
                mine.ttl = 0.0;
            }
        }
        self.mines.retain(|mine| mine.ttl > 0.0);

        for shrap in &mut self.shrapnel {
            if repel_velocity(
                origin,
                &mut shrap.pos,
                &mut shrap.vel,
                repel_speed,
                repel_radius,
            ) {
                refresh_repelled_shrapnel_lifetime(
                    &mut shrap.ttl,
                    &mut shrap.age,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Shrapnel),
                );
            }
        }

        for thor in &mut self.thors {
            if repel_velocity(
                origin,
                &mut thor.pos,
                &mut thor.vel,
                repel_speed,
                repel_radius,
            ) {
                refresh_repelled_weapon_lifetime(
                    &mut thor.ttl,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Thor),
                );
            }
        }

        for decoy in &mut self.decoys {
            if decoy.remote_source.is_none()
                && repel_can_affect_frequency(remote_frequency, decoy.frequency)
            {
                if repel_velocity(
                    origin,
                    &mut decoy.pos,
                    &mut decoy.vel,
                    repel_speed,
                    repel_radius,
                ) {
                    refresh_repelled_weapon_lifetime(
                        &mut decoy.ttl,
                        weapon_lifetime(&self.tuning, WeaponLifetimeKind::Decoy),
                    );
                }
            }
        }
    }

    pub(in crate::game) fn observe_remote_realtime_event(&mut self, event_id: u32) {
        if event_id == 0 {
            return;
        }
        let advances = self.latest_remote_realtime_event_id.is_none_or(|latest| {
            let delta = event_id.wrapping_sub(latest);
            delta != 0 && delta < (1_u32 << 31)
        });
        if advances {
            self.latest_remote_realtime_event_id = Some(event_id);
        }
        let Some(latest) = self.latest_remote_realtime_event_id else {
            return;
        };
        self.remote_owner_weapon_cancellations.retain(|fact| {
            latest.wrapping_sub(fact.event_id)
                < crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32
        });
        self.remote_projectile_terminals.retain(|terminal| {
            latest.wrapping_sub(terminal.event_id)
                < crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32
        });
        self.remote_repel_presentation_history.retain(|fact| {
            latest.wrapping_sub(fact.event_id)
                < crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32
        });
        self.remote_repel_authoritative_history.retain(|mutation| {
            latest.wrapping_sub(mutation.event_id)
                < crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32
        });
        self.remote_weapon_presentations
            .expire_outside_window(latest);
    }

    fn remember_remote_repel_fact(&mut self, source: RemoteWeaponSource) {
        let (Some(arena_scope), Some(event_id), Some(server_tick)) = (
            source.arena_scope,
            source.realtime_event_id,
            source.realtime_server_tick,
        ) else {
            return;
        };
        if self
            .remote_repel_presentation_history
            .iter()
            .any(|fact| fact.arena_scope == arena_scope && fact.event_id == event_id)
        {
            return;
        }
        self.remote_repel_presentation_history
            .push_back(RemoteRepelPresentationFact {
                arena_scope,
                event_id,
                server_tick,
                origin: source.fire_position,
                source_frequency: source.frequency,
            });
    }

    /// Reconstruct only this carrier's newly admitted children in causal order.
    /// A Repel restarts its own deadline, so subtracting the original event age
    /// after replay would incorrectly shorten legitimate extensions. Instead,
    /// advance to each retained impulse/state capture, apply it at that tick,
    /// and advance the final segment to the current server clock. Exact server
    /// mutations override approximate public geometry, including a copy that
    /// locally hit terrain before the authoritative mutation arrived.
    fn reconstruct_remote_weapon_presentation(
        &mut self,
        source: RemoteWeaponSource,
        first_projectile: usize,
        first_decoy: usize,
    ) {
        let (Some(source_tick), Some(observed_tick), Some(source_event), Some(scope)) = (
            source.realtime_server_tick,
            self.browser_server_tick(),
            source.realtime_event_id,
            source.arena_scope,
        ) else {
            // Legacy packets have no authenticated clock. Keep their existing
            // semantics; retained authenticated Repels still fail closed when
            // there is no usable server clock.
            self.apply_retained_repels_to_delayed_weapon(source, first_projectile, first_decoy);
            self.apply_retained_authoritative_repels_to_delayed_weapon(source);
            self.remove_fail_closed_remote_repel_entities();
            return;
        };
        #[derive(Clone, Copy)]
        enum Impulse {
            Public(RemoteRepelPresentationFact),
            Exact(RemoteRepelAuthoritativeMutation),
        }
        let mut impulses = Vec::new();
        for &fact in &self.remote_repel_presentation_history {
            if fact.arena_scope == scope && realtime_event_precedes(source_event, fact.event_id) {
                impulses.push((fact.event_id, fact.server_tick, Impulse::Public(fact)));
            }
        }
        for &mutation in &self.remote_repel_authoritative_history {
            if mutation.arena_scope == scope
                && realtime_event_precedes(source_event, mutation.event_id)
                && remote_source_matches_repel_mutation(source, mutation.state)
            {
                impulses.push((
                    mutation.event_id,
                    mutation.server_tick,
                    Impulse::Exact(mutation),
                ));
            }
        }
        impulses.sort_by_key(|(event, _, _)| event.wrapping_sub(source_event));

        // Isolate the new children: a late carrier cannot rewind, age twice or
        // re-Repel an already materialized recovery copy or an owned Decoy.
        let new_projectiles = self.remote_projectiles.split_off(first_projectile);
        let existing_projectiles = std::mem::replace(&mut self.remote_projectiles, new_projectiles);
        let new_decoys = self.decoys.split_off(first_decoy);
        let existing_decoys = std::mem::replace(&mut self.decoys, new_decoys);
        let mut cursor = source_tick;
        for (_, tick, impulse) in impulses {
            let elapsed_ticks = crate::continuum::continuum_tick_diff(tick, cursor).max(0);
            self.advance_reconstructed_remote_weapon(
                crate::continuum::ticks_to_seconds_i32(elapsed_ticks),
                cursor,
            );
            // Multiple child mutations share the same capture tick.
            if crate::continuum::continuum_tick_diff(tick, cursor) >= 0 {
                cursor = tick;
            }
            match impulse {
                Impulse::Public(fact) => self.apply_remote_presentation_repel_mutation_at(
                    fact.origin,
                    fact.source_frequency,
                    0,
                    0,
                    Some(fact),
                    false,
                    self.door_clock_fresh.then_some(tick),
                ),
                Impulse::Exact(mutation) => {
                    self.apply_remote_authoritative_repel_mutation_at(
                        mutation,
                        self.door_clock_fresh.then_some(tick),
                    );
                    let age = crate::continuum::ticks_to_seconds_i32(
                        crate::continuum::continuum_tick_diff(tick, source_tick).max(0),
                    );
                    for projectile in &mut self.remote_projectiles {
                        if projectile.last_repel_mutation_event_id == Some(mutation.event_id)
                            && !matches!(projectile.kind, RemoteProjectileKind::Shrapnel { .. })
                        {
                            projectile.age = age;
                        }
                    }
                    for decoy in &mut self.decoys {
                        if decoy.last_repel_mutation_event_id == Some(mutation.event_id) {
                            decoy.age = age;
                        }
                    }
                }
            }
        }
        let elapsed_ticks = crate::continuum::continuum_tick_diff(observed_tick, cursor).max(0);
        self.advance_reconstructed_remote_weapon(
            crate::continuum::ticks_to_seconds_i32(elapsed_ticks),
            cursor,
        );
        let pose_tick = if crate::continuum::continuum_tick_diff(observed_tick, cursor) >= 0 {
            observed_tick
        } else {
            cursor
        };
        let view_scope = self.client_authority.view().scope;
        for projectile in &mut self.remote_projectiles {
            let previous_clock = projectile.presentation_clock;
            projectile.bind_presentation_clock(pose_tick);
            if let (Some(previous), Some(clock)) =
                (previous_clock, &mut projectile.presentation_clock)
            {
                clock.motion_frozen = previous.motion_frozen;
                clock.expiry_tick = previous.expiry_tick;
            }
            if projectile.ttl > 0.0
                && self.networked_play
                && view_scope.player_id != Some(projectile.source.identity.player_id())
            {
                self.remote_weapon_view_history
                    .record(projectile, pose_tick);
            }
        }
        for decoy in &mut self.decoys {
            decoy.presentation_clock = Some(crate::remote_weapon::RemoteProjectileClock {
                pose_tick,
                expiry_tick: crate::continuum::continuum_tick_add(
                    pose_tick,
                    (decoy.ttl.max(0.0) * 100.0).round() as u32,
                ),
                render_partial_seconds: 0.0,
                motion_frozen: false,
            });
        }
        self.remove_fail_closed_remote_repel_entities();
        let mut new_projectiles =
            std::mem::replace(&mut self.remote_projectiles, existing_projectiles);
        self.remote_projectiles.append(&mut new_projectiles);
        let mut new_decoys = std::mem::replace(&mut self.decoys, existing_decoys);
        self.decoys.append(&mut new_decoys);
    }

    fn advance_reconstructed_remote_weapon(&mut self, elapsed: f32, captured_tick: u32) {
        if elapsed <= 0.0 {
            return;
        }
        let mut projectiles = std::mem::take(&mut self.remote_projectiles);
        for projectile in &mut projectiles {
            self.catch_up_remote_projectile_presentation_from_tick(
                projectile,
                elapsed,
                Some(captured_tick),
            );
        }
        self.remote_projectiles = projectiles;
        let mut decoys = std::mem::take(&mut self.decoys);
        for decoy in &mut decoys {
            self.catch_up_remote_decoy_presentation(decoy, elapsed);
        }
        self.decoys = decoys;
    }

    fn apply_retained_repels_to_delayed_weapon(
        &mut self,
        source: RemoteWeaponSource,
        first_projectile: usize,
        first_decoy: usize,
    ) {
        let (Some(arena_scope), Some(source_event_id)) =
            (source.arena_scope, source.realtime_event_id)
        else {
            return;
        };
        let mut facts = self
            .remote_repel_presentation_history
            .iter()
            .copied()
            .filter(|fact| {
                fact.arena_scope == arena_scope
                    && realtime_event_precedes(source_event_id, fact.event_id)
            })
            .collect::<Vec<_>>();
        facts.sort_by_key(|fact| fact.event_id.wrapping_sub(source_event_id));
        for fact in facts {
            self.apply_remote_presentation_repel_fact(fact, first_projectile, first_decoy, false);
        }
    }

    fn apply_retained_authoritative_repels_to_delayed_weapon(
        &mut self,
        source: RemoteWeaponSource,
    ) {
        let Some(source_event_id) = source.realtime_event_id else {
            return;
        };
        let mut mutations = self
            .remote_repel_authoritative_history
            .iter()
            .copied()
            .filter(|mutation| {
                realtime_event_precedes(source_event_id, mutation.event_id)
                    && remote_source_matches_repel_mutation(source, mutation.state)
            })
            .collect::<Vec<_>>();
        mutations.sort_by_key(|mutation| mutation.event_id.wrapping_sub(source_event_id));
        for mutation in mutations {
            self.apply_remote_authoritative_repel_mutation(mutation);
        }
    }

    fn apply_remote_authoritative_repel_mutation(
        &mut self,
        mutation: RemoteRepelAuthoritativeMutation,
    ) {
        let observed_server_tick = self
            .door_clock_fresh
            .then(|| self.browser_server_tick())
            .flatten();
        self.apply_remote_authoritative_repel_mutation_at(mutation, observed_server_tick);
    }

    fn apply_remote_authoritative_repel_mutation_at(
        &mut self,
        mutation: RemoteRepelAuthoritativeMutation,
        observed_server_tick: Option<u32>,
    ) {
        match mutation.state.entity_kind {
            crate::realtime_event::REPEL_MUTATION_PROJECTILE => {
                let mut projectiles = std::mem::take(&mut self.remote_projectiles);
                for projectile in &mut projectiles {
                    if !remote_projectile_matches_repel_mutation(projectile, mutation.state)
                        || !remote_source_can_receive_repel_mutation(projectile.source, mutation)
                        || projectile
                            .last_repel_mutation_event_id
                            .is_some_and(|latest| {
                                !realtime_event_precedes(latest, mutation.event_id)
                            })
                    {
                        continue;
                    }
                    if mutation.state.motion_frozen && projectile.presentation_clock.is_none() {
                        projectile.bind_presentation_clock(mutation.server_tick);
                    }
                    if let Some(clock) = &mut projectile.presentation_clock {
                        clock.motion_frozen = mutation.state.motion_frozen;
                    }
                    projectile.pos = vec2(mutation.state.position_x, mutation.state.position_y);
                    projectile.vel = vec2(mutation.state.velocity_x, mutation.state.velocity_y);
                    if matches!(projectile.kind, RemoteProjectileKind::Mine { .. })
                        && mutation.state.result_weapon_data & 0x8000 == 0
                    {
                        let RemoteProjectileKind::Mine { proximity } = projectile.kind else {
                            unreachable!();
                        };
                        projectile.kind = RemoteProjectileKind::Bomb { proximity };
                        projectile.proximity_sensor = None;
                    }
                    projectile.weapon_data = mutation.state.result_weapon_data;
                    projectile.bounces_remaining = mutation.state.bounces_remaining;
                    projectile.ttl = authoritative_repel_remaining_lifetime(
                        mutation.state.expires_at_tick,
                        observed_server_tick.map(|_| mutation.server_tick),
                    );
                    if matches!(projectile.kind, RemoteProjectileKind::Shrapnel { .. }) {
                        let lifetime = weapon_lifetime(&self.tuning, WeaponLifetimeKind::Shrapnel);
                        projectile.age = (lifetime - projectile.ttl).max(0.0);
                    } else if let Some(source_tick) = projectile.source.realtime_server_tick {
                        projectile.age = crate::continuum::ticks_to_seconds_i32(
                            crate::continuum::continuum_tick_diff(
                                mutation.server_tick,
                                source_tick,
                            )
                            .max(0),
                        );
                    }
                    projectile.last_repel_mutation_event_id = Some(mutation.event_id);
                    projectile.authoritative_repel_state = Some(mutation.state);
                    if let Some(observed) = observed_server_tick {
                        let elapsed = crate::continuum::ticks_to_seconds_i32(
                            crate::continuum::continuum_tick_diff(observed, mutation.server_tick)
                                .max(0),
                        );
                        self.catch_up_remote_projectile_presentation_from_tick(
                            projectile,
                            elapsed,
                            Some(mutation.server_tick),
                        );
                        let pose_tick = if crate::continuum::continuum_tick_diff(
                            observed,
                            mutation.server_tick,
                        ) >= 0
                        {
                            observed
                        } else {
                            mutation.server_tick
                        };
                        projectile.bind_presentation_clock(pose_tick);
                        if let Some(clock) = &mut projectile.presentation_clock {
                            clock.expiry_tick = mutation.state.expires_at_tick;
                            clock.motion_frozen = mutation.state.motion_frozen;
                        }
                    }
                }
                self.remote_projectiles = projectiles;
            }
            crate::realtime_event::REPEL_MUTATION_DECOY => {
                let mut decoys = std::mem::take(&mut self.decoys);
                for decoy in &mut decoys {
                    let Some(source) = decoy.remote_source else {
                        continue;
                    };
                    if !remote_source_matches_repel_mutation(source, mutation.state)
                        || !remote_source_can_receive_repel_mutation(source, mutation)
                        || decoy.last_repel_mutation_event_id.is_some_and(|latest| {
                            !realtime_event_precedes(latest, mutation.event_id)
                        })
                    {
                        continue;
                    }
                    decoy.pos = vec2(mutation.state.position_x, mutation.state.position_y);
                    decoy.vel = vec2(mutation.state.velocity_x, mutation.state.velocity_y);
                    decoy.ttl = authoritative_repel_remaining_lifetime(
                        mutation.state.expires_at_tick,
                        observed_server_tick.map(|_| mutation.server_tick),
                    );
                    if let Some(source_tick) = source.realtime_server_tick {
                        decoy.age = crate::continuum::ticks_to_seconds_i32(
                            crate::continuum::continuum_tick_diff(
                                mutation.server_tick,
                                source_tick,
                            )
                            .max(0),
                        );
                    }
                    decoy.last_repel_mutation_event_id = Some(mutation.event_id);
                    decoy.authoritative_repel_state = Some(mutation.state);
                    if let Some(observed) = observed_server_tick {
                        self.catch_up_remote_decoy_presentation(
                            decoy,
                            crate::continuum::ticks_to_seconds_i32(
                                crate::continuum::continuum_tick_diff(
                                    observed,
                                    mutation.server_tick,
                                )
                                .max(0),
                            ),
                        );
                        decoy.presentation_clock =
                            Some(crate::remote_weapon::RemoteProjectileClock {
                                pose_tick: observed,
                                expiry_tick: mutation.state.expires_at_tick,
                                render_partial_seconds: 0.0,
                                motion_frozen: false,
                            });
                    }
                }
                self.decoys = decoys;
            }
            _ => {}
        }
    }

    fn remove_fail_closed_remote_repel_entities(&mut self) {
        self.remote_projectiles
            .retain(|projectile| projectile.ttl > 0.0);
        self.decoys
            .retain(|decoy| decoy.remote_source.is_none() || decoy.ttl > 0.0);
    }

    fn apply_remote_presentation_repel_effect(&mut self, origin: Vec2, source_frequency: u16) {
        self.apply_remote_presentation_repel_mutation(origin, source_frequency, 0, 0, None, true);
    }

    fn apply_remote_presentation_repel_fact(
        &mut self,
        fact: RemoteRepelPresentationFact,
        first_projectile: usize,
        first_decoy: usize,
        admit_legacy: bool,
    ) {
        self.apply_remote_presentation_repel_mutation(
            fact.origin,
            fact.source_frequency,
            first_projectile,
            first_decoy,
            Some(fact),
            admit_legacy,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_remote_presentation_repel_mutation(
        &mut self,
        origin: Vec2,
        source_frequency: u16,
        first_projectile: usize,
        first_decoy: usize,
        fact: Option<RemoteRepelPresentationFact>,
        admit_legacy: bool,
    ) {
        let observed_server_tick = self
            .door_clock_fresh
            .then(|| self.browser_server_tick())
            .flatten();
        self.apply_remote_presentation_repel_mutation_at(
            origin,
            source_frequency,
            first_projectile,
            first_decoy,
            fact,
            admit_legacy,
            observed_server_tick,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_remote_presentation_repel_mutation_at(
        &mut self,
        origin: Vec2,
        source_frequency: u16,
        first_projectile: usize,
        first_decoy: usize,
        fact: Option<RemoteRepelPresentationFact>,
        admit_legacy: bool,
        observed_server_tick: Option<u32>,
    ) {
        let repel_radius = self.tuning.repel_radius;
        let repel_speed = self.tuning.repel_speed;
        for projectile in self.remote_projectiles.iter_mut().skip(first_projectile) {
            if projectile
                .presentation_clock
                .is_some_and(|clock| clock.motion_frozen)
            {
                continue;
            }
            if projectile.ttl <= 0.0 {
                continue;
            }
            if fact.is_some_and(|fact| {
                exact_repel_mutation_supersedes_fact(
                    projectile.last_repel_mutation_event_id,
                    fact.event_id,
                )
            }) {
                continue;
            }
            if fact.is_some_and(|fact| {
                !remote_source_precedes_repel(projectile.source, fact, admit_legacy)
            }) {
                continue;
            }
            if !repel_can_affect_frequency(source_frequency, projectile.frequency) {
                continue;
            }
            if !repel_velocity(
                origin,
                &mut projectile.pos,
                &mut projectile.vel,
                repel_speed,
                repel_radius,
            ) {
                continue;
            }
            if let RemoteProjectileKind::Mine { proximity } = projectile.kind {
                projectile.kind = RemoteProjectileKind::Bomb { proximity };
                projectile.ttl = remote_repel_remaining_lifetime(
                    fact,
                    observed_server_tick,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb),
                );
                projectile.bounces_remaining = remote_mine_repel_bounces(projectile);
                projectile.proximity_sensor = None;
                if let (Some(clock), Some(now)) =
                    (&mut projectile.presentation_clock, observed_server_tick)
                {
                    clock.expiry_tick = crate::continuum::continuum_tick_add(
                        now,
                        (projectile.ttl * 100.0).round() as u32,
                    );
                }
                continue;
            }
            let lifetime = match projectile.kind {
                RemoteProjectileKind::Bullet { .. } | RemoteProjectileKind::Burst { .. } => {
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bullet)
                }
                RemoteProjectileKind::Shrapnel { .. } => {
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Shrapnel)
                }
                RemoteProjectileKind::Bomb { .. } | RemoteProjectileKind::Thor => {
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Bomb)
                }
                RemoteProjectileKind::Mine { .. } => {
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Mine)
                }
            };
            let remaining = remote_repel_remaining_lifetime(fact, observed_server_tick, lifetime);
            if matches!(projectile.kind, RemoteProjectileKind::Shrapnel { .. }) {
                projectile.ttl = remaining;
                projectile.age = (lifetime - remaining).max(0.0);
            } else {
                projectile.ttl = remaining;
            }
            if let (Some(clock), Some(now)) =
                (&mut projectile.presentation_clock, observed_server_tick)
            {
                clock.expiry_tick = crate::continuum::continuum_tick_add(
                    now,
                    (projectile.ttl * 100.0).round() as u32,
                );
            }
        }

        for decoy in self.decoys.iter_mut().skip(first_decoy) {
            if decoy.ttl <= 0.0 {
                continue;
            }
            let Some(source) = decoy.remote_source else {
                continue;
            };
            if fact.is_some_and(|fact| {
                exact_repel_mutation_supersedes_fact(
                    decoy.last_repel_mutation_event_id,
                    fact.event_id,
                ) || !remote_source_precedes_repel(source, fact, admit_legacy)
            }) || !repel_can_affect_frequency(source_frequency, decoy.frequency)
            {
                continue;
            }
            if repel_velocity(
                origin,
                &mut decoy.pos,
                &mut decoy.vel,
                repel_speed,
                repel_radius,
            ) {
                decoy.ttl = remote_repel_remaining_lifetime(
                    fact,
                    observed_server_tick,
                    weapon_lifetime(&self.tuning, WeaponLifetimeKind::Decoy),
                );
                if let (Some(clock), Some(now)) =
                    (&mut decoy.presentation_clock, observed_server_tick)
                {
                    clock.expiry_tick = crate::continuum::continuum_tick_add(
                        now,
                        (decoy.ttl * 100.0).round() as u32,
                    );
                }
            }
        }
    }
}

fn realtime_event_precedes(earlier: u32, later: u32) -> bool {
    let distance = later.wrapping_sub(earlier);
    distance != 0 && distance < (1_u32 << 31)
}

fn exact_repel_mutation_supersedes_fact(
    last_exact_mutation_event_id: Option<u32>,
    fact_event_id: u32,
) -> bool {
    last_exact_mutation_event_id.is_some_and(|exact_event_id| {
        exact_event_id == fact_event_id || realtime_event_precedes(fact_event_id, exact_event_id)
    })
}

fn remote_source_precedes_repel(
    source: RemoteWeaponSource,
    fact: RemoteRepelPresentationFact,
    admit_legacy: bool,
) -> bool {
    match (source.arena_scope, source.realtime_event_id) {
        (Some(arena_scope), Some(event_id)) => {
            arena_scope == fact.arena_scope && realtime_event_precedes(event_id, fact.event_id)
        }
        (None, None) => admit_legacy,
        _ => false,
    }
}

fn remote_source_matches_repel_mutation(
    source: RemoteWeaponSource,
    state: crate::realtime_event::RealtimeRepelMutationState,
) -> bool {
    matches!(
        source.identity,
        RemoteWeaponSourceIdentity::Authenticated {
            player_id,
            session_id,
            life_seq,
        } if player_id == state.source_player_id
            && session_id == state.source_session_id
            && life_seq == state.source_life_seq
    ) && source.realtime_server_tick == Some(state.source_server_tick)
}

// Recovery captures already include every earlier committed impulse. A
// reordered old mutation must not rewind that newer state; original carriers
// still admit all causally later Repels, including lifetime extensions.
fn recovery_includes_repel_witness(
    recovery: RemoteWeaponSource,
    previous: RemoteWeaponSource,
    mutation_event: Option<u32>,
) -> bool {
    // Retain the witness, not its old pose/velocity/deadline. A recovery that
    // precedes the impulse must start without that witness so retained replay
    // still applies the later mutation to its freshly installed state.
    recovery.identity == previous.identity
        && recovery.arena_scope == previous.arena_scope
        && recovery.shot_group_id == previous.shot_group_id
        && recovery.realtime_server_tick == previous.realtime_server_tick
        && mutation_event
            .zip(recovery.realtime_event_id)
            .is_some_and(|(mutation, snapshot)| realtime_event_precedes(mutation, snapshot))
}

fn recovery_precedes_applied_repel(
    recovery_event: Option<u32>,
    applied_mutation: Option<u32>,
) -> bool {
    // An already-applied newer impulse supersedes older captures. Reject
    // them before catch-up work, which could otherwise exhaust the frame's
    // budget and remove the valid, newer projection during a reordered burst.
    applied_mutation
        .zip(recovery_event)
        .is_some_and(|(mutation, recovery)| !realtime_event_precedes(mutation, recovery))
}

fn remote_source_can_receive_repel_mutation(
    source: RemoteWeaponSource,
    mutation: RemoteRepelAuthoritativeMutation,
) -> bool {
    source.arena_scope == Some(mutation.arena_scope)
        && source
            .realtime_event_id
            .is_some_and(|event_id| realtime_event_precedes(event_id, mutation.event_id))
}

fn remote_projectile_matches_repel_mutation(
    projectile: &RemoteProjectile,
    state: crate::realtime_event::RealtimeRepelMutationState,
) -> bool {
    remote_source_matches_repel_mutation(projectile.source, state)
        && projectile.source_projectile_index == state.source_projectile_index
        && projectile.weapon_data == state.source_weapon_data
}

fn remote_repel_remaining_lifetime(
    fact: Option<RemoteRepelPresentationFact>,
    observed_server_tick: Option<u32>,
    lifetime: f32,
) -> f32 {
    let Some(fact) = fact else {
        return lifetime;
    };
    let Some(observed_server_tick) = observed_server_tick else {
        return 0.0;
    };
    let deadline = crate::continuum::continuum_tick_add(
        fact.server_tick,
        crate::continuum::seconds_to_ticks_u32(lifetime),
    );
    crate::continuum::ticks_to_seconds_i32(
        crate::continuum::continuum_tick_diff(deadline, observed_server_tick).max(0),
    )
    .min(lifetime)
}

fn authoritative_repel_remaining_lifetime(
    expires_at_tick: u32,
    observed_server_tick: Option<u32>,
) -> f32 {
    let Some(observed_server_tick) = observed_server_tick else {
        return 0.0;
    };
    crate::continuum::ticks_to_seconds_i32(
        crate::continuum::continuum_tick_diff(expires_at_tick, observed_server_tick).max(0),
    )
}

fn remote_decoy_from_source(
    source: RemoteWeaponSource,
    arena: &ArenaTuning,
    owner_name: String,
    owner_bounty: u32,
) -> Decoy {
    Decoy {
        authority_binding: None,
        presentation_clock: None,
        prediction: None,
        pos: source.fire_position,
        vel: source.fire_velocity,
        frequency: source.frequency,
        orientation: source.fire_orientation,
        initial_orientation: source.fire_orientation,
        remote_source: Some(source),
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        ship_type: source.ship as usize,
        owner_name,
        owner_bounty,
        ttl: weapon_lifetime(arena, WeaponLifetimeKind::Decoy),
        age: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_realtime_authority_scope(game: &mut Game) {
        install_realtime_authority_scope_with_life(game, 3);
    }

    fn install_realtime_authority_scope_with_life(game: &mut Game, life_seq: u32) {
        // A known clock before these synthetic source events keeps existing
        // admission fixtures focused on their actor/lifecycle barrier.
        game.last_observed_browser_server_tick.get_or_insert(0);
        game.install_network_authority_test_scope(
            AuthorityScopeIdentity {
                server_session_id: Some(77),
                arena_stream_id: Some(88),
                player_id: Some(9),
                life_seq: Some(life_seq),
                alive: Some(true),
            },
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(88),
                settings: true,
                security: true,
                map: true,
            },
            88,
        );
    }

    fn remote_presentation_actor(player_id: u16, life_seq: u32) -> NetworkPlayer {
        let mut player = NetworkPlayer::from_roster(&PlayerRosterEntry {
            ship: 0,
            audio: 0,
            name: "source".to_owned(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id,
            frequency: 1,
            wins: 0,
            losses: 0,
            attach_parent: INVALID_PLAYER_ID,
            flags: 0,
            koth: 0,
        });
        player.set_life_seq(Some(life_seq));
        player.position = Some(vec2(115.0, 100.0));
        player.render_position = player.position;
        player.last_seen_snapshot_id = Some(41);
        player
    }

    fn realtime_weapon(
        source_player_id: u16,
        source_frequency: u16,
        weapon_type: u8,
    ) -> crate::realtime_event::RealtimeWeaponEvent {
        crate::realtime_event::RealtimeWeaponEvent {
            source_player_id,
            source_session_id: u64::from(source_player_id) + 100,
            source_life_seq: 7,
            shot_group_id: 9,
            x: if source_player_id == 1 { 115.0 } else { 100.0 },
            y: 100.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            frequency: source_frequency,
            ship: 0,
            weapon_data: encode_weapon_data(
                weapon_type,
                WEAPON_LEVEL_MIN,
                false,
                WEAPON_LEVEL_MIN,
                0,
                false,
            ),
        }
    }

    fn realtime_projectile_state(
        source: crate::realtime_event::RealtimeWeaponEvent,
        projectile_id: u32,
        state_tick: u32,
    ) -> crate::realtime_event::RealtimeProjectileStateEvent {
        crate::realtime_event::RealtimeProjectileStateEvent {
            source_input_sequence: None,
            terminal: false,
            motion_frozen: false,
            source,
            projectile_id,
            source_server_tick: 110,
            source_projectile_index: 0,
            weapon_data: source.weapon_data,
            position_x: 130.0 + (i64::from(state_tick) - 120) as f32,
            position_y: 100.0,
            velocity_x: 100.0,
            velocity_y: 0.0,
            expiry_tick: Some(500),
            behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
            bounces_remaining: 0,
            proximity: None,
        }
    }

    fn pending_remote_weapon(
        event_id: u32,
        source: crate::realtime_event::RealtimeWeaponEvent,
        suppression_reason: RemoteWeaponPresentationSuppressionReason,
    ) -> PendingRemoteWeaponPresentation {
        PendingRemoteWeaponPresentation {
            key: RemoteWeaponPresentationKey {
                arena_scope: RemoteWeaponArenaScope::authenticated(77, 88).unwrap(),
                event_id,
                source_player_id: source.source_player_id,
                source_session_id: source.source_session_id,
                source_life_seq: source.source_life_seq,
            },
            server_tick: 110,
            source,
            suppression_reason,
        }
    }

    fn install_owned_lifetime_bullet(game: &mut Game) {
        game.bullets.push(Bullet {
            prediction: None,
            pos: vec2(200.0, 200.0),
            vel: vec2(100.0, 0.0),
            frequency: 1,
            ttl: 1.5,
            age: 0.5,
            trail_timer: 0.0,
            bounces_remaining: 0,
            bouncing: false,
            activates_on_wall: false,
            active: true,
            level: 1,
            damage: 100.0,
        });
    }

    fn install_predicted_own_bullet(game: &mut Game, life: u32, sequence: u16, index: u8) {
        install_owned_lifetime_bullet(game);
        game.bullets.last_mut().unwrap().prediction =
            Some(crate::model::OwnedProjectilePrediction {
                activation: u64::from(sequence) + 1,
                life_seq: life,
                input_sequence: Some(sequence),
                projectile_index: index,
            });
    }

    fn own_projectile_state(sequence: u16) -> crate::realtime_event::RealtimeProjectileStateEvent {
        let mut source = realtime_weapon(9, 1, 2);
        source.source_life_seq = 3;
        let mut state = realtime_projectile_state(source, 900, 120);
        state.source_input_sequence = Some(sequence);
        state
    }

    #[test]
    fn owner_reconciliation_replaces_each_barrel_once_and_terminal_retires_the_group() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.tuning.ships[0].double_barrel = true;
        install_predicted_own_bullet(&mut game, 3, 42, 0);
        install_predicted_own_bullet(&mut game, 3, 42, 1);
        install_predicted_own_bullet(&mut game, 3, 43, 0);
        let first = own_projectile_state(42);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, first));
        assert_eq!((game.bullets.len(), game.remote_projectiles.len()), (2, 1));
        let second = crate::realtime_event::RealtimeProjectileStateEvent {
            projectile_id: 901,
            source_projectile_index: 1,
            ..first
        };
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 120, second));
        assert_eq!((game.bullets.len(), game.remote_projectiles.len()), (1, 2));
        assert_eq!(game.owned_weapon_counts()[0], 3);
        assert_eq!(game.owned_projectile_reconciliations, 2);
        assert_eq!(
            game.remote_bullet_spawns, 0,
            "owner recovery must not count as enemy firing proof"
        );
        assert!(!game.apply_realtime_projectile_state(77, 88, 3, 120, first));
        assert!(game.apply_realtime_projectile_state(77, 88, 4, 121, first));
        assert_eq!(game.remote_projectiles.len(), 2);
        let terminal = crate::realtime_event::RealtimeProjectileStateEvent {
            terminal: true,
            motion_frozen: false,
            ..first
        };
        assert!(game.apply_realtime_projectile_state(77, 88, 5, 122, terminal));
        assert!(game.remote_projectiles.is_empty());
        assert_eq!(game.bullets.len(), 1);
        assert_eq!(game.bullets[0].prediction.unwrap().input_sequence, Some(43));
        assert!(!game.apply_realtime_projectile_state(77, 88, 6, 120, second));
    }

    #[test]
    fn owner_reconciliation_terminal_before_state_cancels_exact_prediction_without_resurrection() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        for index in 0..2 {
            install_predicted_own_bullet(&mut game, 3, u16::MAX, index);
        }
        let state = own_projectile_state(u16::MAX);
        assert!(game.apply_realtime_projectile_state(
            77,
            88,
            2,
            122,
            crate::realtime_event::RealtimeProjectileStateEvent {
                terminal: true,
                motion_frozen: false,
                ..state
            }
        ));
        assert!(game.bullets.is_empty());
        assert!(!game.apply_realtime_projectile_state(77, 88, 1, 120, state));
        assert!(game.remote_projectiles.is_empty());
    }

    #[test]
    fn owner_reconciliation_survives_respawn_but_ship_cutover_rejects_old_life_recovery() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        install_predicted_own_bullet(&mut game, 3, 0, 0);
        game.reset_ship_after_death();
        install_realtime_authority_scope_with_life(&mut game, 4);
        let state = own_projectile_state(0);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, state));
        assert!(game.bullets.is_empty());
        assert_eq!(game.remote_projectiles.len(), 1);
        game.clear_local_weapons();
        assert!(game.remote_projectiles.is_empty());
        assert!(!game.apply_realtime_projectile_state(
            77,
            88,
            2,
            121,
            crate::realtime_event::RealtimeProjectileStateEvent {
                projectile_id: 901,
                ..state
            }
        ));
        let mut current = state;
        current.source.source_life_seq = 4;
        assert!(game.apply_realtime_projectile_state(77, 88, 3, 122, current));
        let mut legacy = current;
        legacy.source_input_sequence = None;
        assert!(!game.apply_realtime_projectile_state(77, 88, 4, 123, legacy));
        install_realtime_authority_scope_with_life(&mut game, 1);
        game.clear_local_weapons();
        current.source.source_life_seq = u32::MAX;
        assert!(!game.apply_realtime_projectile_state(77, 88, 5, 124, current));
        current.source.source_life_seq = 1;
        assert!(game.apply_realtime_projectile_state(77, 88, 6, 125, current));
    }

    fn owner_bomb_handoff_fixture(
        one_way_ticks: u32,
        ship_speed: f32,
    ) -> (
        Game,
        Bomb,
        RemoteProjectile,
        crate::realtime_event::RealtimeProjectileStateEvent,
    ) {
        let mut game = Game::new_network_authority_test();
        game.tuning = arena_tuning_from_subgame_settings_text(include_str!(
            "../../../assets/zones/chaos-zone/arena.conf"
        ))
        .unwrap();
        install_realtime_authority_scope(&mut game);
        game.networked_play = true;
        game.last_observed_browser_server_tick = Some(100 + one_way_ticks * 2);
        let heading = vec2(0.0, -1.0);
        let tuning = game.tuning.ship(0);
        let launch = vec2(4000.0, 4000.0);
        let source_velocity =
            bomb_recoil_velocity(heading * ship_speed, heading, tuning.bomb_recoil_speed);
        let velocity = source_velocity + heading * tuning.bomb_speed;
        let local_age = one_way_ticks as f32 * 0.02;
        let mut prediction = Bomb {
            prediction: Some(crate::model::OwnedProjectilePrediction {
                activation: 1,
                life_seq: 3,
                input_sequence: Some(7),
                projectile_index: 0,
            }),
            pos: launch,
            vel: velocity,
            frequency: 1,
            ttl: weapon_lifetime(&game.tuning, WeaponLifetimeKind::Bomb),
            age: 0.0,
            trail_timer: 0.0,
            bounces_remaining: 0,
            level: 1,
            damage: 0.0,
            emp_shutdown_time: 0.0,
            shrapnel: ExplosiveShrapnel::none(),
            proximity: false,
            proximity_sensor: None,
        };
        assert!(
            update_bomb_motion(
                &mut prediction,
                local_age,
                game.map(),
                &[],
                &[],
                &NetworkWorld::default(),
                1,
                game.tuning,
                game.tuning.gravity_bombs.then_some(tuning.wormhole_gravity),
            )
            .is_none()
        );
        assert!(prediction.ttl > 0.0);
        game.bombs.push(prediction);
        let mut state = own_projectile_state(7);
        state.source.weapon_data = 3;
        state.source.x = launch.x;
        state.source.y = launch.y;
        state.source.velocity_x = source_velocity.x;
        state.source.velocity_y = source_velocity.y;
        state.source.orientation = 0.0;
        state.weapon_data = 3;
        state.position_x = launch.x;
        state.position_y = launch.y;
        state.velocity_x = velocity.x;
        state.velocity_y = velocity.y;
        state.source_server_tick = 100 + one_way_ticks;
        state.expiry_tick = Some(8100 + one_way_ticks);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 100 + one_way_ticks, state));
        assert!(game.bombs.is_empty());
        assert_eq!(game.remote_projectiles.len(), 1);
        game.prepare_remote_weapon_views(None);
        let confirmed = game.prepared_remote_projectiles[0];
        (game, prediction, confirmed, state)
    }

    #[test]
    fn owner_bomb_handoff_keeps_forward_draw_at_all_chaos_speeds() {
        // A native reproduction, not a claim about the unrecorded prod pixels.
        // Input is sampled at tick 100, accepted at 110, delivered at 115.
        // Both sides use the SAME launch position and post-recoil velocity.
        let mut backwards = Vec::new();
        for one_way_ticks in [1_u32, 3, 10] {
            for ship_speed in [0.0, 100.0, 200.0, 325.0] {
                let (mut game, prediction, confirmed, _) =
                    owner_bomb_handoff_fixture(one_way_ticks, ship_speed);
                let heading = vec2(0.0, -1.0);
                let predicted_position = prediction.pos;
                let presentation = game.remote_projectile_draw_presentation(&confirmed, 0.0);
                assert!(presentation.animation_seconds + 0.0001 >= prediction.age);
                assert_eq!(
                    game.remote_projectiles[0], confirmed,
                    "draw sampling cannot mutate canonical state"
                );
                let mut last_draw = presentation;
                for elapsed_ticks in 1..=30 {
                    let elapsed = elapsed_ticks as f32 / 100.0;
                    let mut canonical = confirmed;
                    canonical.pos += canonical.vel * elapsed;
                    canonical.age += elapsed;
                    canonical.ttl -= elapsed;
                    canonical.presentation_clock.as_mut().unwrap().pose_tick += elapsed_ticks;
                    game.remote_projectiles[0] = canonical;
                    game.prepare_remote_weapon_views(None);
                    let drawn = game.remote_projectile_draw_presentation(
                        &game.prepared_remote_projectiles[0],
                        0.0,
                    );
                    assert!((drawn.position - last_draw.position).dot(heading) >= -0.001);
                    assert!(drawn.animation_seconds >= last_draw.animation_seconds);
                    assert_eq!(game.remote_projectiles[0], canonical);
                    if elapsed_ticks >= 25 {
                        assert_eq!(
                            drawn,
                            remote_projectile_presentation(
                                &canonical,
                                0.0,
                                game.map(),
                                &[],
                                &game.tuning
                            )
                        );
                        assert!(game.owner_bomb_draw_bridges.is_empty());
                    }
                    last_draw = drawn;
                }
                let forward_step = (presentation.position - predicted_position).dot(heading);
                eprintln!(
                    "owner-bomb-handoff one_way_ticks={one_way_ticks} speed={ship_speed} forward_step_px={forward_step} predicted_y={} confirmed_y={} confirmed_age={}",
                    predicted_position.y, presentation.position.y, confirmed.age
                );
                if forward_step < -0.01 {
                    backwards.push((one_way_ticks, ship_speed, forward_step));
                }
            }
        }
        assert!(
            backwards.is_empty(),
            "prediction handoff moved backward: {backwards:?}"
        );
    }

    #[test]
    fn owner_bomb_handoff_never_carries_a_lead_across_authoritative_cuts() {
        for cut in [
            "frozen",
            "repel",
            "bounce",
            "velocity",
            "position",
            "expiry",
            "future-clock",
            "foreign-identity",
        ] {
            let (mut game, _, canonical, _) = owner_bomb_handoff_fixture(3, 200.0);
            assert_eq!(game.owner_bomb_draw_bridges.len(), 1);
            let mut changed = canonical;
            match cut {
                "frozen" => changed.presentation_clock.as_mut().unwrap().motion_frozen = true,
                "repel" => changed.last_repel_mutation_event_id = Some(9),
                "bounce" => changed.bounces_remaining += 1,
                "velocity" => changed.vel.x += 10.0,
                "position" => changed.pos.x += 2.0,
                "expiry" => changed.ttl = 0.001,
                "future-clock" => changed.presentation_clock.as_mut().unwrap().pose_tick -= 1,
                "foreign-identity" => {
                    changed.source.identity = RemoteWeaponSourceIdentity::Authenticated {
                        player_id: 10,
                        session_id: 1,
                        life_seq: 3,
                    }
                }
                _ => unreachable!(),
            }
            let expected = remote_projectile_presentation(
                &changed,
                0.0,
                game.map(),
                &game.bricks,
                &game.tuning,
            );
            assert_eq!(
                game.remote_projectile_draw_presentation(&changed, 0.0),
                expected,
                "{cut}"
            );
            game.remote_projectiles[0] = changed;
            game.prepare_remote_weapon_views(None);
            assert!(
                game.owner_bomb_draw_bridges.is_empty(),
                "cut must permanently retire the lead: {cut}"
            );
            assert_eq!(game.remote_projectiles[0], changed);
        }
    }

    #[test]
    fn owner_bomb_handoff_rejects_large_or_inconsistent_prediction_and_ends_at_scope_retirement() {
        for invalid in ["delay", "lateral", "velocity", "life", "age"] {
            let (mut game, mut prediction, canonical, _) = owner_bomb_handoff_fixture(3, 200.0);
            game.owner_bomb_draw_bridges.clear();
            match invalid {
                "delay" => {
                    prediction.age = canonical.age + 0.21;
                    prediction.pos = canonical.pos + canonical.vel * 0.21;
                }
                "lateral" => prediction.pos.x += 2.0,
                "velocity" => prediction.vel.x += 10.0,
                "life" => prediction.prediction.as_mut().unwrap().life_seq += 1,
                "age" => prediction.age = f32::NAN,
                _ => unreachable!(),
            }
            game.begin_owner_bomb_draw_bridge(prediction, canonical);
            assert!(game.owner_bomb_draw_bridges.is_empty(), "{invalid}");
        }
        let (mut game, _, canonical, _) = owner_bomb_handoff_fixture(3, 200.0);
        install_realtime_authority_scope_with_life(&mut game, 4);
        game.prepare_remote_weapon_views(None);
        assert!(game.owner_bomb_draw_bridges.is_empty());
        assert_eq!(
            game.remote_projectiles[0], canonical,
            "life change cannot truncate the weapon's own lifetime"
        );
        assert_eq!(
            game.remote_projectile_draw_presentation(&canonical, 0.0),
            remote_projectile_presentation(&canonical, 0.0, game.map(), &game.bricks, &game.tuning)
        );
    }

    #[test]
    fn owner_bomb_handoff_respects_bricks_and_terminal_without_visual_resurrection() {
        let (mut game, _, canonical, mut terminal) = owner_bomb_handoff_fixture(10, 325.0);
        let ahead = canonical.pos + canonical.vel * 0.025;
        game.bricks.push(Brick {
            tile: ivec2(
                (ahead.x / 16.0).floor() as i32,
                (ahead.y / 16.0).floor() as i32,
            ),
            previous_tile: 0,
            team: canonical.frequency.wrapping_add(1),
            ttl: 10.0,
            age: 0.0,
            server_timestamp: None,
        });
        let brick_tile = game.bricks.last().unwrap().tile;
        game.maps[game.map_index].set_tile(brick_tile.x, brick_tile.y, BRICK_TILE_ID);
        assert!(is_solid_world_for_frequency(
            game.map(),
            ahead,
            canonical.frequency,
            &game.bricks
        ));
        assert_eq!(
            game.remote_projectile_draw_presentation(&canonical, 0.0),
            remote_projectile_presentation(&canonical, 0.0, game.map(), &game.bricks, &game.tuning)
        );
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 120, terminal));
        game.prepare_remote_weapon_views(None);
        assert!(game.remote_projectiles.is_empty());
        assert!(game.prepared_remote_projectiles.is_empty());
        assert!(game.owner_bomb_draw_bridges.is_empty());
        let (mut game, _, _, _) = owner_bomb_handoff_fixture(3, 100.0);
        game.clear_remote_weapon_presentation();
        assert!(game.owner_bomb_draw_bridges.is_empty());
    }

    #[test]
    fn owner_reconciliation_explosives_keep_one_copy_and_terminal_removes_it() {
        for (weapon, slot) in [(3, 1), (3 | (1 << 15), 2), (8, 4)] {
            let mut game = Game::new_network_authority_test();
            install_realtime_authority_scope(&mut game);
            let prediction = Some(crate::model::OwnedProjectilePrediction {
                activation: 1,
                life_seq: 3,
                input_sequence: Some(7),
                projectile_index: 0,
            });
            if slot == 1 {
                game.bombs.push(Bomb {
                    prediction,
                    pos: vec2(120.0, 100.0),
                    vel: Vec2::ZERO,
                    frequency: 1,
                    ttl: 5.0,
                    age: 0.0,
                    trail_timer: 0.0,
                    bounces_remaining: 0,
                    level: 1,
                    damage: 100.0,
                    emp_shutdown_time: 0.0,
                    shrapnel: ExplosiveShrapnel::none(),
                    proximity: false,
                    proximity_sensor: None,
                });
            } else if slot == 2 {
                game.mines.push(Mine {
                    vel: Vec2::ZERO,
                    bomb_gravity: None,
                    prediction,
                    pos: vec2(120.0, 100.0),
                    frequency: 1,
                    ttl: 5.0,
                    age: 0.0,
                    level: 1,
                    damage: 100.0,
                    emp_shutdown_time: 0.0,
                    shrapnel: ExplosiveShrapnel::none(),
                    proximity: false,
                    proximity_sensor: None,
                });
            } else {
                game.thors.push(Thor {
                    prediction,
                    pos: vec2(120.0, 100.0),
                    vel: Vec2::ZERO,
                    ttl: 5.0,
                    age: 0.0,
                    damage: 100.0,
                    shrapnel: ExplosiveShrapnel::none(),
                    proximity_sensor: None,
                    raw_level: 0,
                });
            }
            let mut state = own_projectile_state(7);
            state.weapon_data = weapon;
            state.source.weapon_data = weapon;
            assert_eq!(game.owned_weapon_counts()[slot], 1);
            assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, state));
            assert!(game.bombs.is_empty() && game.mines.is_empty() && game.thors.is_empty());
            assert_eq!(game.owned_weapon_counts()[slot], 1);
            assert!(game.apply_realtime_projectile_state(
                77,
                88,
                2,
                121,
                crate::realtime_event::RealtimeProjectileStateEvent {
                    terminal: true,
                    motion_frozen: false,
                    ..state
                }
            ));
            assert!(game.remote_projectiles.is_empty());
            assert_eq!(game.owned_weapon_counts()[slot], 0);
        }
    }

    #[test]
    fn safe_owner_cancellation_is_silent_and_blocks_delayed_flight_but_not_future_groups_or_sessions()
     {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 2, 3);
        let state = realtime_projectile_state(source, 900, 120);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, state));
        let decoy_source = game
            .capture_remote_weapon_source(
                RemoteWeaponSourceIdentity::authenticated(1, 101, 7).unwrap(),
                RemoteWeaponArenaScope::authenticated(77, 88),
                vec2(100.0, 100.0),
                Vec2::ZERO,
                0.0,
                2,
                0,
            )
            .unwrap()
            .with_realtime_order(2, 110, 10)
            .unwrap();
        game.decoys.push(remote_decoy_from_source(
            decoy_source,
            &game.tuning,
            "old".to_owned(),
            30,
        ));
        let cancellation = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 1,
            source_session_id: 101,
            through_shot_group_id: 10,
            owner_life_seq: 8,
            owner_input_sequence: Some(42),
        };
        assert!(game.apply_owner_weapon_cancellation(77, 88, 3, cancellation));
        assert!(
            game.remote_projectiles.is_empty()
                && game.decoys.is_empty()
                && game.explosions.is_empty()
        );
        assert!(!game.apply_realtime_projectile_state(77, 88, 2, 120, state));
        assert!(!game.apply_remote_weapon_visual_from_source(decoy_source, 6));
        assert!(game.decoys.is_empty());
        let mut future = state;
        future.source.shot_group_id = 11;
        future.projectile_id = 901;
        assert!(game.apply_realtime_projectile_state(77, 88, 4, 123, future));
        let mut replacement = state;
        replacement.source.source_session_id = 102;
        replacement.projectile_id = 902;
        assert!(game.apply_realtime_projectile_state(77, 88, 5, 124, replacement));
        assert_eq!(game.remote_projectiles.len(), 2);
        assert!(!game.apply_owner_weapon_cancellation(77, 89, 6, cancellation));
        assert_eq!(game.remote_projectiles.len(), 2);
        let _ = game.apply_remote_weapon_visual_from_source(decoy_source, 5);
        assert_eq!(
            game.remote_repel_presentation_history.len(),
            1,
            "source safety cannot undo a previously committed Repel impulse"
        );
        // An already-frozen earlier explosion remains a legitimate terminal.
        assert!(game.apply_realtime_projectile_state(
            77,
            88,
            6,
            125,
            crate::realtime_event::RealtimeProjectileStateEvent {
                terminal: true,
                motion_frozen: false,
                ..state
            }
        ));
        assert_eq!(game.explosions.len(), 1);
        assert_eq!(game.remote_projectiles.len(), 2);
    }

    #[test]
    fn departure_cutoff_after_realtime_expiry_clears_exact_source_without_retaining_old_facts() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let original = realtime_weapon(1, 2, 3);
        for (index, source) in [
            original,
            crate::realtime_event::RealtimeWeaponEvent {
                shot_group_id: 11,
                ..original
            },
            crate::realtime_event::RealtimeWeaponEvent {
                source_session_id: 201,
                ..original
            },
        ]
        .into_iter()
        .enumerate()
        {
            let state = realtime_projectile_state(source, 900 + index as u32, 120);
            assert!(game.apply_realtime_projectile_state(77, 88, index as u32 + 1, 120, state));
        }
        let source = game
            .capture_remote_weapon_source(
                RemoteWeaponSourceIdentity::authenticated(1, 101, 7).unwrap(),
                RemoteWeaponArenaScope::authenticated(77, 88),
                vec2(100.0, 100.0),
                Vec2::ZERO,
                0.0,
                2,
                0,
            )
            .unwrap()
            .with_realtime_order(4, 110, 9)
            .unwrap();
        game.decoys.push(remote_decoy_from_source(
            source,
            &game.tuning,
            "old".to_owned(),
            30,
        ));
        game.observe_remote_realtime_event(6000);
        let cutoff = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 1,
            source_session_id: 101,
            through_shot_group_id: 10,
            owner_life_seq: 8,
            owner_input_sequence: None,
        };
        for _ in 0..2 {
            assert!(game.apply_owner_weapon_cancellation(77, 88, 5, cutoff));
            assert_eq!(game.remote_projectiles.len(), 2);
            assert!(game.decoys.is_empty() && game.explosions.is_empty());
            assert!(
                game.remote_owner_weapon_cancellations.is_empty(),
                "an expired cutoff must not occupy the bounded realtime fact window"
            );
        }
        assert!(
            game.remote_projectiles
                .iter()
                .all(|p| p.source.shot_group_id == Some(11)
                    || matches!(
                        p.source.identity,
                        RemoteWeaponSourceIdentity::Authenticated {
                            session_id: 201,
                            ..
                        }
                    ))
        );
        install_predicted_own_bullet(&mut game, 3, 1, 0);
        assert!(!game.apply_owner_weapon_cancellation(
            77,
            88,
            5,
            crate::realtime_event::RealtimeOwnerWeaponCancellation {
                source_player_id: 9,
                owner_life_seq: 3,
                owner_input_sequence: Some(1),
                ..cutoff
            }
        ));
        assert_eq!(
            game.bullets.len(),
            1,
            "an old local input cutoff must never cross sequence wrap"
        );
    }

    #[test]
    fn departure_cutoff_clears_actor_waiters_and_rejects_delayed_cancelled_carriers() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let original = realtime_weapon(1, 2, 6);
        for (index, source) in [
            original,
            crate::realtime_event::RealtimeWeaponEvent {
                shot_group_id: 11,
                ..original
            },
            crate::realtime_event::RealtimeWeaponEvent {
                source_session_id: 201,
                ..original
            },
            realtime_weapon(1, 2, 5),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(!game.apply_realtime_weapon_visual(77, 88, index as u32 + 1, 110, source));
        }
        assert_eq!(game.remote_weapon_presentations.len(), 4);
        assert!(game.apply_owner_weapon_cancellation(
            77,
            88,
            10,
            crate::realtime_event::RealtimeOwnerWeaponCancellation {
                source_player_id: 1,
                source_session_id: 101,
                through_shot_group_id: 10,
                owner_life_seq: 8,
                owner_input_sequence: None,
            }
        ));
        assert_eq!(game.remote_weapon_presentations.len(), 3);
        assert_eq!(
            game.remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::ScopeCancelled),
            1
        );
        assert!(!game.apply_realtime_weapon_visual(77, 88, 9, 110, original));
        assert_eq!(
            game.remote_weapon_presentations.len(),
            3,
            "a delayed cancelled source must not re-enter actor dependency waiting"
        );
        assert_eq!(
            game.remote_weapon_presentations.dropped_total(),
            0,
            "intentional cancellation is not an unexplained presentation drop"
        );
    }

    #[test]
    fn departure_expired_cutoff_cannot_overfill_remote_terminal_history() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let cutoff = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 1,
            source_session_id: 101,
            through_shot_group_id: 1,
            owner_life_seq: 1,
            owner_input_sequence: None,
        };
        let limit = crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS;
        for id in 2..=limit as u32 + 1 {
            assert!(game.apply_owner_weapon_cancellation(
                77,
                88,
                id,
                crate::realtime_event::RealtimeOwnerWeaponCancellation {
                    source_session_id: 1000 + u64::from(id),
                    ..cutoff
                }
            ));
        }
        assert_eq!(game.remote_owner_weapon_cancellations.len(), limit);
        assert!(game.apply_owner_weapon_cancellation(77, 88, 1, cutoff));
        assert_eq!(
            game.remote_owner_weapon_cancellations.len(),
            limit,
            "ordered cleanup beyond the horizon must not become a 4097th retained fact"
        );
    }

    #[test]
    fn safe_owner_cancellation_uses_life_and_input_cut_without_erasing_new_prediction() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        for (life, sequence) in [(2, 100), (3, 41), (3, 43), (4, 1)] {
            install_predicted_own_bullet(&mut game, life, sequence, 0);
        }
        install_predicted_own_bullet(&mut game, 3, 44, 0);
        game.bullets
            .last_mut()
            .unwrap()
            .prediction
            .as_mut()
            .unwrap()
            .input_sequence = None;
        for sequence in [41, 43] {
            game.decoys.push(Decoy {
                authority_binding: None,
                presentation_clock: None,
                prediction: Some(crate::model::OwnedProjectilePrediction {
                    activation: sequence as u64,
                    life_seq: 3,
                    input_sequence: Some(sequence),
                    projectile_index: 0,
                }),
                pos: Vec2::ZERO,
                vel: Vec2::ZERO,
                frequency: 1,
                orientation: 0.0,
                initial_orientation: 0.0,
                remote_source: None,
                last_repel_mutation_event_id: None,
                authoritative_repel_state: None,
                ship_type: 0,
                owner_name: "owned".to_owned(),
                owner_bounty: 30,
                ttl: 5.0,
                age: 0.0,
            });
        }
        let cancellation = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 9,
            source_session_id: 109,
            through_shot_group_id: 10,
            owner_life_seq: 3,
            owner_input_sequence: Some(42),
        };
        assert!(game.apply_owner_weapon_cancellation(77, 88, 1, cancellation));
        assert_eq!(game.bullets.len(), 3);
        assert_eq!(game.decoys.len(), 1);
        assert_eq!(game.decoys[0].prediction.unwrap().input_sequence, Some(43));
        assert!(game.bullets.iter().all(|p| {
            let p = p.prediction.unwrap();
            (p.life_seq == 3 && p.input_sequence == Some(43))
                || p.life_seq == 4
                || p.input_sequence.is_none()
        }));
        assert!(!game.apply_owner_weapon_cancellation(77, 88, 1, cancellation));
        assert_eq!(game.remote_owner_weapon_cancellations.len(), 1);
        game.bullets.clear();
        game.decoys.clear();
        for (life, sequence) in [(u32::MAX, 100), (1, u16::MAX), (1, 1)] {
            install_predicted_own_bullet(&mut game, life, sequence, 0);
        }
        assert!(game.apply_owner_weapon_cancellation(
            77,
            88,
            2,
            crate::realtime_event::RealtimeOwnerWeaponCancellation {
                owner_life_seq: 1,
                owner_input_sequence: Some(0),
                ..cancellation
            }
        ));
        assert_eq!(game.bullets.len(), 1);
        assert_eq!(game.bullets[0].prediction.unwrap().life_seq, 1);
        assert_eq!(game.bullets[0].prediction.unwrap().input_sequence, Some(1));
    }

    #[test]
    fn safe_owner_cancellation_ignores_stale_remote_safe_position_for_newer_flight() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.networked_play = true;
        game.maps[game.map_index].set_tile(10, 10, 171);
        let mut actor = remote_presentation_actor(1, 7);
        actor.source_session_id = Some(101);
        actor.position = Some(vec2(168.0, 168.0));
        game.network_world.players.insert(1, actor);
        let mut source = realtime_weapon(1, 2, 2);
        source.shot_group_id = 11;
        let state = realtime_projectile_state(source, 900, 120);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, state));
        game.remove_safe_zone_owner_weapons();
        assert_eq!(
            game.remote_projectiles.len(),
            1,
            "a stale mutable actor sample cannot erase authoritative flight"
        );
        let mut cut = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 1,
            source_session_id: 101,
            through_shot_group_id: 10,
            owner_life_seq: 7,
            owner_input_sequence: None,
        };
        assert!(game.apply_owner_weapon_cancellation(77, 88, 2, cut));
        assert_eq!(game.remote_projectiles.len(), 1);
        cut.through_shot_group_id = 11;
        assert!(game.apply_owner_weapon_cancellation(77, 88, 3, cut));
        assert!(game.remote_projectiles.is_empty());
    }

    #[test]
    fn safe_owner_cancellation_window_is_bounded_and_wrap_aware() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let cancellation = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 1,
            source_session_id: 101,
            through_shot_group_id: 10,
            owner_life_seq: 7,
            owner_input_sequence: None,
        };
        assert!(game.apply_owner_weapon_cancellation(77, 88, u32::MAX, cancellation));
        game.observe_remote_realtime_event(5);
        assert_eq!(game.remote_owner_weapon_cancellations.len(), 1);
        game.observe_remote_realtime_event(
            u32::MAX.wrapping_add(crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32),
        );
        assert!(game.remote_owner_weapon_cancellations.is_empty());
        game.clear_remote_weapon_presentation();
        assert!(game.remote_owner_weapon_cancellations.is_empty());
    }

    fn predicted_pointblank_explosion(game: &mut Game) {
        game.networked_play = true;
        game.pending_weapon_prediction = Some(10);
        let prediction = game.local_weapon_prediction(0);
        game.add_predicted_explosion(
            vec2(100.0, 100.0),
            vec2(100.0, 100.0),
            1,
            100.0,
            0.0,
            ExplosiveShrapnel::none(),
            prediction,
        );
        assert_eq!(game.predicted_explosion_receipts[0].input_sequence, None);
        game.bind_pending_weapon_prediction(42);
        assert_eq!(
            game.predicted_explosion_receipts[0].input_sequence,
            Some(42)
        );
        assert_eq!(
            game.explosions[0].prediction.unwrap().input_sequence,
            Some(42)
        );
    }

    #[test]
    fn owner_explosion_terminal_converges_predicted_animation_without_restart() {
        for kind in [3, 4, 8] {
            let mut game = Game::new_network_authority_test();
            install_realtime_authority_scope(&mut game);
            predicted_pointblank_explosion(&mut game);
            game.explosions[0].age = 0.2;
            let mut terminal = own_projectile_state(42);
            terminal.source.weapon_data = kind;
            terminal.weapon_data = kind;
            terminal.terminal = true;
            assert!(game.apply_realtime_projectile_state(77, 88, 1, 122, terminal));
            assert_eq!(game.explosions.len(), 1);
            let explosion = &game.explosions[0];
            assert_eq!(
                explosion.pos,
                vec2(terminal.position_x, terminal.position_y)
            );
            assert_eq!(explosion.age, 0.2);
            assert!(!explosion.damages_bots);
            assert_eq!(explosion.damage, 0.0);
            assert!(explosion.remote_source.is_some());
            assert!(explosion.prediction.is_none());
            assert!(!game.apply_realtime_projectile_state(77, 88, 2, 122, terminal));
            assert_eq!(game.explosions.len(), 1);
        }
    }

    #[test]
    fn owner_explosion_late_terminal_does_not_replay_expired_prediction() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        predicted_pointblank_explosion(&mut game);
        game.explosions.clear(); // normal cosmetic expiry does not erase the receipt
        // Even if the effect expires before a delayed outbound publication,
        // the retained activation still binds to that actual sent sequence.
        game.predicted_explosion_receipts[0].input_sequence = None;
        game.pending_weapon_prediction = Some(10);
        game.bind_pending_weapon_prediction(42);
        assert_eq!(
            game.predicted_explosion_receipts[0].input_sequence,
            Some(42)
        );
        let mut terminal = own_projectile_state(42);
        terminal.source.weapon_data = 3;
        terminal.weapon_data = 3;
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 300, terminal));
        assert!(game.explosions.is_empty());
        // Another activation is not hidden by that prior prediction.
        terminal.source_input_sequence = Some(43);
        terminal.source.shot_group_id += 1;
        terminal.projectile_id += 1;
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 301, terminal));
        assert_eq!(game.explosions.len(), 1);
    }

    #[test]
    fn owner_explosion_receipts_are_bounded_and_expired_sequence_cycle_is_replaced() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        predicted_pointblank_explosion(&mut game);
        game.explosions.clear();
        game.pending_weapon_prediction = Some(11);
        game.bind_pending_weapon_prediction(42);
        assert!(
            game.predicted_explosion_receipts.is_empty(),
            "a new sequence cycle cannot inherit the old explosion receipt"
        );
        for activation in 0..=crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS {
            game.add_predicted_explosion(
                Vec2::ZERO,
                Vec2::ZERO,
                1,
                0.0,
                0.0,
                ExplosiveShrapnel::none(),
                Some(crate::model::OwnedProjectilePrediction {
                    activation: activation as u64 + 20,
                    life_seq: 3,
                    input_sequence: Some(activation as u16),
                    projectile_index: 0,
                }),
            );
            game.explosions.clear();
        }
        assert_eq!(
            game.predicted_explosion_receipts.len(),
            crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS
        );
        game.clear_local_weapons();
        assert!(game.predicted_explosion_receipts.is_empty());
    }

    #[test]
    fn owner_reconciliation_burst_terminal_is_per_child_not_whole_activation() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        install_predicted_own_bullet(&mut game, 3, 7, 0);
        install_predicted_own_bullet(&mut game, 3, 7, 1);
        let mut state = own_projectile_state(7);
        state.source.weapon_data = 7;
        state.weapon_data = 7;
        state.behavior = crate::realtime_event::PROJECTILE_BEHAVIOR_BURST_ACTIVE;
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, state));
        assert_eq!((game.bullets.len(), game.remote_projectiles.len()), (1, 1));
        state.source_projectile_index = 1;
        state.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 121, state));
        assert!(game.bullets.is_empty());
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(game.remote_projectiles[0].source_projectile_index, 0);
    }

    #[test]
    fn owner_reconciliation_binds_the_sent_boundary_and_drops_only_superseded_unsent_prediction() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.networked_play = true;
        game.pending_weapon_prediction = Some(10);
        install_owned_lifetime_bullet(&mut game);
        game.bullets[0].prediction = game.local_weapon_prediction(0);
        game.next_input_seq = 41;
        game.bind_pending_weapon_prediction(99);
        assert_eq!(game.bullets[0].prediction.unwrap().input_sequence, Some(99));
        assert_eq!(game.pending_weapon_prediction, None);
        game.pending_weapon_prediction = Some(11);
        install_owned_lifetime_bullet(&mut game);
        game.bullets[1].prediction = game.local_weapon_prediction(0);
        game.remove_unsubmitted_weapon_prediction(11);
        assert_eq!(game.bullets.len(), 1);
        assert_eq!(game.bullets[0].prediction.unwrap().input_sequence, Some(99));
    }

    #[test]
    fn weapon_lifetime_zero_and_delayed_respawn_render_one_death_explosion() {
        for delay in [0.0, 1.0] {
            let mut game = Game::new_network_authority_test();
            game.networked_play = false;
            install_owned_lifetime_bullet(&mut game);
            game.start_player_respawn_with_delay(vec2(200.0, 200.0), delay);
            assert_eq!(game.ship_explosions.len(), 1);
            assert_eq!(game.bullets.len(), 1);
        }
    }

    #[test]
    fn weapon_lifetime_presentation_survives_death_and_respawn_without_resetting_age() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        install_owned_lifetime_bullet(&mut game);
        let source = realtime_weapon(1, 2, 2);
        let state = realtime_projectile_state(source, 900, 120);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, state));
        let remote_age = game.remote_projectiles[0].age;
        let remote_ttl = game.remote_projectiles[0].ttl;
        game.start_player_respawn_with_delay(vec2(200.0, 200.0), 1.0);
        assert_eq!(game.bullets.len(), 1);
        assert_eq!(game.remote_projectiles.len(), 1);
        game.reset_ship_after_death();
        assert_eq!(game.bullets.len(), 1);
        assert_eq!(game.bullets[0].ttl, 1.5);
        assert_eq!(game.bullets[0].age, 0.5);
        assert_eq!(game.remote_projectiles[0].ttl, remote_ttl);
        assert_eq!(game.remote_projectiles[0].age, remote_age);
        let mut terminal = state;
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 130, terminal));
        game.reset_ship_after_death();
        assert!(!game.apply_realtime_projectile_state(77, 88, 3, 120, state));
        assert!(game.remote_projectiles.is_empty());
    }

    #[test]
    fn weapon_lifetime_decoys_survive_network_respawn_with_remaining_age_and_motion() {
        let (mut game, _) = private_public_order_game();
        game.networked_play = true;
        let before = game
            .decoys
            .iter()
            .map(|d| (d.remote_source, d.ttl, d.age, d.pos, d.vel))
            .collect::<Vec<_>>();
        game.reset_ship_after_death();
        let after = game
            .decoys
            .iter()
            .map(|d| (d.remote_source, d.ttl, d.age, d.pos, d.vel))
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        // Ordinary ship selection still clears owned presentation only.
        game.reset_ship();
        assert_eq!(game.decoys.len(), 1);
        assert_eq!(game.decoys[0].remote_source, before[0].0);
    }

    #[test]
    fn weapon_lifetime_ship_reset_clears_owned_shots_but_preserves_foreign_presentation() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        install_owned_lifetime_bullet(&mut game);
        let source = realtime_weapon(1, 2, 2);
        assert!(game.apply_realtime_projectile_state(
            77,
            88,
            1,
            120,
            realtime_projectile_state(source, 900, 120)
        ));
        let remote = RemoteWeaponSourceIdentity::authenticated(1, 101, 7).unwrap();
        for identity in [None, Some(remote)] {
            game.bullet_impacts.push(BulletImpact {
                pos: vec2(100.0, 100.0),
                age: 0.1,
                remote_source: identity,
            });
            game.bomb_flashes.push(BombFlash {
                pos: vec2(100.0, 100.0),
                age: 0.1,
                remote_source: identity,
            });
        }
        game.reset_ship();
        assert!(game.bullets.is_empty());
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(game.bullet_impacts.len(), 1);
        assert_eq!(game.bullet_impacts[0].remote_source, Some(remote));
        assert_eq!(game.bomb_flashes.len(), 1);
        assert_eq!(game.bomb_flashes[0].remote_source, Some(remote));
        game.clear_active_weapons();
        game.clear_remote_weapon_presentation();
        assert!(game.remote_projectiles.is_empty());
        assert!(game.bullet_impacts.is_empty());
        assert!(game.bomb_flashes.is_empty());
    }

    #[test]
    fn shrapnel_state_and_parent_terminal_remain_distinct_and_render_one_explosion() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let mut source = realtime_weapon(1, 2, 3);
        source.weapon_data = encode_weapon_data(3, 2, true, 3, 4, false);
        let parent = realtime_projectile_state(source, 900, 120);
        assert!(game.apply_realtime_projectile_state(77, 88, 1, 120, parent));
        let mut child = parent;
        child.projectile_id = 901;
        child.weapon_data = encode_weapon_data(15, 3, false, 1, 0, false);
        child.source_server_tick = 120;
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 120, child));
        assert_eq!(game.remote_projectiles.len(), 2);
        let mut terminal = parent;
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 3, 120, terminal));
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(game.remote_projectiles[0].weapon_data & 0x1f, 15);
        assert_eq!(game.explosions.len(), 1);
        assert!(!game.apply_realtime_projectile_state(77, 88, 4, 120, terminal));
        assert_eq!(game.explosions.len(), 1);
        assert!(!game.apply_realtime_projectile_state(77, 88, 5, 120, parent));
        assert_eq!(game.remote_projectiles.len(), 1);
    }

    #[test]
    fn shrapnel_states_include_local_owner_and_retire_independent_children() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let mut source = realtime_weapon(9, 1, 3);
        source.source_life_seq = 3;
        source.weapon_data = encode_weapon_data(3, 2, true, 3, 4, false);
        let parent = realtime_projectile_state(source, 900, 120);
        assert!(!game.apply_realtime_projectile_state(77, 88, 1, 120, parent));
        let mut child = parent;
        child.projectile_id = 901;
        child.weapon_data = encode_weapon_data(15, 3, false, 1, 0, false);
        assert!(game.apply_realtime_projectile_state(77, 88, 2, 120, child));
        let mut sibling = child;
        sibling.projectile_id = 902;
        sibling.source_projectile_index = 1;
        assert!(game.apply_realtime_projectile_state(77, 88, 3, 120, sibling));
        child.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 4, 120, child));
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(game.remote_projectiles[0].source_projectile_index, 1);
        child.terminal = false;
        assert!(!game.apply_realtime_projectile_state(77, 88, 5, 120, child));
    }

    #[test]
    fn projectile_terminal_removes_linked_guns_and_rejects_reordered_copies() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        let mut source = realtime_weapon(1, 1, 1);
        source.weapon_data |= 0x8000;
        game.apply_realtime_weapon_visual(77, 88, 10, 110, source);
        assert_eq!(game.remote_projectiles.len(), 3);
        let mut terminal = realtime_projectile_state(source, 99, 130);
        terminal.terminal = true;
        terminal.source_projectile_index = 1;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 130, terminal));
        assert!(game.remote_projectiles.is_empty());
        assert!(!game.apply_realtime_projectile_state(77, 88, 12, 130, terminal));
        assert_eq!(game.remote_projectile_terminals.len(), 1);
        // The original carrier and a previously captured child state must
        // never resurrect the group after the authoritative hit.
        game.apply_realtime_weapon_visual(77, 88, 10, 110, source);
        let active = realtime_projectile_state(source, 99, 120);
        assert!(!game.apply_realtime_projectile_state(77, 88, 11, 120, active));
        assert!(game.remote_projectiles.is_empty());
        // Numeric actor reuse must not let an old terminal erase a new life.
        let mut next = active;
        next.source.source_life_seq += 1;
        next.projectile_id += 1;
        assert!(game.apply_realtime_projectile_state(77, 88, 13, 140, next));
        assert_eq!(game.remote_projectiles.len(), 1);
        let mut reused_session = active;
        reused_session.source.source_session_id += 1;
        reused_session.projectile_id += 2;
        assert!(game.apply_realtime_projectile_state(77, 88, 14, 140, reused_session));
        assert_eq!(game.remote_projectiles.len(), 2);
        assert!(!game.apply_realtime_projectile_state(77, 89, 15, 140, terminal));
        assert_eq!(game.remote_projectiles.len(), 2);
    }

    #[test]
    fn projectile_terminal_before_actor_and_carrier_blocks_delayed_materialization() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);
        game.apply_realtime_weapon_visual(77, 88, 10, 110, source);
        assert_eq!(game.remote_weapon_presentations.len(), 1);
        let mut terminal = realtime_projectile_state(source, 99, 130);
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 130, terminal));
        game.network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        game.drain_pending_remote_weapon_presentations();
        assert!(game.remote_projectiles.is_empty());
        assert_eq!(game.remote_projectile_spawns, 0);
        game.clear_remote_weapon_presentation();
        assert!(game.remote_projectile_terminals.is_empty());
    }

    #[test]
    fn projectile_terminal_history_is_bounded_and_cleared_with_arena_presentation() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);
        let mut terminal = realtime_projectile_state(source, 99, 130);
        terminal.terminal = true;
        let capacity = crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS;
        for index in 0..=capacity {
            terminal.source.shot_group_id = index as u64 + 1;
            terminal.projectile_id = index as u32 + 1;
            assert!(game.apply_realtime_projectile_state(77, 88, index as u32 + 1, 130, terminal));
        }
        assert_eq!(game.remote_projectile_terminals.len(), capacity);
        assert_eq!(
            game.remote_projectile_terminals
                .front()
                .unwrap()
                .shot_group_id,
            2
        );
        game.clear_remote_weapon_presentation();
        assert!(game.remote_projectile_terminals.is_empty());
    }

    #[test]
    fn projectile_terminal_history_evicts_by_sequence_age_after_reordered_arrivals() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);
        let capacity = crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32;
        // A recent terminal overtakes a full window of older terminals. FIFO
        // eviction would forget it while its pre-hit carrier is still legal.
        let mut terminal = realtime_projectile_state(source, 99, 130);
        terminal.terminal = true;
        terminal.source.shot_group_id = u64::from(capacity);
        assert!(game.apply_realtime_projectile_state(77, 88, capacity, 130, terminal));
        for id in 1..capacity {
            let mut older = terminal;
            older.source.shot_group_id = u64::from(id);
            older.projectile_id = id + 100;
            assert!(game.apply_realtime_projectile_state(77, 88, id, 130, older));
        }
        let mut next = terminal;
        next.source.shot_group_id = u64::from(capacity + 1);
        next.projectile_id = capacity + 100;
        assert!(game.apply_realtime_projectile_state(77, 88, capacity + 1, 130, next));
        assert_eq!(game.remote_projectile_terminals.len(), capacity as usize);
        let mut active = terminal;
        active.terminal = false;
        assert!(!game.apply_realtime_projectile_state(77, 88, capacity - 1, 120, active));
        assert!(game.remote_projectiles.is_empty());
        assert!(
            game.remote_projectile_terminals
                .iter()
                .any(|key| key.event_id == capacity)
        );
        assert!(
            !game
                .remote_projectile_terminals
                .iter()
                .any(|key| key.event_id == 1)
        );
    }

    #[test]
    fn projectile_terminal_keeps_burst_and_shrapnel_siblings_independent() {
        for weapon_type in [7, 15] {
            let mut game = Game::new_network_authority_test();
            install_realtime_authority_scope(&mut game);
            let source = realtime_weapon(1, 1, if weapon_type == 15 { 3 } else { 7 });
            for child in 0..2 {
                let mut active = realtime_projectile_state(source, 99 + u32::from(child), 120);
                active.source_projectile_index = child;
                if weapon_type == 15 {
                    active.weapon_data = 15;
                    active.source.weapon_data |= 2 << 10;
                }
                assert!(game.apply_realtime_projectile_state(
                    77,
                    88,
                    10 + u32::from(child),
                    120,
                    active
                ));
            }
            let mut terminal = realtime_projectile_state(source, 99, 130);
            terminal.terminal = true;
            if weapon_type == 15 {
                terminal.weapon_data = 15;
                terminal.source.weapon_data |= 2 << 10;
            }
            assert!(game.apply_realtime_projectile_state(77, 88, 12, 130, terminal));
            assert_eq!(game.remote_projectiles.len(), 1);
            assert_eq!(game.remote_projectiles[0].source_projectile_index, 1);
        }
    }

    #[test]
    fn active_projectile_state_materializes_without_source_actor_and_converges_both_orders() {
        let source = realtime_weapon(1, 1, 1);
        let current = realtime_projectile_state(source, 99, 120);

        let mut state_first = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut state_first);
        assert!(state_first.apply_realtime_projectile_state(77, 88, 11, 120, current));
        assert_eq!(state_first.remote_projectiles.len(), 1);
        assert_eq!(state_first.remote_projectiles[0].pos, vec2(130.0, 100.0));
        assert_eq!(
            state_first.remote_projectiles[0].authority_binding,
            Some(RemoteProjectileAuthorityBinding {
                projectile_id: 99,
                state_tick: 120,
            })
        );
        assert_eq!(state_first.remote_projectile_spawns, 1);
        assert_eq!(state_first.remote_bullet_spawns, 1);
        // The older original carrier may still arrive on the other unordered
        // DataChannel. Once its actor dependency releases, it must not create
        // a second child for the same shot group/index.
        assert!(!state_first.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        state_first
            .network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        state_first.drain_pending_remote_weapon_presentations();
        assert_eq!(state_first.remote_projectiles.len(), 1);
        assert_eq!(state_first.remote_projectile_spawns, 1);
        assert_eq!(state_first.remote_bullet_spawns, 1);

        let mut weapon_first = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut weapon_first);
        weapon_first
            .network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        assert!(!weapon_first.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert_eq!(weapon_first.remote_projectiles.len(), 1);
        assert_eq!(weapon_first.remote_projectile_spawns, 1);
        assert_eq!(weapon_first.remote_bullet_spawns, 1);
        assert!(weapon_first.apply_realtime_projectile_state(77, 88, 11, 120, current));
        assert_eq!(weapon_first.remote_projectiles.len(), 1);
        assert_eq!(weapon_first.remote_projectiles[0].pos, vec2(130.0, 100.0));
        assert_eq!(weapon_first.remote_projectile_spawns, 1);
        assert_eq!(weapon_first.remote_bullet_spawns, 1);
    }

    #[test]
    fn active_projectile_state_rejects_wrong_scope_self_and_stale_convergence() {
        let source = realtime_weapon(1, 1, 1);
        let current = realtime_projectile_state(source, 99, 120);
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        assert!(!game.apply_realtime_projectile_state(77, 89, 11, 120, current));
        assert!(game.apply_realtime_projectile_state(77, 88, 11, 120, current));
        let position = game.remote_projectiles[0].pos;
        let stale = realtime_projectile_state(source, 99, 119);
        assert!(!game.apply_realtime_projectile_state(77, 88, 12, 119, stale));
        assert_eq!(game.remote_projectiles[0].pos, position);

        let self_source = realtime_weapon(9, 1, 1);
        let self_state = realtime_projectile_state(self_source, 100, 120);
        assert!(!game.apply_realtime_projectile_state(77, 88, 13, 120, self_state));
        assert_eq!(game.remote_projectiles.len(), 1);
    }

    #[test]
    fn active_proximity_state_restores_only_the_exact_local_life_sensor() {
        let source = realtime_weapon(1, 1, 4);
        let mut state = realtime_projectile_state(source, 99, 120);
        state.proximity = Some(crate::realtime_event::RealtimeProjectileProximityState {
            target_player_id: 9,
            target_life_seq: 3,
            sensor_end_tick: 150,
            highest_axis_offset: 42.5,
        });
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.last_observed_browser_server_tick = Some(130);
        assert!(game.apply_realtime_projectile_state(77, 88, 11, 120, state));
        assert_eq!(
            game.remote_projectiles[0].proximity_sensor,
            Some(ProximitySensorState {
                target_id: 9,
                highest_axis_offset: 42.5,
                timer: 0.2,
            })
        );

        let mut other_life = state;
        other_life.projectile_id = 100;
        other_life.source.shot_group_id = 10;
        other_life
            .proximity
            .as_mut()
            .expect("proximity")
            .target_life_seq = 4;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 120, other_life));
        assert!(game.remote_projectiles[1].proximity_sensor.is_none());
    }

    #[test]
    fn realtime_weapon_waits_for_exact_snapshot_actor_before_presentation() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert!(game.remote_projectiles.is_empty());
        assert_eq!(game.remote_weapon_presentations.len(), 1);
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::ActorMissing,),
            1
        );

        game.network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        game.drain_pending_remote_weapon_presentations();

        assert_eq!(game.remote_weapon_presentations.len(), 0);
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(
            game.network_world.players[&1].source_session_id,
            Some(source.source_session_id)
        );
        assert_eq!(
            game.remote_projectiles[0].source.identity,
            RemoteWeaponSourceIdentity::authenticated(
                source.source_player_id,
                source.source_session_id,
                source.source_life_seq,
            )
            .unwrap()
        );
    }

    #[test]
    fn realtime_weapon_materializes_immediately_when_exact_actor_arrived_first() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        let source = realtime_weapon(1, 1, 1);

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert!(game.remote_weapon_presentations.is_empty());
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(game.remote_weapon_presentations.queued_total, 0);
        assert_eq!(game.remote_weapon_presentations.released_total, 0);
    }

    #[test]
    fn scheduler_pause_and_duplicate_event_keep_one_bounded_exact_life_waiter() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert_eq!(game.remote_weapon_presentations.len(), 1);
        assert_eq!(game.remote_weapon_presentations.queued_total, 1);
        for _ in 0..256 {
            game.drain_pending_remote_weapon_presentations();
        }
        assert_eq!(game.remote_weapon_presentations.len(), 1);
        assert!(game.remote_projectiles.is_empty());

        game.network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        game.drain_pending_remote_weapon_presentations();
        assert!(game.remote_weapon_presentations.is_empty());
        assert_eq!(game.remote_weapon_presentations.released_total, 1);
        assert_eq!(game.remote_projectiles.len(), 1);
    }

    #[test]
    fn remote_weapon_dependency_reclassifies_each_presentability_barrier_in_place() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);
        let mut actor = remote_presentation_actor(1, 7);
        actor.ship = 1;
        game.network_world.players.insert(1, actor);

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::ShipMismatch),
            1
        );

        let actor = game.network_world.players.get_mut(&1).unwrap();
        actor.ship = source.ship;
        actor.last_seen_snapshot_id = None;
        game.drain_pending_remote_weapon_presentations();
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::SnapshotMissing),
            1
        );

        let actor = game.network_world.players.get_mut(&1).unwrap();
        actor.last_seen_snapshot_id = Some(41);
        actor.position = None;
        game.drain_pending_remote_weapon_presentations();
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::PositionMissing),
            1
        );

        let actor = game.network_world.players.get_mut(&1).unwrap();
        actor.position = Some(vec2(115.0, 100.0));
        actor.directed_presentation = true;
        actor.render_position = None;
        game.drain_pending_remote_weapon_presentations();
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::DisplayPositionMissing),
            1
        );

        game.network_world
            .players
            .get_mut(&1)
            .unwrap()
            .render_position = Some(vec2(115.0, 100.0));
        game.drain_pending_remote_weapon_presentations();
        assert!(game.remote_weapon_presentations.is_empty());
        assert_eq!(game.remote_weapon_presentations.released_total, 1);
        assert_eq!(game.remote_projectiles.len(), 1);
        assert!(
            RemoteWeaponPresentationSuppressionReason::ALL
                .into_iter()
                .all(|reason| game.remote_weapon_presentations.pending_for(reason) == 0)
        );
    }

    #[test]
    fn unresolved_life_dependency_releases_only_for_the_exact_life() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let source = realtime_weapon(1, 1, 1);
        let mut actor = remote_presentation_actor(1, 7);
        actor.set_life_seq(None);
        actor.position = Some(vec2(115.0, 100.0));
        actor.render_position = actor.position;
        actor.last_seen_snapshot_id = None;
        game.network_world.players.insert(1, actor);

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, source));
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::LifeUnresolved),
            1
        );
        game.network_world
            .players
            .get_mut(&1)
            .unwrap()
            .set_life_seq(Some(7));
        game.drain_pending_remote_weapon_presentations();
        assert_eq!(
            game.remote_weapon_presentations
                .pending_for(RemoteWeaponPresentationSuppressionReason::SnapshotMissing),
            1
        );
        assert!(game.remote_projectiles.is_empty());

        let actor = game.network_world.players.get_mut(&1).unwrap();
        actor.last_seen_snapshot_id = Some(42);
        actor.position = Some(vec2(115.0, 100.0));
        actor.render_position = actor.position;
        game.drain_pending_remote_weapon_presentations();
        assert!(game.remote_weapon_presentations.is_empty());
        assert_eq!(game.remote_projectiles.len(), 1);
    }

    #[test]
    fn remote_weapon_dependency_capacity_preserves_older_fifo_and_rejects_newer() {
        let mut queue = RemoteWeaponPresentationQueue::default();
        let source = realtime_weapon(1, 1, 1);
        for event_id in 1..=MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS as u32 {
            queue.enqueue(pending_remote_weapon(
                event_id,
                source,
                RemoteWeaponPresentationSuppressionReason::ActorMissing,
            ));
        }
        let first_key = queue.pending.front().unwrap().key;
        let last_key = queue.pending.back().unwrap().key;

        queue.enqueue(pending_remote_weapon(
            MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS as u32 + 1,
            source,
            RemoteWeaponPresentationSuppressionReason::ActorMissing,
        ));

        assert_eq!(queue.len(), MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS);
        assert_eq!(queue.pending.front().unwrap().key, first_key);
        assert_eq!(queue.pending.back().unwrap().key, last_key);
        assert_eq!(
            queue.queued_total,
            MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS as u32
        );
        assert_eq!(
            queue.pending_for(RemoteWeaponPresentationSuppressionReason::ActorMissing),
            MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS
        );
        assert_eq!(
            queue.terminal_for(RemoteWeaponPresentationSuppressionReason::CapacityRejected),
            1
        );
        assert_eq!(queue.dropped_total(), 1);
    }

    #[test]
    fn remote_weapon_dependency_rejects_conflicting_identity_or_payload_for_one_event() {
        let mut queue = RemoteWeaponPresentationQueue::default();
        let source = realtime_weapon(1, 1, 1);
        queue.enqueue(pending_remote_weapon(
            10,
            source,
            RemoteWeaponPresentationSuppressionReason::ActorMissing,
        ));
        queue.enqueue(pending_remote_weapon(
            10,
            realtime_weapon(2, 1, 1),
            RemoteWeaponPresentationSuppressionReason::ActorMissing,
        ));
        let mut conflicting_payload = source;
        conflicting_payload.x += 1.0;
        queue.enqueue(pending_remote_weapon(
            10,
            conflicting_payload,
            RemoteWeaponPresentationSuppressionReason::ActorMissing,
        ));

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pending.front().unwrap().key.source_player_id, 1);
        assert_eq!(
            queue.terminal_for(RemoteWeaponPresentationSuppressionReason::InvalidIdentity),
            2
        );
    }

    #[test]
    fn realtime_event_window_expiry_and_session_reuse_are_terminal() {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        let old = realtime_weapon(1, 1, 1);
        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, old));
        let newer_id = 10 + crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32;
        assert!(!game.apply_realtime_weapon_visual(
            77,
            88,
            newer_id,
            111,
            realtime_weapon(2, 2, 1),
        ));
        assert_eq!(game.remote_weapon_presentations.len(), 1);
        assert_eq!(
            game.remote_weapon_presentations.pending[0].key.event_id,
            newer_id
        );
        assert_eq!(
            game.remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::EventWindowExpired,),
            1
        );

        let mut reused = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut reused);
        let mut actor = remote_presentation_actor(1, 7);
        assert!(actor.bind_source_session(old.source_session_id + 1, 7));
        reused.network_world.players.insert(1, actor);
        assert!(!reused.apply_realtime_weapon_visual(77, 88, 10, 110, old));
        assert!(reused.remote_weapon_presentations.is_empty());
        assert!(reused.remote_projectiles.is_empty());
        assert_eq!(reused.remote_weapon_presentations.superseded_total(), 1);
        assert_eq!(
            reused
                .remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::SessionSuperseded,),
            1
        );
    }

    #[test]
    fn realtime_weapon_never_binds_to_newer_actor_life_and_hard_reset_clears_waiters() {
        let mut stale = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut stale);
        stale
            .network_world
            .players
            .insert(1, remote_presentation_actor(1, 8));
        assert!(!stale.apply_realtime_weapon_visual(77, 88, 10, 110, realtime_weapon(1, 1, 1)));
        assert!(stale.remote_projectiles.is_empty());
        assert!(stale.remote_weapon_presentations.is_empty());
        assert_eq!(stale.remote_weapon_presentations.dropped_total(), 1);
        assert_eq!(
            stale
                .remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::LifeSuperseded,),
            1
        );

        let mut waiting = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut waiting);
        assert!(!waiting.apply_realtime_weapon_visual(77, 88, 10, 110, realtime_weapon(1, 1, 1),));
        assert_eq!(waiting.remote_weapon_presentations.len(), 1);
        waiting.clear_remote_weapon_presentation();
        assert!(waiting.remote_weapon_presentations.is_empty());
        assert_eq!(
            waiting
                .remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::ScopeCancelled,),
            1
        );
    }

    fn repel_mutation_batch(
        states: &[crate::realtime_event::RealtimeRepelMutationState],
    ) -> crate::realtime_event::RealtimeRepelMutationBatch {
        crate::realtime_event::RealtimeRepelMutationBatch::from_states(states).unwrap()
    }

    fn exact_repel_states() -> [crate::realtime_event::RealtimeRepelMutationState; 2] {
        [
            crate::realtime_event::RealtimeRepelMutationState {
                motion_frozen: false,
                entity_kind: crate::realtime_event::REPEL_MUTATION_DECOY,
                source_player_id: 1,
                source_session_id: 101,
                source_life_seq: 7,
                source_server_tick: 110,
                source_weapon_data: realtime_weapon(1, 1, 6).weapon_data,
                source_projectile_index: 0,
                result_weapon_data: realtime_weapon(1, 1, 6).weapon_data,
                position_x: 111.25,
                position_y: 99.5,
                velocity_x: 499.8,
                velocity_y: -14.1,
                expires_at_tick: 500,
                bounces_remaining: 0,
            },
            crate::realtime_event::RealtimeRepelMutationState {
                motion_frozen: false,
                entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
                source_player_id: 1,
                source_session_id: 101,
                source_life_seq: 7,
                source_server_tick: 115,
                source_weapon_data: realtime_weapon(1, 1, 8).weapon_data,
                source_projectile_index: 0,
                result_weapon_data: realtime_weapon(1, 1, 8).weapon_data,
                position_x: 116.5,
                position_y: 101.75,
                velocity_x: 493.2,
                velocity_y: 82.1,
                expires_at_tick: 500,
                bounces_remaining: 0,
            },
        ]
    }

    fn private_public_order_game() -> (Game, RemoteRepelPresentationFact) {
        let mut game = Game::new_network_authority_test();
        let arena = game.tuning.clone();
        let source = RemoteWeaponSource::capture(
            RemoteWeaponSourceIdentity::authenticated(9, 109, 7).unwrap(),
            RemoteWeaponArenaScope::authenticated(77, 88),
            vec2(115.0, 100.0),
            Vec2::ZERO,
            0.0,
            1,
            0,
            arena.ship(0),
            arena.gravity_bombs,
        )
        .unwrap()
        .with_realtime_order(10, 100, 11)
        .unwrap();
        game.decoys.push(remote_decoy_from_source(
            source,
            &arena,
            "remote".to_owned(),
            0,
        ));
        game.decoys.push(Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: None,
            pos: vec2(120.0, 100.0),
            vel: Vec2::ZERO,
            frequency: 1,
            orientation: 0.0,
            initial_orientation: 0.0,
            remote_source: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: 0,
            owner_name: "owned".to_owned(),
            owner_bounty: 0,
            ttl: 0.1,
            age: 0.0,
        });
        let fact = RemoteRepelPresentationFact {
            arena_scope: RemoteWeaponArenaScope::authenticated(77, 88).unwrap(),
            event_id: 11,
            server_tick: 101,
            origin: vec2(100.0, 100.0),
            source_frequency: 2,
        };
        (game, fact)
    }

    #[test]
    fn network_remote_player_contacts_wait_for_authoritative_terminal_for_every_weapon_form() {
        for weapon in [1_u16, 2, 3, 4, 7, 8, 15, 3 | (1 << 15), 4 | (1 << 15)] {
            let mut game = Game::new_network_authority_test();
            install_realtime_authority_scope(&mut game);
            game.door_clock_fresh = true;
            game.last_observed_browser_server_tick = Some(100);
            let energy_before = game.ship.energy;
            let mut source = realtime_weapon(
                1,
                1,
                if weapon & 31 == 15 {
                    3
                } else {
                    (weapon & 31) as u8
                },
            );
            source.x = game.ship.pos.x;
            source.y = game.ship.pos.y;
            if weapon & 31 == 15 {
                source.weapon_data |= 1 << 10;
            }
            let state = crate::realtime_event::RealtimeProjectileStateEvent {
                source_input_sequence: Some(1),
                terminal: false,
                motion_frozen: false,
                source,
                projectile_id: 123,
                source_server_tick: 100,
                source_projectile_index: 0,
                weapon_data: weapon,
                position_x: game.ship.pos.x,
                position_y: game.ship.pos.y,
                velocity_x: 0.0,
                velocity_y: 0.0,
                expiry_tick: Some(300),
                behavior: if weapon & 31 == 7 {
                    crate::realtime_event::PROJECTILE_BEHAVIOR_BURST_ACTIVE
                } else {
                    crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD
                },
                bounces_remaining: 0,
                proximity: None,
            };
            assert!(
                game.apply_realtime_projectile_state(77, 88, 1, 100, state),
                "weapon {weapon}"
            );
            assert_eq!(game.remote_projectiles.len(), 1);
            assert_ne!(
                game.remote_projectiles[0].frequency,
                game.player_frequency()
            );
            game.update_remote_projectiles(0.05);
            assert_eq!(
                game.remote_projectiles.len(),
                1,
                "predicted local contact erased weapon {weapon}"
            );
            assert!(game.remote_projectiles[0].ttl > 1.9);
            assert!(
                game.explosions.is_empty(),
                "unconfirmed contact created an explosion for {weapon}"
            );
            assert_eq!(game.ship.energy, energy_before);
            assert!(game.apply_realtime_projectile_state(
                77,
                88,
                2,
                105,
                crate::realtime_event::RealtimeProjectileStateEvent {
                    terminal: true,
                    motion_frozen: false,
                    ..state
                }
            ));
            assert!(
                game.remote_projectiles.is_empty(),
                "authoritative terminal must remove weapon {weapon}"
            );
        }
    }

    #[test]
    fn remote_decoy_uses_player_state_and_arena_lifetime() {
        let mut arena = ArenaTuning::continuum_skirmish_default();
        arena.decoy_lifetime = 12.0;
        let identity = RemoteWeaponSourceIdentity::authenticated(9, u64::MAX, u32::MAX).unwrap();
        let source = RemoteWeaponSource::capture(
            identity,
            RemoteWeaponArenaScope::authenticated(77, 88),
            vec2(40.0, 80.0),
            vec2(3.0, -5.0),
            1.0,
            7,
            4,
            arena.ship(4),
            arena.gravity_bombs,
        )
        .unwrap();

        let decoy = remote_decoy_from_source(source, &arena, "owner".to_owned(), 123);

        assert_eq!(decoy.pos, vec2(40.0, 80.0));
        assert_eq!(decoy.vel, vec2(3.0, -5.0));
        assert_eq!(decoy.frequency, 7);
        assert_eq!(decoy.orientation, 1.0);
        assert_eq!(decoy.initial_orientation, 1.0);
        assert_eq!(decoy.remote_source, Some(source));
        assert_eq!(decoy.ship_type, 4);
        assert_eq!(decoy.owner_name, "owner");
        assert_eq!(decoy.owner_bounty, 123);
        assert_eq!(decoy.ttl, 12.0);
        assert_eq!(decoy.age, 0.0);
    }

    #[test]
    fn public_repel_updates_observer_remote_entities_without_owned_mutation() {
        let mut game = Game::new_network_authority_test();
        let arena = game.tuning.clone();
        let hostile_identity = RemoteWeaponSourceIdentity::authenticated(9, 900, 90).unwrap();
        let hostile_source = RemoteWeaponSource::capture(
            hostile_identity,
            RemoteWeaponArenaScope::authenticated(77, 88),
            vec2(110.0, 100.0),
            vec2(-10.0, 0.0),
            0.0,
            12,
            0,
            arena.ship(0),
            arena.gravity_bombs,
        )
        .unwrap();
        game.remote_projectiles = remote_weapon_projectiles(
            hostile_source,
            encode_weapon_data(1, 1, false, 1, 0, false),
            arena.ship(0),
            &arena,
        );
        game.remote_projectiles[0].pos = vec2(110.0, 100.0);
        game.remote_projectiles[0].ttl = 0.1;
        game.decoys.push(remote_decoy_from_source(
            hostile_source,
            &arena,
            "remote".to_owned(),
            0,
        ));
        game.decoys[0].pos = vec2(115.0, 100.0);
        game.decoys[0].ttl = 0.1;
        game.decoys.push(Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: None,
            pos: vec2(120.0, 100.0),
            vel: vec2(-5.0, 0.0),
            frequency: 12,
            orientation: 0.0,
            initial_orientation: 0.0,
            remote_source: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: 0,
            owner_name: "owned".to_owned(),
            owner_bounty: 0,
            ttl: 0.1,
            age: 0.0,
        });
        game.ship.pos = vec2(125.0, 100.0);
        game.ship.vel = vec2(7.0, -3.0);
        let owned_decoy = (game.decoys[1].pos, game.decoys[1].vel, game.decoys[1].ttl);

        game.apply_remote_presentation_repel_effect(vec2(100.0, 100.0), 7);

        assert!(game.remote_projectiles[0].vel.x > 0.0);
        assert_eq!(
            game.remote_projectiles[0].ttl,
            weapon_lifetime(&arena, WeaponLifetimeKind::Bullet)
        );
        assert!(game.decoys[0].vel.x > 0.0);
        assert_eq!(
            game.decoys[0].ttl,
            weapon_lifetime(&arena, WeaponLifetimeKind::Decoy)
        );
        assert_eq!(
            (game.decoys[1].pos, game.decoys[1].vel, game.decoys[1].ttl),
            owned_decoy
        );
        assert_eq!(game.ship.vel, vec2(7.0, -3.0));
    }

    #[test]
    fn delayed_weapon_facts_replay_later_repel_but_future_facts_remain_untouched() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.last_observed_browser_server_tick = Some(125);
        game.door_clock_fresh = true;

        assert!(!game.apply_realtime_weapon_visual(77, 88, 12, 120, realtime_weapon(2, 2, 5),));
        let exact_states = exact_repel_states();
        game.apply_realtime_repel_mutations(77, 88, 13, 120, repel_mutation_batch(&exact_states));
        assert!(game.remote_projectiles.is_empty());
        assert!(game.decoys.is_empty());

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, realtime_weapon(1, 1, 6),));
        assert!(!game.apply_realtime_weapon_visual(77, 88, 11, 115, realtime_weapon(1, 1, 8),));
        assert_eq!(game.decoys.len(), 1);
        assert_eq!(game.remote_projectiles.len(), 1);
        assert!(
            game.decoys[0]
                .pos
                .distance(vec2(111.25, 99.5) + vec2(499.8, -14.1) * 0.05)
                < 0.001
        );
        assert_eq!(game.decoys[0].vel, vec2(499.8, -14.1));
        assert!(
            game.remote_projectiles[0]
                .pos
                .distance(vec2(116.5, 101.75) + vec2(493.2, 82.1) * 0.05)
                < 0.001
        );
        assert_eq!(game.remote_projectiles[0].vel, vec2(493.2, 82.1));
        assert_eq!(
            game.decoys[0].authoritative_repel_state,
            Some(exact_states[0])
        );
        assert_eq!(
            game.remote_projectiles[0].authoritative_repel_state,
            Some(exact_states[1])
        );
        assert_eq!(
            game.decoys[0].remote_source.unwrap().realtime_event_id,
            Some(10)
        );
        assert_eq!(
            game.remote_projectiles[0].source.realtime_event_id,
            Some(11)
        );
        assert!(
            game.decoys[0].ttl < weapon_lifetime(&game.tuning, WeaponLifetimeKind::Decoy),
            "a late carrier retains the server Repel deadline instead of restarting TTL"
        );

        let mut future_first = Game::new_network_authority_test();
        future_first.door_clock_fresh = true;
        future_first.last_observed_browser_server_tick = Some(125);
        assert!(!future_first.apply_realtime_weapon_visual(
            77,
            88,
            12,
            120,
            realtime_weapon(1, 1, 8),
        ));
        let before = future_first.remote_projectiles[0].vel;
        assert!(!future_first.apply_realtime_weapon_visual(
            77,
            88,
            11,
            115,
            realtime_weapon(2, 2, 5),
        ));
        assert_eq!(future_first.remote_projectiles[0].vel, before);
    }

    #[test]
    fn ordinary_repel_delivery_reconciles_exact_server_projectile_and_decoy_state() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.door_clock_fresh = true;
        game.last_observed_browser_server_tick = Some(125);
        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, realtime_weapon(1, 1, 6),));
        assert!(!game.apply_realtime_weapon_visual(77, 88, 11, 115, realtime_weapon(1, 1, 8),));
        assert!(!game.apply_realtime_weapon_visual(77, 88, 12, 120, realtime_weapon(2, 2, 5),));

        let exact_states = exact_repel_states();
        game.apply_realtime_repel_mutations(77, 88, 13, 120, repel_mutation_batch(&exact_states));

        assert!(
            game.decoys[0]
                .pos
                .distance(vec2(111.25, 99.5) + vec2(499.8, -14.1) * 0.05)
                < 0.001
        );
        assert_eq!(game.decoys[0].vel, vec2(499.8, -14.1));
        assert!(
            game.remote_projectiles[0]
                .pos
                .distance(vec2(116.5, 101.75) + vec2(493.2, 82.1) * 0.05)
                < 0.001,
            "late exact Repel starts at capture pose and advances to the current server clock"
        );
        assert_eq!(game.remote_projectiles[0].vel, vec2(493.2, 82.1));
        assert_eq!(game.decoys[0].ttl, 3.75);
        assert_eq!(game.remote_projectiles[0].ttl, 3.75);
    }

    #[test]
    fn older_public_repel_cannot_overwrite_exact_state_on_existing_carriers() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.door_clock_fresh = true;
        game.last_observed_browser_server_tick = Some(125);
        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, realtime_weapon(1, 1, 6),));
        assert!(!game.apply_realtime_weapon_visual(77, 88, 11, 115, realtime_weapon(1, 1, 8),));

        let exact_states = exact_repel_states();
        game.apply_realtime_repel_mutations(77, 88, 13, 120, repel_mutation_batch(&exact_states));
        let decoy_exact = (game.decoys[0].pos, game.decoys[0].vel, game.decoys[0].ttl);
        let projectile_exact = (
            game.remote_projectiles[0].pos,
            game.remote_projectiles[0].vel,
            game.remote_projectiles[0].ttl,
        );

        assert!(!game.apply_realtime_weapon_visual(77, 88, 12, 120, realtime_weapon(2, 2, 5),));

        assert_eq!(
            (game.decoys[0].pos, game.decoys[0].vel, game.decoys[0].ttl),
            decoy_exact,
            "an older public fact cannot double-repel an exactly reconciled Decoy"
        );
        assert_eq!(
            (
                game.remote_projectiles[0].pos,
                game.remote_projectiles[0].vel,
                game.remote_projectiles[0].ttl,
            ),
            projectile_exact,
            "an older public fact cannot double-repel an exactly reconciled projectile"
        );
    }

    #[test]
    fn older_public_repel_cannot_overwrite_exact_state_on_delayed_carriers() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.door_clock_fresh = true;
        game.last_observed_browser_server_tick = Some(125);
        let exact_states = exact_repel_states();
        game.apply_realtime_repel_mutations(77, 88, 13, 120, repel_mutation_batch(&exact_states));

        assert!(!game.apply_realtime_weapon_visual(77, 88, 10, 110, realtime_weapon(1, 1, 6),));
        assert!(!game.apply_realtime_weapon_visual(77, 88, 11, 115, realtime_weapon(1, 1, 8),));
        let decoy_exact = (game.decoys[0].pos, game.decoys[0].vel, game.decoys[0].ttl);
        let projectile_exact = (
            game.remote_projectiles[0].pos,
            game.remote_projectiles[0].vel,
            game.remote_projectiles[0].ttl,
        );

        assert!(!game.apply_realtime_weapon_visual(77, 88, 12, 120, realtime_weapon(2, 2, 5),));

        assert_eq!(
            (game.decoys[0].pos, game.decoys[0].vel, game.decoys[0].ttl),
            decoy_exact,
            "an older public fact cannot double-repel a delayed exact Decoy"
        );
        assert_eq!(
            (
                game.remote_projectiles[0].pos,
                game.remote_projectiles[0].vel,
                game.remote_projectiles[0].ttl,
            ),
            projectile_exact,
            "an older public fact cannot double-repel a delayed exact projectile"
        );
    }

    #[test]
    fn stale_some_clock_fails_closed_for_delayed_post_repel_carrier() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.last_observed_browser_server_tick = Some(125);
        game.door_clock_fresh = false;
        assert!(!game.apply_realtime_weapon_visual(77, 88, 12, 120, realtime_weapon(2, 2, 5),));
        let exact_states = exact_repel_states();
        game.apply_realtime_repel_mutations(77, 88, 13, 120, repel_mutation_batch(&exact_states));

        assert!(!game.apply_realtime_weapon_visual(77, 88, 11, 115, realtime_weapon(1, 1, 8),));
        assert!(
            game.remote_projectiles.is_empty(),
            "a stale retained tick cannot rematerialize a refreshed projectile lifetime"
        );
    }

    #[test]
    fn local_repel_echo_retains_only_causal_history_for_a_delayed_remote_weapon() {
        let mut game = Game::new_network_authority_test();
        game.door_clock_fresh = true;
        game.last_observed_browser_server_tick = Some(125);
        game.install_network_test_player_identity(2);
        assert!(!game.apply_realtime_weapon_visual(77, 88, 12, 120, realtime_weapon(2, 2, 5),));
        assert_eq!(game.remote_repel_spawns, 0);
        assert_eq!(game.remote_repel_presentation_history.len(), 1);

        assert!(!game.apply_realtime_weapon_visual(77, 88, 11, 115, realtime_weapon(1, 1, 8),));
        assert!(game.remote_projectiles[0].vel.length() > 0.0);
        assert_eq!(game.remote_repel_spawns, 0);
    }

    #[test]
    fn private_and_public_repel_carriers_mutate_disjoint_decoys_in_either_order() {
        let (mut private_first, fact) = private_public_order_game();
        let remote_before = (
            private_first.decoys[0].pos,
            private_first.decoys[0].vel,
            private_first.decoys[0].ttl,
        );
        assert!(private_first.apply_private_remote_repel(
            9,
            fact.origin,
            fact.source_frequency,
            vec2(7.0, 8.0),
        ));
        assert_eq!(
            (
                private_first.decoys[0].pos,
                private_first.decoys[0].vel,
                private_first.decoys[0].ttl,
            ),
            remote_before,
            "the private target fact cannot mutate a third-party Decoy"
        );
        private_first.apply_remote_presentation_repel_fact(fact, 0, 0, false);

        let (mut public_first, fact) = private_public_order_game();
        let owned_before = (
            public_first.decoys[1].pos,
            public_first.decoys[1].vel,
            public_first.decoys[1].ttl,
        );
        public_first.apply_remote_presentation_repel_fact(fact, 0, 0, false);
        assert_eq!(
            (
                public_first.decoys[1].pos,
                public_first.decoys[1].vel,
                public_first.decoys[1].ttl,
            ),
            owned_before,
            "the public fact cannot mutate the target's owned Decoy"
        );
        assert!(public_first.apply_private_remote_repel(
            9,
            fact.origin,
            fact.source_frequency,
            vec2(7.0, 8.0),
        ));

        for index in 0..2 {
            assert_eq!(
                private_first.decoys[index].pos,
                public_first.decoys[index].pos
            );
            assert_eq!(
                private_first.decoys[index].vel,
                public_first.decoys[index].vel
            );
            assert_eq!(
                private_first.decoys[index].ttl,
                public_first.decoys[index].ttl
            );
        }
        assert_eq!(private_first.ship.vel, public_first.ship.vel);
    }
    fn delayed_carrier_fixture(
        observed: u32,
    ) -> (Game, crate::realtime_event::RealtimeWeaponEvent) {
        let mut game = Game::new_network_authority_test();
        install_realtime_authority_scope(&mut game);
        game.maps[game.map_index].tiles.fill(0);
        game.door_clock_fresh = true;
        game.last_observed_browser_server_tick = Some(observed);
        game.tuning.bullet_lifetime = 5.5; // Shipped Chaos value.
        game.network_world
            .players
            .insert(1, remote_presentation_actor(1, 7));
        let mut source = realtime_weapon(1, 1, 1);
        source.x = 1000.0;
        source.y = 1000.0;
        source.orientation = 0.25;
        (game, source)
    }

    #[test]
    fn held_source_snapshot_does_not_start_a_weapon_on_the_future_global_cursor() {
        let (mut game, mut source) = delayed_carrier_fixture(4_795);
        source.x = 8_354.072;
        source.y = 7_773.7173;
        source.velocity_x = -254.91484;
        source.velocity_y = 72.210625;
        source.orientation = 0.050000016;
        let owner = game.network_world.players.get_mut(&1).unwrap();
        owner.position = Some(vec2(8_359.0, 7_772.0));
        owner.record_directed_snapshot(1, 4_786, RemoteSnapReason::None);
        owner.update_directed_render_position(4_788, 1.0 / 60.0);
        game.apply_realtime_weapon_visual(77, 88, 10, 4_788, source);
        let live_before = game.remote_projectiles.clone();
        game.prepare_remote_weapon_views(Some(4_788));
        assert!(
            game.prepared_remote_projectiles.is_empty(),
            "a held source at4786 cannot establish a fresh muzzle at4788"
        );
        assert_eq!(game.remote_projectiles, live_before);
        let owner = game.network_world.players.get_mut(&1).unwrap();
        owner.position = Some(vec2(source.x, source.y));
        owner.record_directed_snapshot(2, 4_788, RemoteSnapReason::None);
        owner.update_directed_render_position(4_788, 1.0 / 60.0);
        game.prepare_remote_weapon_views(Some(4_788));
        assert_eq!(game.prepared_remote_projectiles.len(), 1);
        assert_eq!(
            game.prepared_remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            4_788
        );
        let actor = game.network_world.players[&1].display_position().unwrap();
        assert_eq!(
            game.prepared_remote_projectiles[0].pos.round(),
            actor.round()
        );
        assert_eq!(game.remote_projectiles, live_before);
    }

    #[test]
    fn held_source_cannot_extend_weapon_hiding_past_the_existing_harm_deadline() {
        let (mut game, source) = delayed_carrier_fixture(4_816);
        let owner = game.network_world.players.get_mut(&1).unwrap();
        owner.position = Some(vec2(source.x + 5.0, source.y - 2.0));
        owner.record_directed_snapshot(1, 4_786, RemoteSnapReason::None);
        owner.update_directed_render_position(4_809, 1.0 / 60.0);
        let held = owner.display_position();
        game.apply_realtime_weapon_visual(77, 88, 10, 4_788, source);
        let live_before = game.remote_projectiles.clone();
        game.prepare_remote_weapon_views(Some(4_809));
        assert_eq!(game.prepared_remote_projectiles.len(), 1);
        assert_eq!(
            game.prepared_remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            4_816
        );
        assert_eq!(game.remote_projectiles, live_before);
        assert_eq!(game.network_world.players[&1].display_position(), held);
    }

    #[test]
    fn weapon_birth_firing_heading_matches_source_without_mutating_motion_or_weapons() {
        let (mut game, mut source) = delayed_carrier_fixture(107);
        source.orientation = 0.2;
        let owner = game.network_world.players.get_mut(&1).unwrap();
        owner.position = Some(vec2(source.x, source.y));
        owner.orientation = source.orientation;
        owner.record_directed_snapshot(1, 100, RemoteSnapReason::None);
        owner.update_directed_render_position(100, 1.0 / 60.0);
        owner.directed_render_orientation = Some(0.17); // observed two-bin lag
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles.len(), 1);
        let before = game.remote_projectiles.clone();
        let owner = &game.network_world.players[&1];
        let motion = (
            owner.position,
            owner.render_position,
            owner.velocity,
            owner.orientation,
        );
        game.prepare_remote_weapon_views(Some(100));
        assert_eq!(
            game.prepared_remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            100
        );
        let owner = &game.network_world.players[&1];
        assert_eq!(owner.display_orientation(), source.orientation);
        assert_eq!(
            (
                owner.position,
                owner.render_position,
                owner.velocity,
                owner.orientation
            ),
            motion
        );
        assert_eq!(game.remote_projectiles, before);
    }

    #[test]
    fn weapon_birth_current_fallback_does_not_rewind_the_ship_to_old_firing_facing() {
        for actor_tick in [None, Some(107)] {
            let (mut game, mut source) = delayed_carrier_fixture(107);
            source.orientation = 0.2;
            let owner = game.network_world.players.get_mut(&1).unwrap();
            owner.position = Some(vec2(source.x, source.y));
            owner.orientation = 0.4;
            owner.record_directed_snapshot(1, 100, RemoteSnapReason::None);
            // Establish an actually displayed later pose, not a held100
            // sample under a global107 cursor. This case requires current
            // flight without rewinding that later source's facing.
            owner.record_directed_snapshot(2, 107, RemoteSnapReason::None);
            owner.update_directed_render_position(107, 1.0 / 60.0);
            owner.directed_render_orientation = Some(0.37);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            game.prepare_remote_weapon_views(actor_tick);
            assert_eq!(
                game.prepared_remote_projectiles[0]
                    .presentation_clock
                    .unwrap()
                    .pose_tick,
                107
            );
            assert_eq!(game.network_world.players[&1].display_orientation(), 0.37);
        }
    }

    #[test]
    fn weapon_birth_draw_starts_at_the_source_then_converges_without_changing_runtime() {
        let (mut game, mut source) = delayed_carrier_fixture(100);
        source.x = 1000.49;
        source.y = 1000.51;
        source.velocity_x = 150.0;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let origin = game.remote_projectiles[0].pos;
        let velocity = game.remote_projectiles[0].vel;
        let energy = game.ship.energy;
        for now in 100..=114 {
            let actor_tick = now - 7;
            let owner = game.network_world.players.get_mut(&1).unwrap();
            owner.render_position = Some(
                origin
                    + vec2(source.velocity_x, source.velocity_y)
                        * ((actor_tick as i32 - 100) as f32 / 100.0),
            );
            owner.orientation = source.orientation + (actor_tick as i32 - 100) as f32 * 0.002;
            game.reset_weapon_recovery_budget();
            game.last_observed_browser_server_tick = Some(now);
            game.update_remote_weapons_at_server_time();
            game.prepare_remote_weapon_views(Some(now - 7));
            let live = game.remote_projectiles[0];
            assert_eq!(live.presentation_clock.unwrap().pose_tick, now);
            assert!(
                live.pos
                    .distance(origin + velocity * ((now - 100) as f32 / 100.0))
                    < 0.02
            );
            if now < 107 {
                assert!(game.prepared_remote_projectiles.is_empty());
            }
            if now == 107 {
                let drawn = game.prepared_remote_projectiles[0];
                assert_eq!(drawn.pos, origin);
                assert_eq!(drawn.presentation_clock.unwrap().pose_tick, 100);
                assert_eq!(drawn.age, 0.0);
                assert_ne!(drawn.pos, live.pos);
                let owner = game.network_world.players.get(&1).unwrap();
                // The reference game starts at packet center; the ship sprite covers
                // the first segment. Adding a radius would alter gun physics.
                let muzzle = owner.display_position().unwrap();
                assert_eq!(owner.display_orientation(), source.orientation);
                assert!(
                    drawn.pos.distance(muzzle) < 0.01,
                    "the first submitted ray shares the moving/turning source center"
                );
            }
            if now == 114 {
                assert_eq!(game.prepared_remote_projectiles[0].pos, live.pos);
            }
        }
        assert_eq!(game.pending_remote_birth_effects.len(), 0);
        assert_eq!(game.ship.energy, energy);
        assert!(114 < 100 + crate::combat_timeline::PROJECTILE_PRESENTATION_MIN_LEAD_TICKS);
    }

    #[test]
    fn weapon_birth_without_a_drawable_exact_source_uses_current_surviving_flight() {
        for mode in [
            "dead-life",
            "session-replaced",
            "departed",
            "spectator",
            "attached",
            "no-position",
            "cloaked",
        ] {
            let (mut game, source) = delayed_carrier_fixture(107);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            let live = game.remote_projectiles[0];
            let enemy_frequency = game.viewer_frequency().wrapping_add(1);
            let owner = game.network_world.players.get_mut(&1).unwrap();
            match mode {
                "dead-life" => {
                    owner.set_life_seq(Some(8));
                    owner.position = None;
                    owner.render_position = None;
                }
                "session-replaced" => owner.source_session_id = Some(999),
                "departed" => {
                    game.network_world.players.remove(&1);
                }
                "spectator" => owner.ship = SPECTATOR_SHIP,
                "attached" => owner.attach_parent = 9,
                "no-position" => {
                    owner.position = None;
                    owner.render_position = None;
                }
                "cloaked" => {
                    owner.togglables = STATUS_CLOAK;
                    owner.frequency = enemy_frequency;
                }
                _ => unreachable!(),
            }
            game.prepare_remote_weapon_views(Some(100));
            assert_eq!(
                game.remote_projectiles.len(),
                1,
                "{mode}: source loss cannot kill its weapon"
            );
            assert_eq!(
                game.prepared_remote_projectiles.len(),
                1,
                "{mode}: live weapon must stay visible"
            );
            let drawn = game.prepared_remote_projectiles[0];
            assert_eq!(
                drawn.presentation_clock.unwrap().pose_tick,
                107,
                "{mode}: no historic muzzle without a drawn source"
            );
            assert_eq!(drawn.pos, live.pos, "{mode}: retain current flight");
            assert_eq!(
                drawn.ttl, live.ttl,
                "{mode}: no shortened or extended lifetime"
            );
        }
    }

    #[test]
    fn weapon_birth_after_source_cutover_cannot_rewind_when_a_source_reappears() {
        let (mut game, source) = delayed_carrier_fixture(107);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        game.prepare_remote_weapon_views(Some(100));
        assert_eq!(
            game.prepared_remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            100
        );
        game.network_world.players.remove(&1);
        game.last_observed_browser_server_tick = Some(108);
        game.reset_weapon_recovery_budget();
        game.update_remote_weapons_at_server_time();
        game.prepare_remote_weapon_views(Some(101));
        assert_eq!(
            game.prepared_remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            108
        );
        let mut actor = remote_presentation_actor(1, 7);
        assert!(actor.bind_source_session(source.source_session_id, 7));
        game.network_world.players.insert(1, actor);
        game.last_observed_browser_server_tick = Some(109);
        game.reset_weapon_recovery_budget();
        game.update_remote_weapons_at_server_time();
        game.prepare_remote_weapon_views(Some(102));
        assert_eq!(
            game.prepared_remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            109
        );
    }

    #[test]
    fn weapon_birth_visible_stealth_team_cloak_or_xradar_sources_keep_alignment() {
        for mode in ["stealth", "team-cloak", "xradar-cloak"] {
            let (mut game, source) = delayed_carrier_fixture(107);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            let viewer_frequency = game.viewer_frequency();
            let owner = game.network_world.players.get_mut(&1).unwrap();
            match mode {
                "stealth" => owner.togglables = STATUS_STEALTH,
                "team-cloak" => {
                    owner.togglables = STATUS_CLOAK;
                    owner.frequency = viewer_frequency;
                }
                "xradar-cloak" => {
                    owner.togglables = STATUS_CLOAK;
                    owner.frequency = viewer_frequency.wrapping_add(1);
                    game.ship.xradar = true;
                }
                _ => unreachable!(),
            }
            game.prepare_remote_weapon_views(Some(100));
            assert_eq!(
                game.prepared_remote_projectiles[0]
                    .presentation_clock
                    .unwrap()
                    .pose_tick,
                100,
                "{mode}"
            );
        }
    }

    #[test]
    fn weapon_birth_recovery_fills_small_forward_gaps_before_replacing_the_canonical_pose() {
        let (mut game, source) = delayed_carrier_fixture(107);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let live = game.remote_projectiles[0];
        game.prepare_remote_weapon_views(Some(100));
        let mut recovery = realtime_projectile_state(source, 123, 109);
        recovery.source_server_tick = 100;
        recovery.position_x = live.pos.x + live.vel.x * 0.02;
        recovery.position_y = live.pos.y + live.vel.y * 0.02;
        recovery.velocity_x = live.vel.x;
        recovery.velocity_y = live.vel.y;
        recovery.expiry_tick = Some(650);
        game.last_observed_browser_server_tick = Some(109);
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 109, recovery));
        game.prepare_remote_weapon_views(Some(102));
        game.last_observed_browser_server_tick = Some(111);
        game.update_remote_weapons_at_server_time();
        game.prepare_remote_weapon_views(Some(104));
        let view = game.prepared_remote_projectiles[0];
        assert_eq!(view.presentation_clock.unwrap().pose_tick, 108);
        assert!(view.pos.distance(live.pos + live.vel * 0.01) < 0.01);
        assert_eq!(
            game.remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            111
        );
        assert_eq!(
            game.remote_weapon_view_history
                .stats()
                .missing_history_frames,
            0
        );
    }

    #[test]
    fn weapon_birth_linked_barrels_share_the_clock_and_release_one_source_cue() {
        let (mut game, source) = delayed_carrier_fixture(107);
        game.tuning.ships[0].double_barrel = true;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles.len(), 2);
        assert_eq!(game.pending_remote_birth_effects.len(), 1);
        game.prepare_remote_weapon_views(Some(100));
        assert_eq!(game.prepared_remote_projectiles.len(), 2);
        assert_eq!(game.prepared_remote_weapon_groups.len(), 1);
        let offset = (game.tuning.ship(0).radius_pixels * 0.75).trunc();
        for (index, projectile) in game.prepared_remote_projectiles.iter().enumerate() {
            assert_eq!(projectile.presentation_clock.unwrap().pose_tick, 100);
            assert!((projectile.pos.x - source.x).abs() < 0.001);
            let sign = if index == 0 { -1.0 } else { 1.0 };
            assert!((projectile.pos.y - (source.y + sign * offset)).abs() < 0.001);
        }
        assert!(game.pending_remote_birth_effects.is_empty());
        game.prepare_remote_weapon_views(Some(100));
        assert!(game.pending_remote_birth_effects.is_empty());
    }

    #[test]
    fn weapon_birth_late_effects_keep_their_age_and_stale_trails_expire() {
        let (mut game, mut source) = delayed_carrier_fixture(125);
        source.weapon_data = 3;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        game.prepare_remote_weapon_views(Some(118));
        assert!(game.pending_remote_birth_effects.is_empty());
        assert!(
            game.bomb_flashes.is_empty(),
            "an old source flash cannot restart at age zero"
        );
        let live = game.remote_projectiles[0];
        let key = crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&live)
            .unwrap()
            .group_key();
        game.remote_weapon_trails.push_back(RemoteWeaponTrail {
            key,
            source: live.source,
            born_tick: 100,
            position: live.pos,
            kind: RemoteWeaponTrailKind::Bomb,
            presented: false,
        });
        game.last_observed_browser_server_tick = Some(200);
        game.prepare_remote_weapon_views(Some(193));
        assert!(game.prepared_remote_bomb_trails.is_empty());
        assert!(game.remote_weapon_trails.is_empty());
    }

    #[test]
    fn weapon_birth_gap_fill_reserves_the_exact_float_rounded_incoming_budget() {
        for gap_ticks in [9, 15, 18] {
            let (mut game, source) = delayed_carrier_fixture(100);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            let live = game.remote_projectiles[0];
            let capture_tick = 101 + gap_ticks;
            let now = capture_tick + 9;
            let mut recovery = realtime_projectile_state(source, 123, capture_tick);
            recovery.source_server_tick = 100;
            recovery.position_x = live.pos.x + live.vel.x * ((capture_tick - 100) as f32 / 100.0);
            recovery.position_y = live.pos.y + live.vel.y * ((capture_tick - 100) as f32 / 100.0);
            recovery.velocity_x = live.vel.x;
            recovery.velocity_y = live.vel.y;
            let incoming_steps =
                (0.09_f32 / crate::gameplay_rules::weapon_motion::STEP_SECONDS).ceil() as u32;
            assert_eq!(incoming_steps, 10);
            game.weapon_recovery_steps_remaining = incoming_steps + gap_ticks;
            game.last_observed_browser_server_tick = Some(now);
            assert!(game.apply_realtime_projectile_state(77, 88, 12, capture_tick, recovery));
            assert_eq!(game.remote_projectiles.len(), 1);
            assert!(game.remote_projectiles[0].ttl > 0.0);
            assert_eq!(
                game.remote_projectiles[0]
                    .presentation_clock
                    .unwrap()
                    .pose_tick,
                now
            );
            assert_eq!(
                game.weapon_recovery_steps_remaining, gap_ticks,
                "cosmetic gap work must be skipped when its actual cost steals incoming slots"
            );
        }
    }

    #[test]
    fn weapon_birth_flash_and_trails_wait_for_the_same_first_visible_copy() {
        let (mut game, mut source) = delayed_carrier_fixture(100);
        source.weapon_data = 3;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert!(game.bomb_flashes.is_empty());
        for now in 100..=108 {
            game.reset_weapon_recovery_budget();
            game.last_observed_browser_server_tick = Some(now);
            game.update_remote_weapons_at_server_time();
            game.prepare_remote_weapon_views(Some(now - 7));
            if now < 107 {
                assert!(game.bomb_flashes.is_empty());
                assert!(game.prepared_remote_bomb_trails.is_empty());
            } else {
                assert_eq!(game.bomb_flashes.len(), 1);
            }
            if now == 107 {
                assert!(
                    game.prepared_remote_bomb_trails.is_empty(),
                    "future trails cannot lead the first visible bomb"
                );
            }
        }
        assert!(!game.prepared_remote_bomb_trails.is_empty());
        let tip = game.prepared_remote_projectiles[0].pos;
        assert!(
            game.prepared_remote_bomb_trails
                .iter()
                .all(|trail| trail.pos.x <= tip.x)
        );
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        game.prepare_remote_weapon_views(Some(101));
        assert_eq!(
            game.bomb_flashes.len(),
            1,
            "a repeated carrier cannot replay its muzzle effect"
        );
        let mut terminal = realtime_projectile_state(source, 123, 108);
        terminal.source_server_tick = 100;
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 108, terminal));
        assert!(
            game.prepared_remote_projectiles.is_empty(),
            "terminal receipt removes prepared flight immediately"
        );
        game.prepare_remote_weapon_views(Some(101));
        assert!(
            game.prepared_remote_projectiles.is_empty(),
            "historical data cannot extend a terminal"
        );
        assert!(
            !game.prepared_remote_bomb_trails.is_empty(),
            "already presented trails keep their ordinary residual fade"
        );
        assert!(
            game.remote_weapon_trails
                .iter()
                .all(|trail| trail.presented)
        );
    }

    #[test]
    fn weapon_birth_terminal_before_reveal_discards_queued_source_effects() {
        let (mut game, mut source) = delayed_carrier_fixture(100);
        source.weapon_data = 3;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        game.prepare_remote_weapon_views(Some(93));
        assert_eq!(game.pending_remote_birth_effects.len(), 1);
        let mut terminal = realtime_projectile_state(source, 123, 101);
        terminal.source_server_tick = 100;
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 101, terminal));
        game.prepare_remote_weapon_views(Some(100));
        assert!(game.pending_remote_birth_effects.is_empty());
        assert!(game.prepared_remote_projectiles.is_empty());
        assert!(game.bomb_flashes.is_empty());
    }

    #[test]
    fn weapon_birth_terminal_before_first_view_cannot_release_unseen_trails() {
        let (mut game, mut source) = delayed_carrier_fixture(100);
        source.weapon_data = 3;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        game.last_observed_browser_server_tick = Some(101);
        game.reset_weapon_recovery_budget();
        game.update_remote_weapons_at_server_time();
        game.prepare_remote_weapon_views(Some(94));
        assert!(!game.remote_weapon_trails.is_empty());
        assert!(game.prepared_remote_projectiles.is_empty());
        assert!(game.prepared_remote_bomb_trails.is_empty());
        let mut terminal = realtime_projectile_state(source, 123, 102);
        terminal.source_server_tick = 100;
        terminal.terminal = true;
        game.last_observed_browser_server_tick = Some(102);
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 102, terminal));
        game.prepare_remote_weapon_views(Some(95));
        assert!(game.prepared_remote_projectiles.is_empty());
        assert!(game.prepared_remote_bomb_trails.is_empty());
        assert!(game.remote_weapon_trails.is_empty());
    }

    #[test]
    fn weapon_birth_high_delay_late_recovery_and_ownership_cuts_preserve_authority() {
        let (mut game, source) = delayed_carrier_fixture(100);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        game.prepare_remote_weapon_views(Some(72));
        assert_eq!(
            game.prepared_remote_projectiles[0].pos, game.remote_projectiles[0].pos,
            "an impossible warmup must not hide a weapon past the server harm floor"
        );
        let (mut recovered, source) = delayed_carrier_fixture(125);
        let mut state = realtime_projectile_state(source, 123, 125);
        state.source_server_tick = 100;
        assert!(recovered.apply_realtime_projectile_state(77, 88, 12, 125, state));
        recovered.prepare_remote_weapon_views(Some(118));
        assert_eq!(
            recovered.prepared_remote_projectiles[0].pos,
            recovered.remote_projectiles[0].pos
        );
        assert!(recovered.pending_remote_birth_effects.is_empty());
        let cancel = crate::realtime_event::RealtimeOwnerWeaponCancellation {
            source_player_id: 1,
            source_session_id: 101,
            through_shot_group_id: source.shot_group_id,
            owner_life_seq: 7,
            owner_input_sequence: None,
        };
        assert!(recovered.apply_owner_weapon_cancellation(77, 88, 13, cancel));
        assert!(recovered.remote_projectiles.is_empty());
        assert!(recovered.prepared_remote_projectiles.is_empty());
        recovered.prepare_remote_weapon_views(Some(118));
        assert!(recovered.prepared_remote_projectiles.is_empty());
    }

    #[test]
    fn weapon_view_history_captures_birth_and_each_live_shared_physics_tick() {
        let (mut game, mut source) = delayed_carrier_fixture(107);
        game.networked_play = true;
        source.velocity_x = 150.0;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let live = game.remote_projectiles[0];
        let key = crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&live).unwrap();
        let view = game
            .remote_weapon_view_history
            .view(key, 100, 0.006)
            .unwrap();
        assert!(view.pos.distance(live.pos - live.vel * 0.07) < 0.01);
        assert_eq!(view.age, 0.0);
        assert_eq!(view.presentation_clock.unwrap().pose_tick, 100);
        assert_eq!(live.presentation_clock.unwrap().pose_tick, 107);
        game.last_observed_browser_server_tick = Some(109);
        let energy = game.ship.energy;
        game.update_remote_weapons_at_server_time();
        let intermediate = game.remote_weapon_view_history.view(key, 108, 0.0).unwrap();
        assert!(intermediate.pos.distance(live.pos + live.vel * 0.01) < 0.01);
        assert!((intermediate.age - 0.08).abs() < 0.00001);
        let endpoint = game.remote_weapon_view_history.view(key, 109, 0.0).unwrap();
        assert_eq!(endpoint.pos, game.remote_projectiles[0].pos);
        assert!((endpoint.age - game.remote_projectiles[0].age).abs() < 0.00001);
        assert_eq!(
            game.ship.energy, energy,
            "historical capture is never damage authority"
        );
    }

    #[test]
    fn weapon_view_history_active_recovery_never_backdates_a_missing_path() {
        let (mut game, source) = delayed_carrier_fixture(107);
        game.networked_play = true;
        game.network_world.players.remove(&1);
        let mut recovery = realtime_projectile_state(source, 123, 107);
        recovery.source_server_tick = 100;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 107, recovery));
        let key =
            crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&game.remote_projectiles[0])
                .unwrap();
        assert!(
            game.remote_weapon_view_history
                .view(key, 100, 0.0)
                .is_none()
        );
        assert_eq!(
            game.remote_weapon_view_history
                .view(key, 107, 0.0)
                .unwrap()
                .pos,
            vec2(recovery.position_x, recovery.position_y)
        );
        assert!(
            !game.network_world.players.contains_key(&1),
            "a weapon cannot fabricate its former owner"
        );
    }

    #[test]
    fn weapon_view_history_terminal_preserves_prior_flight_without_runtime_resurrection() {
        let (mut game, source) = delayed_carrier_fixture(112);
        game.networked_play = true;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let key =
            crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&game.remote_projectiles[0])
                .unwrap();
        let mut terminal = realtime_projectile_state(source, 123, 110);
        terminal.source_server_tick = 100;
        terminal.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 110, terminal));
        assert!(
            game.remote_projectiles.is_empty(),
            "server terminal removes live simulation immediately"
        );
        assert!(
            game.remote_weapon_view_history
                .view(key, 109, 0.0)
                .is_some()
        );
        assert!(
            game.remote_weapon_view_history
                .view(key, 110, 0.0)
                .is_none()
        );
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert!(game.remote_projectiles.is_empty());
        game.clear_remote_weapon_presentation();
        assert_eq!(
            game.remote_weapon_view_history.views_at(109, 0.0).count(),
            0
        );
    }

    #[test]
    fn weapon_view_history_exact_repel_replaces_only_its_causal_future() {
        let (mut game, source) = delayed_carrier_fixture(112);
        game.networked_play = true;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let live = game.remote_projectiles[0];
        let key = crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&live).unwrap();
        let before = game.remote_weapon_view_history.view(key, 104, 0.0).unwrap();
        let at_mutation = game.remote_weapon_view_history.view(key, 105, 0.0).unwrap();
        let mutation = crate::realtime_event::RealtimeRepelMutationState {
            motion_frozen: false,
            entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
            source_player_id: 1,
            source_session_id: 101,
            source_life_seq: 7,
            source_server_tick: 100,
            source_weapon_data: source.weapon_data,
            source_projectile_index: 0,
            result_weapon_data: source.weapon_data,
            position_x: at_mutation.pos.x,
            position_y: at_mutation.pos.y,
            velocity_x: -400.0,
            velocity_y: 0.0,
            expires_at_tick: 700,
            bounces_remaining: 0,
        };
        game.apply_realtime_repel_mutations(77, 88, 12, 105, repel_mutation_batch(&[mutation]));
        assert_eq!(
            game.remote_weapon_view_history
                .view(key, 104, 0.0)
                .unwrap()
                .pos,
            before.pos
        );
        let after = game.remote_weapon_view_history.view(key, 107, 0.0).unwrap();
        assert!(after.pos.distance(at_mutation.pos + vec2(-8.0, 0.0)) < 0.01);
        assert_eq!(after.presentation_clock.unwrap().expiry_tick, 700);
        assert_eq!(
            game.remote_weapon_view_history
                .view(key, 104, 0.0)
                .unwrap()
                .last_repel_mutation_event_id,
            None
        );
        assert_eq!(after.last_repel_mutation_event_id, Some(12));
        assert_eq!(
            game.remote_projectiles[0]
                .presentation_clock
                .unwrap()
                .pose_tick,
            112
        );
    }

    #[test]
    fn weapon_view_history_frozen_recovery_holds_without_opening_player_authority() {
        let (mut game, source) = delayed_carrier_fixture(120);
        game.networked_play = true;
        let mut recovery = realtime_projectile_state(source, 123, 110);
        recovery.source_server_tick = 100;
        recovery.motion_frozen = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 12, 110, recovery));
        let key =
            crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&game.remote_projectiles[0])
                .unwrap();
        let before = game
            .remote_weapon_view_history
            .view(key, 115, 0.009)
            .unwrap();
        assert_eq!(before.pos, vec2(recovery.position_x, recovery.position_y));
        assert!((before.age - 0.15).abs() < 0.00001);
        assert_eq!(
            before.presentation_clock.unwrap().render_partial_seconds,
            0.0
        );
        let energy = game.ship.energy;
        game.last_observed_browser_server_tick = Some(125);
        game.update_remote_weapons_at_server_time();
        let after = game
            .remote_weapon_view_history
            .view(key, 123, 0.009)
            .unwrap();
        assert_eq!(after.pos, before.pos);
        assert!((after.age - 0.23).abs() < 0.00001);
        assert_eq!(game.ship.energy, energy);
        recovery.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 13, 124, recovery));
        assert!(
            game.remote_weapon_view_history
                .view(key, 123, 0.0)
                .is_some()
        );
        assert!(
            game.remote_weapon_view_history
                .view(key, 124, 0.0)
                .is_none()
        );
    }

    #[test]
    fn weapon_view_history_keeps_the_last_live_tick_without_reviving_expired_carriers() {
        let (mut game, source) = delayed_carrier_fixture(648);
        game.networked_play = true;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let before = game.remote_projectiles[0];
        let key = crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&before).unwrap();
        assert_eq!(before.presentation_clock.unwrap().expiry_tick, 650);
        game.last_observed_browser_server_tick = Some(650);
        game.update_remote_weapons_at_server_time();
        assert!(game.remote_projectiles.is_empty());
        let last_live = game
            .remote_weapon_view_history
            .view(key, 649, 0.0)
            .expect("the final known live centisecond remains part of historical flight");
        assert!(last_live.pos.distance(before.pos + before.vel * 0.01) < 0.01);
        assert!(
            game.remote_weapon_view_history
                .view(key, 650, 0.0)
                .is_none()
        );
        let (mut expired, source) = delayed_carrier_fixture(650);
        expired.networked_play = true;
        expired.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert!(expired.remote_projectiles.is_empty());
        assert_eq!(
            expired
                .remote_weapon_view_history
                .views_at(649, 0.0)
                .count(),
            0,
            "an already-expired late carrier cannot create a new ghost view"
        );
    }

    #[test]
    fn recovered_own_decoy_keeps_its_name_hidden_across_a_successor_life() {
        let (mut game, _) = delayed_carrier_fixture(130);
        install_realtime_authority_scope_with_life(&mut game, 7);
        let source = realtime_weapon(9, 1, 6);
        let mut recovery = realtime_projectile_state(source, 123, 130);
        recovery.source_input_sequence = Some(17);
        recovery.source_server_tick = 100;
        assert!(game.apply_realtime_projectile_state(77, 88, 15, 130, recovery));
        assert_eq!(game.decoys.len(), 1);
        assert!(game.decoys[0].remote_source.is_some());
        assert!(!game.decoy_name_label_visible(&game.decoys[0]));

        install_realtime_authority_scope_with_life(&mut game, 8);
        assert!(!game.decoy_name_label_visible(&game.decoys[0]));
        game.install_network_test_player_identity(1);
        assert!(game.decoy_name_label_visible(&game.decoys[0]));
    }

    #[test]
    fn recovered_own_decoy_still_mirrors_the_local_ship_turn() {
        let (mut game, _) = delayed_carrier_fixture(130);
        install_realtime_authority_scope_with_life(&mut game, 7);
        let source = realtime_weapon(9, 1, 6);
        let mut recovery = realtime_projectile_state(source, 123, 130);
        recovery.source_input_sequence = Some(17);
        recovery.source_server_tick = 100;
        assert!(game.apply_realtime_projectile_state(77, 88, 15, 130, recovery));
        game.ship.orientation = 0.25;
        game.update_decoys_at_server_time();
        assert_eq!(game.decoys.len(), 1);
        assert_eq!(game.decoys[0].orientation, 0.75);
    }

    #[test]
    fn original_carrier_catches_up_live_flight_and_discards_expired_flight() {
        for actor_wait in [false, true] {
            for observed in [200, 650, 800] {
                let (mut game, source) = delayed_carrier_fixture(observed);
                if actor_wait {
                    game.network_world.players.remove(&1);
                    game.last_observed_browser_server_tick = Some(100);
                }
                game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
                if actor_wait {
                    assert!(game.remote_projectiles.is_empty());
                    game.last_observed_browser_server_tick = Some(observed);
                    game.network_world
                        .players
                        .insert(1, remote_presentation_actor(1, 7));
                    game.drain_pending_remote_weapon_presentations();
                    assert!(game.remote_weapon_presentations.is_empty());
                }
                if observed >= 650 {
                    assert!(
                        game.remote_projectiles.is_empty(),
                        "expired carrier: actor_wait={actor_wait}"
                    );
                } else {
                    let projectile = game.remote_projectiles[0];
                    assert!((projectile.age - 1.0).abs() < 0.0001);
                    assert!((projectile.ttl - 4.5).abs() < 0.0001);
                    assert!(
                        projectile
                            .pos
                            .distance(vec2(source.x, source.y) + projectile.vel)
                            < 0.01
                    );
                    assert!(game.bullet_impacts.is_empty());
                }
            }
        }
    }

    #[test]
    fn original_carrier_uses_wrapping_source_ticks_and_terrain_without_player_hits() {
        let (mut game, source) = delayed_carrier_fixture(49);
        game.apply_realtime_weapon_visual(77, 88, 10, u32::MAX - 50, source);
        assert!((game.remote_projectiles[0].age - 1.0).abs() < 0.0001);
        // A hull in the catch-up ray cannot be a client-owned damage event.
        let (mut game, source) = delayed_carrier_fixture(200);
        game.ship.pos = vec2(1100.0, 1000.0);
        let energy = game.ship.energy;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles.len(), 1);
        assert_eq!(game.ship.energy, energy);
        let (mut game, source) = delayed_carrier_fixture(200);
        game.maps[game.map_index].set_tile(65, 62, 1);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert!(game.remote_projectiles.is_empty());
        assert!(
            game.bullet_impacts.is_empty(),
            "catch-up must not replay effects"
        );
    }

    #[test]
    fn original_carrier_preserves_exact_repel_extension_and_does_not_double_age_recovery() {
        let (mut game, source) = delayed_carrier_fixture(800);
        let mutation = crate::realtime_event::RealtimeRepelMutationState {
            motion_frozen: false,
            entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
            source_player_id: 1,
            source_session_id: 101,
            source_life_seq: 7,
            source_server_tick: 100,
            source_weapon_data: source.weapon_data,
            source_projectile_index: 0,
            result_weapon_data: source.weapon_data,
            position_x: 1200.0,
            position_y: 1000.0,
            velocity_x: 100.0,
            velocity_y: 0.0,
            expires_at_tick: 1050,
            bounces_remaining: 0,
        };
        // Source would have expired at 650. The server extended it at tick 500.
        game.apply_realtime_repel_mutations(77, 88, 12, 500, repel_mutation_batch(&[mutation]));
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles.len(), 1);
        let projectile = game.remote_projectiles[0];
        assert!((projectile.ttl - 2.5).abs() < 0.001);
        assert!((projectile.age - 7.0).abs() < 0.001);
        assert!(projectile.pos.distance(vec2(1500.0, 1000.0)) < 0.02);
        assert_eq!(projectile.authoritative_repel_state, Some(mutation));
        // Repeated original carriers cannot rewind a recovered/mutated child.
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles, vec![projectile]);

        let (mut game, source) = delayed_carrier_fixture(800);
        let mut state = realtime_projectile_state(source, 123, 750);
        state.source_server_tick = 100;
        state.position_x = 1600.0;
        state.position_y = 1000.0;
        state.expiry_tick = Some(1050);
        assert!(game.apply_realtime_projectile_state(77, 88, 13, 750, state));
        let before = game.remote_projectiles[0];
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles, vec![before]);
        // An exact terminal still prevents a later original from resurrecting it.
        state.terminal = true;
        assert!(game.apply_realtime_projectile_state(77, 88, 14, 800, state));
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert!(game.remote_projectiles.is_empty());
    }

    #[test]
    fn original_carrier_public_repel_uses_position_at_impulse_and_extended_deadline() {
        let (mut game, source) = delayed_carrier_fixture(800);
        // The shot has left its fire origin by the impulse. Only its position
        // at that capture tick is eligible for the public radius test.
        let mut original = remote_weapon_projectiles(
            game.capture_remote_weapon_source(
                RemoteWeaponSourceIdentity::authenticated(1, 101, 7).unwrap(),
                RemoteWeaponArenaScope::authenticated(77, 88),
                vec2(source.x, source.y),
                Vec2::ZERO,
                source.orientation,
                1,
                0,
            )
            .unwrap(),
            source.weapon_data,
            game.tuning.ship(0),
            &game.tuning,
        )[0];
        game.catch_up_remote_projectile_presentation(&mut original, 4.0);
        let fact = RemoteRepelPresentationFact {
            arena_scope: RemoteWeaponArenaScope::authenticated(77, 88).unwrap(),
            event_id: 12,
            server_tick: 500,
            origin: original.pos - vec2(1.0, 0.0),
            source_frequency: 2,
        };
        game.observe_remote_realtime_event(fact.event_id);
        game.remote_repel_presentation_history.push_back(fact);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles.len(), 1);
        assert!((game.remote_projectiles[0].ttl - 2.5).abs() < 0.001);
        let expected = original.pos + vec2(game.tuning.repel_speed, 0.0) * 3.0;
        assert!(game.remote_projectiles[0].pos.distance(expected) < 0.1);
    }
    #[test]
    fn original_carriers_respect_each_authored_weapon_lifetime() {
        for weapon in [1, 2, 3, 4, 6, 7, 8, 3 | 0x8000] {
            for expired in [false, true] {
                let (mut game, mut source) = delayed_carrier_fixture(150);
                game.tuning.bullet_lifetime = 1.0;
                game.tuning.bomb_lifetime = 2.0;
                game.tuning.mine_lifetime = 3.0;
                game.tuning.decoy_lifetime = 4.0;
                source.weapon_data = weapon;
                let lifetime = if weapon == 6 {
                    4.0
                } else if weapon & 0x8000 != 0 {
                    3.0
                } else if matches!(weapon, 3 | 4 | 8) {
                    2.0
                } else {
                    1.0
                };
                if expired {
                    game.last_observed_browser_server_tick = Some(100 + (lifetime * 100.0) as u32);
                }
                game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
                if expired {
                    assert!(game.remote_projectiles.is_empty(), "weapon={weapon}");
                    assert!(game.decoys.is_empty(), "weapon={weapon}");
                    assert!(game.bomb_flashes.is_empty());
                } else if weapon == 6 {
                    assert_eq!(game.decoys.len(), 1);
                    assert!((game.decoys[0].age - 0.5).abs() < 0.001);
                    assert!((game.decoys[0].ttl - (lifetime - 0.5)).abs() < 0.001);
                } else {
                    assert!(!game.remote_projectiles.is_empty(), "weapon={weapon}");
                    for projectile in &game.remote_projectiles {
                        assert!((projectile.age - 0.5).abs() < 0.001);
                        assert!((projectile.ttl - (lifetime - 0.5)).abs() < 0.001);
                    }
                }
            }
        }
    }

    #[test]
    fn original_carrier_replays_multiple_exact_repels_and_honors_final_expiry() {
        for observed in [1200, 1450, 1500] {
            let (mut game, source) = delayed_carrier_fixture(observed);
            let mut state = crate::realtime_event::RealtimeRepelMutationState {
                motion_frozen: false,
                entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
                source_player_id: 1,
                source_session_id: 101,
                source_life_seq: 7,
                source_server_tick: 100,
                source_weapon_data: source.weapon_data,
                source_projectile_index: 0,
                result_weapon_data: source.weapon_data,
                position_x: 1200.0,
                position_y: 1000.0,
                velocity_x: 100.0,
                velocity_y: 0.0,
                expires_at_tick: 1050,
                bounces_remaining: 0,
            };
            game.apply_realtime_repel_mutations(77, 88, 12, 500, repel_mutation_batch(&[state]));
            state.position_x = 2000.0;
            state.expires_at_tick = 1450;
            game.apply_realtime_repel_mutations(77, 88, 13, 900, repel_mutation_batch(&[state]));
            // Insert the original into an actor wait after both mutations;
            // releasing it must follow the same reconstruction path.
            game.network_world.players.remove(&1);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            game.network_world
                .players
                .insert(1, remote_presentation_actor(1, 7));
            game.drain_pending_remote_weapon_presentations();
            if observed >= 1450 {
                assert!(game.remote_projectiles.is_empty());
            } else {
                assert_eq!(game.remote_projectiles.len(), 1);
                let projectile = game.remote_projectiles[0];
                assert!((projectile.ttl - 2.5).abs() < 0.001);
                assert!((projectile.age - 11.0).abs() < 0.001);
                assert!(projectile.pos.distance(vec2(2300.0, 1000.0)) < 0.05);
                assert_eq!(projectile.last_repel_mutation_event_id, Some(13));
            }
        }
    }
    #[test]
    fn original_carrier_waits_for_server_clock_instead_of_assuming_a_fresh_lifetime() {
        for observed in [200, 800] {
            let (mut game, source) = delayed_carrier_fixture(200);
            game.last_observed_browser_server_tick = None;
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            assert!(game.remote_projectiles.is_empty());
            assert_eq!(
                game.remote_weapon_presentations
                    .pending_for(RemoteWeaponPresentationSuppressionReason::ServerClockUnavailable),
                1
            );
            game.drain_pending_remote_weapon_presentations();
            assert!(game.remote_projectiles.is_empty());
            assert_eq!(game.remote_weapon_presentations.len(), 1);
            game.last_observed_browser_server_tick = Some(observed);
            game.drain_pending_remote_weapon_presentations();
            assert!(game.remote_weapon_presentations.is_empty());
            if observed == 800 {
                assert!(game.remote_projectiles.is_empty());
            } else {
                assert!((game.remote_projectiles[0].age - 1.0).abs() < 0.001);
            }
        }
    }
    #[test]
    fn recovered_post_repel_state_cannot_be_rewound_by_an_older_mutation() {
        let (mut game, source) = delayed_carrier_fixture(800);
        let mut recovered = realtime_projectile_state(source, 123, 750);
        recovered.source_server_tick = 100;
        recovered.position_x = 1600.0;
        recovered.position_y = 1000.0;
        recovered.expiry_tick = Some(1050);
        assert!(game.apply_realtime_projectile_state(77, 88, 15, 750, recovered));
        let before = game.remote_projectiles[0];
        let mut mutation = crate::realtime_event::RealtimeRepelMutationState {
            motion_frozen: false,
            entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
            source_player_id: 1,
            source_session_id: 101,
            source_life_seq: 7,
            source_server_tick: 100,
            source_weapon_data: source.weapon_data,
            source_projectile_index: 0,
            result_weapon_data: source.weapon_data,
            position_x: 1200.0,
            position_y: 1000.0,
            velocity_x: 100.0,
            velocity_y: 0.0,
            expires_at_tick: 1050,
            bounces_remaining: 0,
        };
        game.apply_realtime_repel_mutations(77, 88, 12, 500, repel_mutation_batch(&[mutation]));
        assert_eq!(game.remote_projectiles, vec![before]);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.remote_projectiles, vec![before]);
        mutation.position_x = 1700.0;
        mutation.expires_at_tick = 1350;
        game.apply_realtime_repel_mutations(77, 88, 16, 800, repel_mutation_batch(&[mutation]));
        assert_eq!(game.remote_projectiles[0].pos, vec2(1700.0, 1000.0));
        assert!((game.remote_projectiles[0].ttl - 5.5).abs() < 0.001);
    }
    #[test]
    fn recovered_state_keeps_committed_repel_witness_without_replaying_old_impulses() {
        for weapon in [6, 8] {
            for event_base in [0, u32::MAX - 12] {
                for recovery_after_repel in [false, true] {
                    for first_seen_in_recovery in [false, true] {
                        if first_seen_in_recovery && !recovery_after_repel {
                            continue;
                        }
                        let (mut game, mut source) = delayed_carrier_fixture(100);
                        source.weapon_data = weapon;
                        source.velocity_x = 100.0;
                        let event = |offset: u32| event_base.wrapping_add(offset);
                        if !first_seen_in_recovery {
                            game.apply_realtime_weapon_visual(77, 88, event(10), 100, source);
                        }
                        let mutation = crate::realtime_event::RealtimeRepelMutationState {
                            motion_frozen: false,
                            entity_kind: if weapon == 6 {
                                crate::realtime_event::REPEL_MUTATION_DECOY
                            } else {
                                crate::realtime_event::REPEL_MUTATION_PROJECTILE
                            },
                            source_player_id: source.source_player_id,
                            source_session_id: source.source_session_id,
                            source_life_seq: source.source_life_seq,
                            source_server_tick: 100,
                            source_weapon_data: weapon,
                            source_projectile_index: 0,
                            result_weapon_data: weapon,
                            position_x: 1000.0,
                            position_y: 1000.0,
                            velocity_x: 500.0,
                            velocity_y: 0.0,
                            expires_at_tick: 1000,
                            bounces_remaining: 0,
                        };
                        game.last_observed_browser_server_tick = Some(120);
                        game.apply_realtime_repel_mutations(
                            77,
                            88,
                            event(12),
                            120,
                            repel_mutation_batch(&[mutation]),
                        );
                        let capture = if recovery_after_repel { 130 } else { 110 };
                        let recovery_event = event(if recovery_after_repel { 15 } else { 11 });
                        let mut recovery = realtime_projectile_state(source, 123, capture);
                        recovery.source_server_tick = 100;
                        recovery.position_x = 1500.0;
                        recovery.position_y = 1000.0;
                        recovery.velocity_x = 200.0;
                        recovery.velocity_y = 0.0;
                        recovery.expiry_tick = Some(1200);
                        game.last_observed_browser_server_tick = Some(130);
                        if !recovery_after_repel {
                            game.weapon_recovery_steps_remaining = 0;
                        }
                        assert_eq!(
                            game.apply_realtime_projectile_state(
                                77,
                                88,
                                recovery_event,
                                capture,
                                recovery,
                            ),
                            recovery_after_repel
                        );
                        assert_eq!(
                            if weapon == 6 {
                                game.decoys.len()
                            } else {
                                game.remote_projectiles.len()
                            },
                            1,
                            "obsolete recovery cannot erase an already-applied live impulse"
                        );
                        game.reset_weapon_recovery_budget();
                        if weapon == 6 {
                            game.update_decoys_at_server_time();
                        } else {
                            game.update_remote_weapons_at_server_time();
                        }
                        let (pos, vel, witness, last_event, delivery, expiry) = if weapon == 6 {
                            let decoy = &game.decoys[0];
                            (
                                decoy.pos,
                                decoy.vel,
                                decoy.authoritative_repel_state,
                                decoy.last_repel_mutation_event_id,
                                decoy.remote_source.unwrap().realtime_event_id,
                                decoy.presentation_clock.unwrap().expiry_tick,
                            )
                        } else {
                            let projectile = game.remote_projectiles[0];
                            (
                                projectile.pos,
                                projectile.vel,
                                projectile.authoritative_repel_state,
                                projectile.last_repel_mutation_event_id,
                                projectile.source.realtime_event_id,
                                projectile.presentation_clock.unwrap().expiry_tick,
                            )
                        };
                        assert_eq!(witness, Some(mutation));
                        assert_eq!(last_event, Some(event(12)));
                        assert_eq!(
                            delivery,
                            Some(if recovery_after_repel {
                                recovery_event
                            } else {
                                event(10)
                            })
                        );
                        if recovery_after_repel {
                            assert_eq!(pos, vec2(1500.0, 1000.0));
                            assert_eq!(vel, vec2(200.0, 0.0));
                            assert_eq!(expiry, 1200, "new state owns its deadline");
                        } else {
                            assert!(pos.distance(vec2(1050.0, 1000.0)) < 0.01);
                            assert_eq!(vel, vec2(500.0, 0.0));
                            assert_eq!(expiry, 1000, "later Repel must still apply");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn received_weapon_clock_survives_scheduler_caps_and_expires_on_server_deadline() {
        for pause_ticks in [5, 13, 50, 600] {
            let (mut game, source) = delayed_carrier_fixture(100);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            assert_eq!(game.remote_projectiles.len(), 1);
            let start = game.remote_projectiles[0].pos;
            let velocity = game.remote_projectiles[0].vel;
            // The local ship scheduler intentionally discards long pauses.
            let mut ship_clock = crate::fixed_step::FixedGameplayClock::default();
            assert!(ship_clock.advance(pause_ticks as f32 / 100.0) <= 8);
            game.last_observed_browser_server_tick = Some(100 + pause_ticks);
            game.update_remote_weapons_at_server_time();
            if pause_ticks >= 550 {
                assert!(game.remote_projectiles.is_empty());
                assert!(
                    !game.projectile_refresh_needed,
                    "expired shots need no replay"
                );
            } else if pause_ticks > 20 {
                assert!(game.remote_projectiles.is_empty());
                assert!(
                    game.projectile_refresh_needed,
                    "recover live poses after a long pause"
                );
            } else {
                let projectile = game.remote_projectiles[0];
                assert!(
                    projectile
                        .pos
                        .distance(start + velocity * (pause_ticks as f32 / 100.0))
                        < 0.01
                );
                assert!((projectile.ttl - (5.5 - pause_ticks as f32 / 100.0)).abs() < 0.0001);
                game.update_remote_weapons_at_server_time();
                assert_eq!(
                    game.remote_projectiles[0], projectile,
                    "one frame cannot age twice"
                );
            }
        }
    }

    #[test]
    fn received_weapon_clock_wrap_and_aggregate_recovery_work_are_bounded() {
        let (mut game, source) = delayed_carrier_fixture(0x7fff_fffe);
        game.apply_realtime_weapon_visual(77, 88, 10, 0x7fff_fffe, source);
        let before = game.remote_projectiles[0];
        game.last_observed_browser_server_tick = Some(3);
        game.update_remote_weapons_at_server_time();
        assert!((game.remote_projectiles[0].age - 0.05).abs() < 0.0001);
        assert!(
            game.remote_projectiles[0]
                .pos
                .distance(before.pos + before.vel * 0.05)
                < 0.01
        );
        // Enough copies to exceed the explicit aggregate substep ceiling.
        game.remote_projectiles = vec![game.remote_projectiles[0]; 256];
        game.last_observed_browser_server_tick = Some(23);
        game.update_remote_weapons_at_server_time();
        assert!(game.projectile_refresh_needed);
        assert_eq!(game.remote_projectiles.len(), 2048 / 20);
    }
    #[test]
    fn decoy_server_clock_recovers_after_pause_and_converges_reordered_carriers() {
        let (mut game, mut source) = delayed_carrier_fixture(100);
        source.weapon_data = 6;
        source.velocity_x = 100.0;
        game.tuning.decoy_lifetime = 4.0;
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(game.decoys.len(), 1);
        let start = game.decoys[0].pos;
        game.last_observed_browser_server_tick = Some(105);
        game.update_decoys_at_server_time();
        assert!(game.decoys[0].pos.distance(start + vec2(5.0, 0.0)) < 0.001);
        let pose = game.decoys[0].pos;
        game.update_decoys(0.05);
        assert_eq!(
            game.decoys[0].pos, pose,
            "ship cadence cannot age a clock-bound Decoy"
        );
        game.last_observed_browser_server_tick = Some(150);
        game.update_decoys_at_server_time();
        assert!(game.decoys.is_empty());
        assert!(game.projectile_refresh_needed);
        let mut state = realtime_projectile_state(source, 123, 149);
        state.source_server_tick = 100;
        state.position_x = 1500.0;
        state.position_y = 1000.0;
        state.velocity_x = 200.0;
        state.velocity_y = 0.0;
        state.expiry_tick = Some(500);
        assert!(game.apply_realtime_projectile_state(77, 88, 20, 149, state));
        assert_eq!(game.decoys.len(), 1);
        assert!(game.decoys[0].pos.distance(vec2(1502.0, 1000.0)) < 0.001);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        assert_eq!(
            game.decoys.len(),
            1,
            "old original cannot duplicate a recovered Decoy"
        );
        assert!(!game.apply_realtime_projectile_state(77, 88, 19, 148, state));
        game.projectile_refresh_needed = false;
        game.last_observed_browser_server_tick = Some(500);
        game.update_decoys_at_server_time();
        assert!(game.decoys.is_empty());
        assert!(
            !game.projectile_refresh_needed,
            "an expired Decoy needs no recovery replay"
        );
    }
    #[test]
    fn recovered_fragment_uses_deadline_age_and_keeps_carrier_identity_across_repels() {
        let (mut game, mut source) = delayed_carrier_fixture(300);
        source.weapon_data = encode_weapon_data(3, 1, true, 1, 1, false);
        let fragment_data = encode_weapon_data(15, 1, false, 1, 0, false);
        let mut state = realtime_projectile_state(source, 123, 300);
        state.source_server_tick = 100;
        state.weapon_data = fragment_data;
        state.expiry_tick = Some(600);
        assert!(game.apply_realtime_projectile_state(77, 88, 20, 300, state));
        assert!(
            (game.remote_projectiles[0].age - 2.5).abs() < 0.001,
            "fragment inactivity derives from its refreshed deadline, not parent flight time"
        );
        for (event, capture) in [(30, 310), (40, 320)] {
            game.last_observed_browser_server_tick = Some(capture + 5);
            let mutation = crate::realtime_event::RealtimeRepelMutationState {
                motion_frozen: false,
                entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
                source_player_id: source.source_player_id,
                source_session_id: source.source_session_id,
                source_life_seq: source.source_life_seq,
                source_server_tick: 100,
                source_weapon_data: fragment_data,
                source_projectile_index: 0,
                result_weapon_data: fragment_data,
                position_x: 1200.0,
                position_y: 1000.0,
                velocity_x: 100.0,
                velocity_y: 0.0,
                expires_at_tick: capture + 550,
                bounces_remaining: 0,
            };
            game.apply_realtime_repel_mutations(
                77,
                88,
                event,
                capture,
                repel_mutation_batch(&[mutation]),
            );
            let fragment = game.remote_projectiles[0];
            assert_eq!(fragment.source.realtime_server_tick, Some(100));
            assert_eq!(fragment.last_repel_mutation_event_id, Some(event));
            assert!((fragment.age - 0.05).abs() < 0.001);
            assert!((fragment.ttl - 5.45).abs() < 0.001);
            assert!(fragment.pos.distance(vec2(1205.0, 1000.0)) < 0.001);
            assert_eq!(
                fragment.presentation_clock.unwrap().expiry_tick,
                capture + 550
            );
        }
    }

    #[test]
    fn dead_life_weapon_presentation_retires_trails_and_ship_explosion_while_shots_keep_flying() {
        for weapon_type in [2, 3] {
            let (mut game, mut source) = delayed_carrier_fixture(100);
            source.weapon_data = weapon_type;
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            let start = game.remote_projectiles[0].pos;
            let scope = game.client_authority.view().scope;
            let dead = crate::protocol::AuthorityScopeIdentity {
                life_seq: scope.life_seq.map(|life| life + 1),
                alive: Some(false),
                ..scope
            };
            game.client_authority.observe_private_lifecycle(
                crate::game::lifecycle_sequencer::ClientAuthoritySource::Reliable,
                dead,
                Some(100),
                2.0,
            );
            game.start_player_respawn_with_delay(game.ship.pos, 2.0);
            assert!(!game.gameplay_step_ready());
            for frame in 1..=50 {
                game.last_observed_browser_server_tick = Some(100 + frame * 2);
                // Production's render-side update runs even when the dead
                // life cannot run a canonical ship/input step.
                game.update_remote_presentation(0.02);
            }
            assert_eq!(game.remote_projectiles.len(), 1);
            assert!(game.remote_projectiles[0].pos.distance(start) > 50.0);
            let trail_count = if weapon_type == 2 {
                game.bullet_trails.len() + game.prepared_remote_bullet_trails.len()
            } else {
                game.bomb_trails.len() + game.prepared_remote_bomb_trails.len()
            };
            assert!(trail_count > 0);
            assert!(
                trail_count <= 9,
                "weapon {weapon_type}: {trail_count} trails must not form a second-long ray during death"
            );
            assert!(
                game.ship_explosions.is_empty(),
                "the 0.8 s death animation must not freeze for the respawn wait"
            );
            assert_eq!(
                game.client_authority.view().scope,
                dead,
                "cosmetics cannot advance life or enable gameplay"
            );
            assert_eq!(game.ship.energy, 0.0);
        }
    }

    #[test]
    fn held_authoritative_clock_keeps_exact_pose_until_expiry() {
        for pause in [5, 50, 549, 550] {
            let (mut game, source) = delayed_carrier_fixture(100);
            game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
            let before = game.remote_projectiles[0];
            game.remote_projectiles[0]
                .presentation_clock
                .as_mut()
                .unwrap()
                .motion_frozen = true;
            game.last_observed_browser_server_tick = Some(100 + pause);
            game.update_remote_weapons_at_server_time();
            if pause == 550 {
                assert!(game.remote_projectiles.is_empty());
            } else {
                let after = game.remote_projectiles[0];
                assert_eq!(
                    after.pos, before.pos,
                    "server-held flight cannot drift while a collision waits"
                );
                assert_eq!(
                    after.vel, before.vel,
                    "holding cannot destroy the resume velocity"
                );
                assert!((after.ttl - (5.5 - pause as f32 / 100.0)).abs() < 0.0001);
                assert!(after.presentation_clock.unwrap().motion_frozen);
                assert!(
                    !game.projectile_refresh_needed,
                    "a held pose requires no physics recovery work"
                );
            }
        }
    }

    #[test]
    fn held_weapon_draw_cannot_project_through_the_contact() {
        let (mut game, source) = delayed_carrier_fixture(100);
        game.apply_realtime_weapon_visual(77, 88, 10, 100, source);
        let projectile = &mut game.remote_projectiles[0];
        let clock = projectile.presentation_clock.as_mut().unwrap();
        clock.motion_frozen = true;
        clock.render_partial_seconds = 0.009;
        let before = game.remote_projectiles[0];
        let drawn =
            remote_projectile_presentation(&before, 0.016, game.map(), &game.bricks, &game.tuning);
        assert_eq!(
            drawn.position, before.pos,
            "fractional drawing must honor held flight too"
        );
        assert_eq!(game.remote_projectiles[0], before);
    }

    #[test]
    fn held_wire_state_recovers_without_flight_then_resumes_and_rejects_stale_copies() {
        use crate::realtime_event::{AuthenticatedRealtimeEvent, RealtimeEventPayload};
        for kind in [1, 3, 7, 8, 15] {
            let (mut game, mut source) = delayed_carrier_fixture(150);
            source.weapon_data = if kind == 15 {
                encode_weapon_data(3, 3, false, 3, 1, false)
            } else {
                kind
            };
            let mut held = realtime_projectile_state(source, 123, 120);
            held.source_server_tick = 100;
            held.weapon_data = if kind == 15 {
                encode_weapon_data(15, 3, false, 1, 0, false)
            } else {
                kind
            };
            held.position_x = 1200.0;
            held.position_y = 1000.0;
            held.motion_frozen = true;
            if kind == 7 {
                held.behavior = crate::realtime_event::PROJECTILE_BEHAVIOR_BURST_ACTIVE;
            }
            let wire = AuthenticatedRealtimeEvent {
                server_session_id: 77,
                arena_stream_id: 88,
                event_id: 15,
                server_tick: 120,
                payload: RealtimeEventPayload::ProjectileState(held),
            }
            .encode();
            let parsed = AuthenticatedRealtimeEvent::parse(&wire).unwrap();
            let RealtimeEventPayload::ProjectileState(held) = parsed.payload else {
                unreachable!()
            };
            assert!(game.apply_realtime_projectile_state(77, 88, 15, 120, held));
            assert_eq!(game.remote_projectiles.len(), 1);
            assert_eq!(
                game.remote_projectiles[0].pos,
                vec2(1200.0, 1000.0),
                "recovery must not replay held flight"
            );
            assert!((game.remote_projectiles[0].ttl - 3.5).abs() < 0.001);
            game.last_observed_browser_server_tick = Some(155);
            game.update_remote_weapons_at_server_time();
            let before = game.remote_projectiles[0];
            let fact = RemoteRepelPresentationFact {
                arena_scope: RemoteWeaponArenaScope::authenticated(77, 88).unwrap(),
                event_id: 16,
                server_tick: 155,
                origin: before.pos - vec2(1.0, 0.0),
                source_frequency: source.frequency + 1,
            };
            game.apply_remote_presentation_repel_fact(fact, 0, 0, false);
            assert_eq!(
                game.remote_projectiles[0], before,
                "approximate public Repel cannot move committed held flight"
            );
            let mut resume = held;
            resume.motion_frozen = false;
            resume.position_x = 1210.0;
            game.last_observed_browser_server_tick = Some(170);
            assert!(game.apply_realtime_projectile_state(77, 88, 20, 170, resume));
            assert!(
                !game.remote_projectiles[0]
                    .presentation_clock
                    .unwrap()
                    .motion_frozen
            );
            game.last_observed_browser_server_tick = Some(175);
            game.update_remote_weapons_at_server_time();
            assert!(
                game.remote_projectiles[0]
                    .pos
                    .distance(vec2(1215.0, 1000.0))
                    < 0.01
            );
            let resumed = game.remote_projectiles[0];
            assert!(!game.apply_realtime_projectile_state(77, 88, 15, 120, held));
            assert_eq!(game.remote_projectiles[0], resumed);
            let mut terminal = resume;
            terminal.terminal = true;
            game.apply_realtime_projectile_state(77, 88, 21, 180, terminal);
            assert!(game.remote_projectiles.is_empty());
            assert!(!game.apply_realtime_projectile_state(77, 88, 22, 181, held));
            assert!(
                game.remote_projectiles.is_empty(),
                "a late held state cannot revive a consumed child/group"
            );
        }
    }

    #[test]
    fn same_tick_hold_resume_uses_exact_newer_event_order_including_wrap() {
        for (held_event, resume_event) in [(15, 16), (u32::MAX, 1)] {
            let (mut game, source) = delayed_carrier_fixture(120);
            let mut held = realtime_projectile_state(source, 123, 120);
            held.motion_frozen = true;
            assert!(game.apply_realtime_projectile_state(77, 88, held_event, 120, held));
            let mut resumed = held;
            resumed.motion_frozen = false;
            assert!(
                game.apply_realtime_projectile_state(77, 88, resume_event, 120, resumed),
                "a later server event can resume at the same capture tick"
            );
            let before = game.remote_projectiles[0];
            assert!(!before.presentation_clock.unwrap().motion_frozen);
            assert!(!game.apply_realtime_projectile_state(77, 88, held_event, 120, held));
            assert!(!game.apply_realtime_projectile_state(77, 88, resume_event, 120, held));
            assert!(
                !game.apply_realtime_projectile_state(
                    77,
                    88,
                    resume_event.wrapping_add(1),
                    119,
                    held
                ),
                "a newer delivery id cannot make an older capture current"
            );
            assert_eq!(game.remote_projectiles[0], before);
        }
    }

    #[test]
    fn exact_later_repel_resumes_held_flight_without_older_recovery_rollback() {
        let (mut game, source) = delayed_carrier_fixture(150);
        let mut held = realtime_projectile_state(source, 123, 120);
        held.source_server_tick = 100;
        held.motion_frozen = true;
        held.position_x = 1200.0;
        held.position_y = 1000.0;
        assert!(game.apply_realtime_projectile_state(77, 88, 15, 120, held));
        let mutation = crate::realtime_event::RealtimeRepelMutationState {
            motion_frozen: false,
            entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
            source_player_id: source.source_player_id,
            source_session_id: source.source_session_id,
            source_life_seq: source.source_life_seq,
            source_server_tick: 100,
            source_weapon_data: source.weapon_data,
            source_projectile_index: 0,
            result_weapon_data: source.weapon_data,
            position_x: 1300.0,
            position_y: 1000.0,
            velocity_x: 50.0,
            velocity_y: 0.0,
            expires_at_tick: 1000,
            bounces_remaining: 0,
        };
        game.last_observed_browser_server_tick = Some(170);
        game.apply_realtime_repel_mutations(77, 88, 20, 160, repel_mutation_batch(&[mutation]));
        let after = game.remote_projectiles[0];
        assert!(!after.presentation_clock.unwrap().motion_frozen);
        assert!(after.pos.distance(vec2(1305.0, 1000.0)) < 0.01);
        assert_eq!(after.presentation_clock.unwrap().expiry_tick, 1000);
        assert!((after.ttl - 8.3).abs() < 0.001);
        assert!(!game.apply_realtime_projectile_state(77, 88, 19, 150, held));
        assert_eq!(game.remote_projectiles[0], after);
    }

    #[test]
    fn capacity_paused_repel_preserves_flight_pause_through_reordered_carriers_and_recovery() {
        for before_carrier in [false, true] {
            let (mut game, source) = delayed_carrier_fixture(150);
            let mut held = realtime_projectile_state(source, 123, 120);
            held.source_server_tick = 100;
            held.motion_frozen = true;
            held.position_x = 1200.0;
            held.position_y = 1000.0;
            if !before_carrier {
                assert!(game.apply_realtime_projectile_state(77, 88, 15, 120, held));
            }
            let mutation = crate::realtime_event::RealtimeRepelMutationState {
                motion_frozen: true,
                entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
                source_player_id: source.source_player_id,
                source_session_id: source.source_session_id,
                source_life_seq: source.source_life_seq,
                source_server_tick: 100,
                source_weapon_data: source.weapon_data,
                source_projectile_index: 0,
                result_weapon_data: source.weapon_data,
                position_x: 1300.0,
                position_y: 1000.0,
                velocity_x: 50.0,
                velocity_y: 0.0,
                expires_at_tick: 1000,
                bounces_remaining: 0,
            };
            game.last_observed_browser_server_tick = Some(170);
            game.apply_realtime_repel_mutations(77, 88, 20, 160, repel_mutation_batch(&[mutation]));
            if before_carrier {
                game.apply_realtime_weapon_visual(77, 88, 12, 100, source);
            }
            assert_eq!(game.remote_projectiles.len(), 1);
            let after = game.remote_projectiles[0];
            assert!(after.presentation_clock.unwrap().motion_frozen);
            assert!(
                after.pos.distance(vec2(1300.0, 1000.0)) < 0.01,
                "a delayed Repel cannot replay flight while capacity still holds it"
            );
            assert_eq!(after.presentation_clock.unwrap().expiry_tick, 1000);
            assert!((after.ttl - 8.3).abs() < 0.001);
            game.last_observed_browser_server_tick = Some(180);
            game.update_remote_weapons_at_server_time();
            assert_eq!(game.remote_projectiles[0].pos, after.pos);
            assert!((game.remote_projectiles[0].ttl - 8.2).abs() < 0.001);
            let drawn = remote_projectile_presentation(
                &game.remote_projectiles[0],
                0.01,
                &game.maps[game.map_index],
                &game.bricks,
                &game.tuning,
            );
            assert_eq!(
                drawn.position, after.pos,
                "fractional frames remain held after Repel"
            );
            let mut resumed = held;
            resumed.motion_frozen = false;
            resumed.position_x = 1300.0;
            resumed.velocity_x = 50.0;
            resumed.velocity_y = 0.0;
            resumed.expiry_tick = Some(1000);
            game.last_observed_browser_server_tick = Some(185);
            assert!(game.apply_realtime_projectile_state(77, 88, 21, 181, resumed));
            assert!(
                !game.remote_projectiles[0]
                    .presentation_clock
                    .unwrap()
                    .motion_frozen
            );
            assert!(
                game.remote_projectiles[0]
                    .pos
                    .distance(vec2(1302.0, 1000.0))
                    < 0.01
            );
        }
    }
}
