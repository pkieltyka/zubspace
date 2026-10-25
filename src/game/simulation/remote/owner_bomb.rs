use super::*;

// At most22 canonical10ms steps including the frame remainder, converging within250ms.
// A scheduler pause or a larger discrepancy uses the unmodified server view.
const MAX_LEAD_SECONDS: f32 = 0.2;
const CONVERGENCE_SECONDS: f32 = 0.25;
const MAX_BRIDGES: usize = 64;
const TRAJECTORY_TOLERANCE_PX: f32 = 1.0;

impl OwnerBombDrawBridge {
    fn matches(&self, projectile: &RemoteProjectile) -> bool {
        crate::remote_weapon::RemoteWeaponViewKey::from_projectile(projectile) == Some(self.key)
            && projectile
                .authority_binding
                .is_some_and(|binding| binding.projectile_id == self.projectile_id)
    }

    fn lead(&self, projectile: &RemoteProjectile, partial: f32) -> Option<f32> {
        let clock = projectile.presentation_clock?;
        let ticks = crate::continuum::continuum_tick_diff(clock.pose_tick, self.start_tick);
        if ticks < 0
            || clock.motion_frozen
            || projectile.ttl <= 0.0
            || projectile.last_repel_mutation_event_id.is_some()
            || projectile.bounces_remaining != self.bounces_remaining
            || !matches!(projectile.kind, RemoteProjectileKind::Bomb { .. })
            || !projectile.pos.is_finite()
            || !projectile.vel.is_finite()
            || (projectile.vel - self.velocity).length() > 0.01
        {
            return None;
        }
        let elapsed = crate::continuum::ticks_to_seconds_i32(ticks);
        // A canonical correction, gravity, bounce or impulse is authoritative;
        // never carry this small straight-flight lead across that boundary.
        if projectile
            .pos
            .distance(self.start_position + self.velocity * elapsed)
            > TRAJECTORY_TOLERANCE_PX
        {
            return None;
        }
        let elapsed = elapsed + partial;
        if elapsed >= CONVERGENCE_SECONDS {
            return None;
        }
        let lead = self.initial_lead_seconds * (1.0 - elapsed / CONVERGENCE_SECONDS);
        (lead + partial < projectile.ttl).then_some(lead)
    }
}

impl Game {
    pub(in crate::game) fn begin_owner_bomb_draw_bridge(
        &mut self,
        prediction: Bomb,
        projectile: RemoteProjectile,
    ) {
        let scope = self.client_authority.view().scope;
        let Some(clock) = projectile.presentation_clock else {
            return;
        };
        let Some(key) = crate::remote_weapon::RemoteWeaponViewKey::from_projectile(&projectile)
        else {
            return;
        };
        let Some(binding) = projectile.authority_binding else {
            return;
        };
        let Some(prediction_identity) = prediction.prediction else {
            return;
        };
        if !self.networked_play
            || scope.alive != Some(true)
            || !owner_bomb_scope_matches(scope, &projectile)
            || prediction_identity.life_seq != scope.life_seq.unwrap_or_default()
            || prediction_identity.projectile_index != projectile.source_projectile_index
            || !matches!(projectile.kind, RemoteProjectileKind::Bomb { .. })
            || clock.motion_frozen
            || projectile.last_repel_mutation_event_id.is_some()
            || prediction.bounces_remaining != projectile.bounces_remaining
            || !prediction.pos.is_finite()
            || !projectile.pos.is_finite()
            || !prediction.vel.is_finite()
            || !projectile.vel.is_finite()
            || (prediction.vel - projectile.vel).length() > 0.01
            || !prediction.age.is_finite()
            || !projectile.age.is_finite()
        {
            return;
        }
        let lead = prediction.age - projectile.age;
        if !(0.0..=MAX_LEAD_SECONDS).contains(&lead)
            || lead <= 0.0
            || projectile.ttl <= lead
            || !projectile.ttl.is_finite()
            || prediction
                .pos
                .distance(projectile.pos + projectile.vel * lead)
                > TRAJECTORY_TOLERANCE_PX
        {
            return;
        }
        if self
            .owner_bomb_draw_bridges
            .iter()
            .any(|bridge| bridge.matches(&projectile))
        {
            return;
        }
        if self.owner_bomb_draw_bridges.len() == MAX_BRIDGES {
            self.owner_bomb_draw_bridges.pop_front();
        }
        self.owner_bomb_draw_bridges.push_back(OwnerBombDrawBridge {
            key,
            projectile_id: binding.projectile_id,
            start_tick: clock.pose_tick,
            start_position: projectile.pos,
            velocity: projectile.vel,
            bounces_remaining: projectile.bounces_remaining,
            initial_lead_seconds: lead,
        });
    }

    pub(in crate::game) fn prune_owner_bomb_draw_bridges(&mut self) {
        let scope = self.client_authority.view().scope;
        self.owner_bomb_draw_bridges.retain(|bridge| {
            self.remote_projectiles.iter().any(|projectile| {
                owner_bomb_scope_matches(scope, projectile)
                    && bridge.matches(projectile)
                    && bridge.lead(projectile, 0.0).is_some()
            })
        });
    }

    /// Every draw and its diagnostics use this same terrain-aware visual copy.
    /// Canonical pose/clock, collision, TTL and effects are never mutated.
    pub(in crate::game) fn remote_projectile_draw_presentation(
        &self,
        projectile: &RemoteProjectile,
        partial_seconds: f32,
    ) -> crate::gameplay::WeaponPresentation {
        let base = remote_projectile_presentation(
            projectile,
            partial_seconds,
            self.map(),
            &self.bricks,
            &self.tuning,
        );
        let scope = self.client_authority.view().scope;
        if !owner_bomb_scope_matches(scope, projectile) {
            return base;
        }
        let Some(clock) = projectile.presentation_clock else {
            return base;
        };
        let partial = clock.render_partial_seconds;
        if !partial.is_finite() || !(0.0..=GAMEPLAY_STEP_SECONDS).contains(&partial) {
            return base;
        }
        let Some(lead) = self
            .owner_bomb_draw_bridges
            .iter()
            .find(|bridge| bridge.matches(projectile))
            .and_then(|bridge| bridge.lead(projectile, partial))
        else {
            return base;
        };
        let mut view = *projectile;
        let elapsed = lead + partial;
        let steps = (elapsed / crate::gameplay_rules::weapon_motion::STEP_SECONDS).ceil() as u32;
        for index in 0..steps {
            let dt = (elapsed - index as f32 * crate::gameplay_rules::weapon_motion::STEP_SECONDS)
                .clamp(0.0, crate::gameplay_rules::weapon_motion::STEP_SECONDS);
            view.age += dt;
            view.ttl -= dt;
            let (event, bounce) = update_remote_bomb_motion(
                &mut view,
                dt,
                self.map(),
                &self.bricks,
                Vec2::ZERO,
                0.0,
                self.tuning,
                projectile.source.bomb_gravity,
                false,
            );
            if event.is_some()
                || bounce.is_some()
                || view.bounces_remaining != projectile.bounces_remaining
                || view.vel.distance(projectile.vel) > 0.01
            {
                return base;
            }
        }
        crate::gameplay::WeaponPresentation {
            position: view.pos,
            animation_seconds: projectile.age + elapsed,
            bounces_remaining: view.bounces_remaining,
            burst_active: false,
            owner_prediction_lead_seconds: lead,
        }
    }
}

fn owner_bomb_scope_matches(scope: AuthorityScopeIdentity, projectile: &RemoteProjectile) -> bool {
    scope.alive == Some(true)
        && scope.player_id == Some(projectile.source.identity.player_id())
        && projectile.source.identity.matches_authenticated_runtime(
            scope.player_id.unwrap_or(INVALID_PLAYER_ID),
            match projectile.source.identity {
                RemoteWeaponSourceIdentity::Authenticated { session_id, .. } => Some(session_id),
                _ => None,
            },
            scope.life_seq,
        )
        && projectile.source.arena_scope.is_some_and(|arena| {
            scope.server_session_id == Some(arena.server_session_id)
                && scope.arena_stream_id == Some(arena.arena_stream_id)
        })
}
