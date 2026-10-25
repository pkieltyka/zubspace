use super::{
    MotionHistorySource, PlayerHullSnapshot, ProjectileCollisionWitness, ProjectileContactReason,
    ProjectileHit, ProjectileRecord, RepelMutationSummary, RepelledProjectileState,
};
use crate::continuum_weapon::{
    continuum_proximity_sensor_overlaps_ship, continuum_weapon_contact_overlaps_ship,
    continuum_weapon_explosion_level, continuum_weapon_is_mine, continuum_weapon_level,
    continuum_weapon_proximity_trigger_radius, continuum_weapon_type,
};

/// One owner can never pin more collision transactions than its complete
/// admitted projectile allowance. The global cap matches one four-fact burst
/// for every supported 768-player arena and keeps retry work/memory explicit.
pub(crate) const MAX_PENDING_HITS_PER_OWNER: usize = 128;
pub(crate) const MAX_PENDING_HITS_TOTAL: usize = 3_072;
pub(crate) const MAX_PENDING_HIT_RETRIES_PER_TICK: usize = 256;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ProjectileRegistry {
    records: Vec<ProjectileRecord>,
    contact_diagnostics: Option<super::contact_diagnostics::ContactDiagnostics>,
    /// Collisions whose damage/event transaction has not committed yet.
    /// Their complete shot groups remain frozen in `records`; every tick
    /// returns the immutable hit again until the caller explicitly resolves
    /// it. This prevents event-queue pressure from despawning authority.
    pending_hits: Vec<PendingProjectileHit>,
    /// Free-flight records whose most recent physics step stopped at an
    /// authority-work capacity barrier. This is a presentation cue, never
    /// permission to retain a collision or extend lifetime/cancellation.
    capacity_paused: std::collections::HashSet<u32>,
    pending_wall_explosions: std::collections::VecDeque<ProjectileWallExplosion>,
    /// New immutable collisions first frozen before their actual human victim's
    /// bounded source-life threat window opened. Retries do not increment it.
    harmful_readiness_deferrals_total: u64,
    /// Bounded production diagnostics for the server-owned observer
    /// presentation projection that owns human-projectile collision geometry.
    presentation_geometry_lookups_total: u64,
    presentation_geometry_resolved_total: u64,
    presentation_geometry_unavailable_total: u64,
    presentation_geometry_server_native_total: u64,
    presentation_geometry_local_self_total: u64,
    presentation_geometry_server_advanced_total: u64,
    presentation_geometry_eval_nanos_total: u64,
    presentation_geometry_eval_nanos_max: u64,
    /// Phase 2b: Continuum tick at which `step` was last called.
    /// Used to compute dt per step. None until the first step runs.
    last_step_tick: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingProjectileHit {
    hit: ProjectileHit,
    shot_group_id: u64,
    consume_group: bool,
    witness: ProjectileCollisionWitness,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectileWallExplosion {
    pub(crate) parent: ProjectileRecord,
    pub(crate) collision_tick: u32,
}

/// Hard payload budget for one immutable collision plus its structured
/// diagnostic witness. The compile-time assertion makes adding fields an
/// explicit capacity decision instead of silently invalidating the arena's
/// retained-memory bound.
/// The compact observer-projection witness is retained beside the collision.
/// At the global 3,072-hit cap this explicit 256-byte ceiling bounds the
/// complete pending collision store to 768 KiB.
pub(crate) const MAX_PENDING_HIT_RECORD_BYTES: usize = 256;
const _: () = assert!(std::mem::size_of::<PendingProjectileHit>() <= MAX_PENDING_HIT_RECORD_BYTES);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CollisionTargetWitness {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) requested_tick: u32,
    pub(crate) selected_tick: Option<u32>,
    pub(crate) source: MotionHistorySource,
    pub(crate) collision_available: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ExactMotionSample {
    x: f32,
    y: f32,
    selected_tick: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PresentationGeometryMetrics {
    pub(crate) lookups_total: u64,
    pub(crate) resolved_total: u64,
    pub(crate) unavailable_total: u64,
    pub(crate) server_native_total: u64,
    pub(crate) local_self_total: u64,
    pub(crate) server_advanced_total: u64,
    pub(crate) eval_nanos_total: u64,
    pub(crate) eval_nanos_max: u64,
}

impl PresentationGeometryMetrics {
    fn observe_result(&mut self, witness: crate::combat_timeline::ProjectilePresentationWitnessV1) {
        use crate::combat_timeline::ProjectilePresentationStatusV1;
        match witness.status {
            ProjectilePresentationStatusV1::ServerNative => {
                self.server_native_total = self.server_native_total.saturating_add(1);
                return;
            }
            ProjectilePresentationStatusV1::LocalSelf => {
                self.lookups_total = self.lookups_total.saturating_add(1);
                self.local_self_total = self.local_self_total.saturating_add(1);
                self.resolved_total = self.resolved_total.saturating_add(1);
            }
            ProjectilePresentationStatusV1::Resolved(_) => {
                self.lookups_total = self.lookups_total.saturating_add(1);
                self.resolved_total = self.resolved_total.saturating_add(1);
            }
            ProjectilePresentationStatusV1::NotEvaluated
            | ProjectilePresentationStatusV1::QueryUnavailable(_)
            | ProjectilePresentationStatusV1::ProjectionUnavailable(_) => {
                self.lookups_total = self.lookups_total.saturating_add(1);
                self.unavailable_total = self.unavailable_total.saturating_add(1);
            }
        }
        if witness.server_advanced_watermark {
            self.server_advanced_total = self.server_advanced_total.saturating_add(1);
        }
    }
}

impl ProjectileRegistry {
    pub(crate) fn set_contact_diagnostics_enabled(&mut self, enabled: bool) {
        if enabled {
            self.contact_diagnostics
                .get_or_insert_with(Default::default);
        } else {
            self.contact_diagnostics = None;
        }
    }

    pub(crate) fn drain_contact_diagnostics(
        &mut self,
    ) -> Option<(Vec<super::ContactDiagnostic>, u64)> {
        self.contact_diagnostics.as_mut()?.drain()
    }

    pub(crate) fn collision_witness(
        &self,
        projectile_id: u32,
    ) -> Option<ProjectileCollisionWitness> {
        self.pending_hits
            .iter()
            .find(|pending| pending.hit.projectile_id == projectile_id)
            .map(|pending| pending.witness)
    }

    pub(crate) fn projectile(&self, projectile_id: u32) -> Option<&ProjectileRecord> {
        self.records
            .iter()
            .find(|projectile| projectile.id == projectile_id)
    }

    pub(crate) fn harmful_readiness_deferrals_total(&self) -> u64 {
        self.harmful_readiness_deferrals_total
    }

    pub(crate) fn presentation_geometry_metrics(&self) -> PresentationGeometryMetrics {
        PresentationGeometryMetrics {
            lookups_total: self.presentation_geometry_lookups_total,
            resolved_total: self.presentation_geometry_resolved_total,
            unavailable_total: self.presentation_geometry_unavailable_total,
            server_native_total: self.presentation_geometry_server_native_total,
            local_self_total: self.presentation_geometry_local_self_total,
            server_advanced_total: self.presentation_geometry_server_advanced_total,
            eval_nanos_total: self.presentation_geometry_eval_nanos_total,
            eval_nanos_max: self.presentation_geometry_eval_nanos_max,
        }
    }

    /// Return one bounded FIFO prefix for immutable damage-transaction
    /// preparation. This deliberately includes collisions whose harmful
    /// readiness is still closed: geometry, splash membership, and damage
    /// anchors must freeze at collision time, while application waits.
    pub(crate) fn pending_hits_for_preparation(&self) -> Vec<ProjectileHit> {
        self.pending_hits
            .iter()
            .take(MAX_PENDING_HIT_RETRIES_PER_TICK)
            .map(|pending| pending.hit)
            .collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ProjectileRecord> {
        self.records.iter()
    }

    pub(crate) fn last(&self) -> Option<&ProjectileRecord> {
        self.records.last()
    }

    pub(crate) fn clear(&mut self) {
        self.contact_diagnostics = None;
        self.records.clear();
        self.capacity_paused = Default::default();
        self.pending_hits = Vec::new();
        self.pending_wall_explosions.clear();
    }

    pub(crate) fn pending_wall_explosions(&self) -> impl Iterator<Item = &ProjectileWallExplosion> {
        self.pending_wall_explosions.iter()
    }

    pub(crate) fn resolve_wall_explosion(&mut self, projectile_id: u32) {
        self.pending_wall_explosions
            .retain(|explosion| explosion.parent.id != projectile_id);
    }

    pub(crate) fn record(
        &mut self,
        projectile: ProjectileRecord,
        max_per_owner: usize,
        max_total: usize,
    ) {
        self.capacity_paused.remove(&projectile.id);
        self.records.push(projectile);
        self.trim_owner(projectile.owner_id, max_per_owner);
        self.trim_total(max_total);
    }

    /// Inserts one accepted packet atomically so capacity cleanup can never
    /// retain a partial double-barrel/multifire salvo.
    pub(crate) fn record_group(
        &mut self,
        projectiles: Vec<ProjectileRecord>,
        max_per_owner: usize,
        max_total: usize,
    ) {
        let Some(first) = projectiles.first() else {
            return;
        };
        let owner_id = first.owner_id;
        let shot_group_id = first.shot_group_id;
        debug_assert!(projectiles.iter().all(|projectile| {
            projectile.owner_id == owner_id && projectile.shot_group_id == shot_group_id
        }));
        for projectile in &projectiles {
            self.capacity_paused.remove(&projectile.id);
        }
        self.records.extend(projectiles);
        self.trim_owner_groups(owner_id, max_per_owner);
        self.trim_total_groups(max_total);
    }

    /// An explosion fan must fit beside every already frozen transaction.
    /// Ordinary uncommitted groups may still be evicted by record_group, but
    /// pressure may never turn a committed fan into a partial/absent fan.
    pub(crate) fn can_record_group_addition(
        &self,
        owner_id: u16,
        count: usize,
        max_per_owner: usize,
        max_total: usize,
    ) -> bool {
        if count == 0 {
            return true;
        }
        let protected = self.pending_group_ids();
        let mut owner_pinned = 0usize;
        let mut total_pinned = 0usize;
        for record in &self.records {
            if protected.contains(&record.shot_group_id) {
                total_pinned += 1;
                owner_pinned += usize::from(record.owner_id == owner_id);
            }
        }
        owner_pinned.saturating_add(count) <= max_per_owner
            && total_pinned.saturating_add(count) <= max_total
    }

    /// Safety cancels free flight, never an already-frozen contact. Keep
    /// linked groups together so a prior contact retains its immutable outcome.
    pub(crate) fn cancel_unfrozen_where(
        &mut self,
        mut cancel: impl FnMut(&ProjectileRecord) -> bool,
    ) -> usize {
        let protected = self.pending_group_ids();
        let before = self.records.len();
        self.records
            .retain(|record| protected.contains(&record.shot_group_id) || !cancel(record));
        before - self.records.len()
    }

    pub(crate) fn remove_owner(&mut self, owner_id: u16) {
        self.pending_wall_explosions
            .retain(|explosion| explosion.parent.owner_id != owner_id);
        self.records
            .retain(|projectile| projectile.owner_id != owner_id);
        self.pending_hits.retain(|pending| {
            pending.hit.owner_id != owner_id && pending.hit.victim_id != owner_id
        });
        if self.pending_hits.is_empty() {
            self.pending_hits = Vec::new();
        }
    }

    pub(crate) fn remove_owner_life(&mut self, owner: crate::authority_identity::LifeIdentity) {
        self.pending_wall_explosions
            .retain(|explosion| explosion.parent.owner_identity() != owner);
        self.records
            .retain(|projectile| projectile.owner_identity() != owner);
        self.pending_hits.retain(|pending| {
            pending.hit.owner_identity() != owner && pending.hit.victim_identity() != owner
        });
        if self.pending_hits.is_empty() {
            self.pending_hits = Vec::new();
        }
    }

    /// Retire an actor's live projectile authority after a server-authored
    /// damage death while preserving only collisions that were already
    /// frozen in that exact simulation tick.
    ///
    /// This is deliberately narrower than `remove_owner`: an unresolved
    /// projectile cannot keep flying after death, but a collision the server
    /// has already detected is a completed authority fact. Capacity pressure
    /// may delay that fact's delivery transaction without changing its
    /// shooter life. Incoming collisions against the retired victim are
    /// resolved first so they cannot target the replacement life.
    pub(crate) fn remove_owner_preserving_same_tick_hits(
        &mut self,
        owner: crate::authority_identity::LifeIdentity,
        collision_tick: u32,
    ) {
        let incoming = self
            .pending_hits
            .iter()
            .filter_map(|pending| {
                (pending.hit.victim_identity() == owner).then_some(pending.hit.projectile_id)
            })
            .collect::<Vec<_>>();
        for projectile_id in incoming {
            self.resolve_hit(projectile_id);
        }

        self.pending_hits.retain(|pending| {
            pending.hit.owner_id != owner.player_id
                || (pending.hit.owner_identity() == owner && pending.hit.tick == collision_tick)
        });
        let protected_groups = self.pending_group_ids();
        self.records.retain(|projectile| {
            projectile.owner_id != owner.player_id
                || protected_groups.contains(&projectile.shot_group_id)
        });
        if self.pending_hits.is_empty() {
            self.pending_hits = Vec::new();
        }
    }

    /// Resolve collisions that already targeted a dying actor **without** retiring the shots that
    /// actor fired.
    ///
    /// The reference removes a projectile by its own alive time or by its own explosion, never
    /// because its shooter died (`WeaponManager` computes
    /// `end_tick = local_timestamp + GetWeaponTotalAliveTime(type, alternate)` and nothing in that
    /// removal path consults the shooter's state), so a destroyed player's live weapons keep
    /// flying. Incoming collisions against the dying victim are still resolved first, because a
    /// hit that lands on the retired life must not be re-targeted at the replacement life.
    pub(crate) fn resolve_incoming_hits_of_life(
        &mut self,
        owner: crate::authority_identity::LifeIdentity,
    ) {
        let incoming = self
            .pending_hits
            .iter()
            .filter_map(|pending| {
                (pending.hit.victim_identity() == owner).then_some(pending.hit.projectile_id)
            })
            .collect::<Vec<_>>();
        for projectile_id in incoming {
            self.resolve_hit(projectile_id);
        }
    }

    /// True while the server still holds any live record fired by this life.
    pub(crate) fn contains_flight_of_life(
        &self,
        owner: crate::authority_identity::LifeIdentity,
    ) -> bool {
        self.records
            .iter()
            .any(|record| record.owner_identity() == owner)
            || self
                .pending_wall_explosions
                .iter()
                .any(|explosion| explosion.parent.owner_identity() == owner)
    }

    /// The life that owns the live record for `projectile_id`, if the server still has one.
    ///
    /// Attribution of a retired shooter's hit has to be pinned to a record the server actually
    /// authored. Widening the retired-shooter window without this would let any hit that merely
    /// names a retired life be accepted for as long as that life is remembered; a projectile id
    /// still present in the registry, owned by that same life, is the fact that makes the
    /// acceptance mean something.
    pub(crate) fn flight_owner_of(
        &self,
        projectile_id: u32,
    ) -> Option<crate::authority_identity::LifeIdentity> {
        self.records
            .iter()
            .find(|record| record.id == projectile_id)
            .map(|record| record.owner_identity())
    }

    /// Apply one accepted hostile Repel to live projectile authority. The
    /// same square-range/normalized-velocity helper drives browser feel and
    /// server outcomes. Frozen collision transactions are immutable and are
    /// therefore excluded from later Repel mutation.
    pub(crate) fn apply_repel(
        &mut self,
        origin: crate::gameplay_rules::RulesVector,
        source_frequency: u16,
        speed: f32,
        radius: f32,
        server_tick: u32,
        owner_state: &std::collections::HashMap<crate::authority_identity::LifeIdentity, (u16, u8)>,
        lifetime_for: impl Fn(u16) -> Option<u32>,
    ) -> RepelMutationSummary {
        let pending_groups = self.pending_group_ids();
        let mut summary = RepelMutationSummary::default();
        for projectile in &mut self.records {
            if pending_groups.contains(&projectile.shot_group_id) {
                continue;
            }
            let Some((frequency, bomb_bounces, source_session_id)) =
                projectile.repel_owner_state(owner_state)
            else {
                continue;
            };
            if frequency == source_frequency {
                continue;
            }
            let Some(velocity) = crate::gameplay_rules::repel_velocity_after(
                origin,
                crate::gameplay_rules::RulesVector::new(projectile.pos_x, projectile.pos_y),
                speed,
                radius,
            ) else {
                continue;
            };
            let source_weapon_data = projectile.weapon_data;
            projectile.precise_velocity = Some(velocity);
            projectile.velocity_x = (velocity.x * 10.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            projectile.velocity_y = (velocity.y * 10.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            if continuum_weapon_is_mine(projectile.weapon_data) {
                projectile.weapon_data &= !0x8000;
                projectile.bounces_remaining = bomb_bounces;
                projectile.proximity_target = None;
                projectile.proximity_sensor_end_tick = None;
                projectile.proximity_highest_offset = 0.0;
                summary.mines_converted = summary.mines_converted.saturating_add(1);
            }
            projectile.expiry_tick = lifetime_for(projectile.weapon_data)
                .map(|lifetime| crate::continuum::continuum_tick_add(server_tick, lifetime));
            if continuum_weapon_type(projectile.weapon_data) == 15 {
                // The reference game derives fragment age from its refreshed end_tick.
                // Keep this age anchor aligned with that new alive interval;
                // timestamp still preserves the original life-admission gate.
                projectile.created_tick = server_tick;
            }
            let Some(expires_at_tick) = projectile.expiry_tick else {
                continue;
            };
            summary.states.push(RepelledProjectileState {
                motion_frozen: self.capacity_paused.contains(&projectile.id),
                projectile_id: projectile.id,
                source_session_id,
                owner_id: projectile.owner_id,
                owner_life_seq: projectile.owner_life_seq,
                created_tick: projectile
                    .source_server_tick
                    .unwrap_or(projectile.created_tick),
                source_weapon_data,
                source_projectile_index: projectile.shot_index,
                result_weapon_data: projectile.weapon_data,
                position_x: projectile.pos_x,
                position_y: projectile.pos_y,
                velocity_x: projectile.motion_velocity().x,
                velocity_y: projectile.motion_velocity().y,
                expires_at_tick,
                bounces_remaining: projectile.bounces_remaining,
            });
            summary.projectiles_repelled = summary.projectiles_repelled.saturating_add(1);
        }
        summary
    }

    pub(crate) fn preview_repel_mutation_count(
        &self,
        origin: crate::gameplay_rules::RulesVector,
        source_frequency: u16,
        radius: f32,
        owner_state: &std::collections::HashMap<crate::authority_identity::LifeIdentity, (u16, u8)>,
    ) -> usize {
        let pending_groups = self.pending_group_ids();
        self.records
            .iter()
            .filter(|projectile| {
                !pending_groups.contains(&projectile.shot_group_id)
                    && projectile
                        .repel_owner_state(owner_state)
                        .is_some_and(|(frequency, _, _)| frequency != source_frequency)
                    && crate::gameplay_rules::repel_velocity_after(
                        origin,
                        crate::gameplay_rules::RulesVector::new(projectile.pos_x, projectile.pos_y),
                        1.0,
                        radius,
                    )
                    .is_some()
            })
            .count()
    }

    pub(crate) fn contains_pending_hit(&self, projectile_id: u32) -> bool {
        self.pending_hits
            .iter()
            .any(|pending| pending.hit.projectile_id == projectile_id)
    }

    pub(crate) fn contains_pending_hit_for_owner_at_tick(
        &self,
        owner: crate::authority_identity::LifeIdentity,
        collision_tick: u32,
    ) -> bool {
        self.pending_hits.iter().any(|pending| {
            pending.hit.owner_identity() == owner && pending.hit.tick == collision_tick
        })
    }

    pub(crate) fn expire_older_than(&mut self, tick: u32, max_age_ticks: u32) {
        let pending_groups = self.pending_group_ids();
        self.records.retain(|projectile| {
            pending_groups.contains(&projectile.shot_group_id)
                || crate::continuum::continuum_tick_elapsed(tick, projectile.created_tick)
                    <= max_age_ticks
        });
    }

    /// Resolve one previously returned collision after the damage/event
    /// transaction either committed or proved permanently invalid. Capacity
    /// pressure deliberately does not call this method.
    pub(crate) fn resolve_hit(&mut self, projectile_id: u32) -> bool {
        let Some(index) = self
            .pending_hits
            .iter()
            .position(|pending| pending.hit.projectile_id == projectile_id)
        else {
            return false;
        };
        let pending = self.pending_hits.remove(index);
        if pending.consume_group {
            self.records
                .retain(|projectile| projectile.shot_group_id != pending.shot_group_id);
            self.pending_hits
                .retain(|other| other.shot_group_id != pending.shot_group_id);
        } else {
            self.records
                .retain(|projectile| projectile.id != projectile_id);
        }
        if self.pending_hits.is_empty() {
            self.pending_hits = Vec::new();
        }
        true
    }

    pub(crate) fn pending_hit_count(&self) -> usize {
        self.pending_hits.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_hit_count_for_owner(&self, owner_id: u16) -> usize {
        self.pending_hits
            .iter()
            .filter(|pending| pending.hit.owner_id == owner_id)
            .count()
    }

    pub(crate) fn pending_hit_retained_bytes(&self) -> usize {
        self.pending_hits
            .capacity()
            .saturating_mul(std::mem::size_of::<PendingProjectileHit>())
    }

    fn pending_group_ids(&self) -> std::collections::HashSet<u64> {
        self.pending_hits
            .iter()
            .map(|pending| pending.shot_group_id)
            .collect()
    }

    /// Include pending collision groups and records stopped at work capacity.
    /// Neither kind of paused flight may be extrapolated by the client.
    pub(crate) fn frozen_projectile_ids(&self) -> std::collections::HashSet<u32> {
        let groups = self.pending_group_ids();
        self.records
            .iter()
            .filter(|p| groups.contains(&p.shot_group_id) || self.capacity_paused.contains(&p.id))
            .map(|p| p.id)
            .collect()
    }

    /// Phase 2b/2c: advance every projectile by one server-sim step.
    ///
    /// Runs once per server tick from `run_server_tick`. dt is computed
    /// from the previous call's `current_tick`; on the first call the
    /// step is a no-op (just records the anchor) so a projectile
    /// freshly added in this tick doesn't get integrated for unknown
    /// elapsed time. Mines (zero velocity) are stepped along with
    /// everything else but their position doesn't move.
    ///
    /// **Collision** (Phase 2c + polish): when `is_solid_tile_at`
    /// returns true for the projectile's new position, the projectile
    /// either bounces (decrementing `bounces_remaining`) or despawns
    /// (`bounces_remaining == 0`). Bounce reflects the velocity
    /// component on the colliding axis. The server caller supplies a
    /// door-aware solid predicate, so open doors are passable and
    /// closed or unknown door states block.
    ///
    /// Despawns projectiles whose `expiry_tick` has been reached.
    /// Player-hull collision is Phase 2d.
    pub(crate) fn step(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
    ) -> Vec<ProjectileHit> {
        self.step_with_proximity_distance(current_tick, is_solid_tile_at, players, 3.0, 0)
    }

    pub(crate) fn step_with_proximity_distance(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
    ) -> Vec<ProjectileHit> {
        let hits = self.step_with_proximity_distance_transactional(
            current_tick,
            is_solid_tile_at,
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
        );
        // Legacy/unit callers consume collision results synchronously. The
        // production tick uses the transactional variant below and resolves
        // only after damage/event capacity commits.
        for hit in &hits {
            self.resolve_hit(hit.projectile_id);
        }
        hits
    }

    pub(crate) fn step_with_proximity_distance_transactional(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
    ) -> Vec<ProjectileHit> {
        self.step_with_damage_geometry_transactional(
            current_tick,
            is_solid_tile_at,
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
            0.0,
        )
    }

    /// Production explosive geometry. `bomb_explode_pixels` is the authored
    /// level-one radial-damage radius; it is deliberately supplied by the
    /// active arena rather than embedded in the shared projectile registry.
    pub(crate) fn step_with_damage_geometry_transactional(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
        bomb_explode_pixels: f32,
    ) -> Vec<ProjectileHit> {
        self.step_with_damage_geometry_and_harmful_eligibility_transactional(
            current_tick,
            is_solid_tile_at,
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
            bomb_explode_pixels,
            |_| true,
        )
    }

    /// Production collision step with one global, exact-victim harmful gate.
    /// An early collision is frozen in the existing immutable pending-hit FIFO
    /// and re-evaluated on later ticks; it neither passes through the victim nor
    /// forks projectile physics per recipient.
    pub(crate) fn step_with_damage_geometry_and_harmful_eligibility_transactional(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
        bomb_explode_pixels: f32,
        harmful_eligibility: impl Fn(&ProjectileHit) -> bool,
    ) -> Vec<ProjectileHit> {
        self.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional_inner(
            current_tick,
            |_, _, _| crate::gameplay_rules::RulesVector::new(0.0, 0.0),
            |_, _| false,
            |_, x, y| is_solid_tile_at(x, y),
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
            bomb_explode_pixels,
            |_, _, _| crate::combat_timeline::ProjectilePresentationWitnessV1::not_evaluated(),
            harmful_eligibility,
        )
    }

    /// Production collision path. Human projectiles resolve every target
    /// against the firing observer's immutable verified presentation
    /// timeline. Server-native projectiles use exact authoritative motion.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_with_combat_timeline_geometry_and_harmful_eligibility_transactional(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
        bomb_explode_pixels: f32,
        presentation_geometry: impl FnMut(
            &ProjectileRecord,
            &PlayerHullSnapshot,
            u32,
        )
            -> crate::combat_timeline::ProjectilePresentationWitnessV1,
        harmful_eligibility: impl Fn(&ProjectileHit) -> bool,
    ) -> Vec<ProjectileHit> {
        self.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional_inner(
            current_tick,
            |_, _, _| crate::gameplay_rules::RulesVector::new(0.0, 0.0),
            |_, _| false,
            |_, x, y| is_solid_tile_at(x, y),
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
            bomb_explode_pixels,
            presentation_geometry,
            harmful_eligibility,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_with_frequency_aware_solidity_transactional(
        &mut self,
        current_tick: u32,
        is_solid_tile_at: impl Fn(u16, f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
        bomb_explode_pixels: f32,
        presentation_geometry: impl FnMut(
            &ProjectileRecord,
            &PlayerHullSnapshot,
            u32,
        )
            -> crate::combat_timeline::ProjectilePresentationWitnessV1,
        harmful_eligibility: impl Fn(&ProjectileHit) -> bool,
    ) -> Vec<ProjectileHit> {
        self.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional_inner(
            current_tick,
            |_, _, _| crate::gameplay_rules::RulesVector::new(0.0, 0.0),
            |_, _| false,
            is_solid_tile_at,
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
            bomb_explode_pixels,
            presentation_geometry,
            harmful_eligibility,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step_with_environment_transactional(
        &mut self,
        current_tick: u32,
        gravity_delta: impl Fn(
            &ProjectileRecord,
            crate::gameplay_rules::RulesVector,
            f32,
        ) -> crate::gameplay_rules::RulesVector,
        is_wormhole: impl Fn(f32, f32) -> bool,
        is_solid_tile_at: impl Fn(u16, f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
        bomb_explode_pixels: f32,
        presentation_geometry: impl FnMut(
            &ProjectileRecord,
            &PlayerHullSnapshot,
            u32,
        )
            -> crate::combat_timeline::ProjectilePresentationWitnessV1,
        harmful_eligibility: impl Fn(&ProjectileHit) -> bool,
    ) -> Vec<ProjectileHit> {
        self.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional_inner(
            current_tick,
            gravity_delta,
            is_wormhole,
            is_solid_tile_at,
            players,
            proximity_distance,
            bomb_explode_delay_ticks,
            bomb_explode_pixels,
            presentation_geometry,
            harmful_eligibility,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn step_with_combat_timeline_geometry_and_harmful_eligibility_transactional_inner(
        &mut self,
        current_tick: u32,
        gravity_delta: impl Fn(
            &ProjectileRecord,
            crate::gameplay_rules::RulesVector,
            f32,
        ) -> crate::gameplay_rules::RulesVector,
        is_wormhole: impl Fn(f32, f32) -> bool,
        is_solid_tile_at: impl Fn(u16, f32, f32) -> bool,
        players: &[PlayerHullSnapshot],
        proximity_distance: f32,
        bomb_explode_delay_ticks: u32,
        bomb_explode_pixels: f32,
        mut presentation_geometry: impl FnMut(
            &ProjectileRecord,
            &PlayerHullSnapshot,
            u32,
        )
            -> crate::combat_timeline::ProjectilePresentationWitnessV1,
        harmful_eligibility: impl Fn(&ProjectileHit) -> bool,
    ) -> Vec<ProjectileHit> {
        if let Some(diagnostics) = &mut self.contact_diagnostics {
            diagnostics.begin_step(current_tick);
        }
        let mut presentation_geometry_metrics = PresentationGeometryMetrics::default();
        let pending_groups = self.pending_group_ids();
        // Expiry is a hard upper bound: remove records before movement or
        // collision so a projectile cannot deal damage on/after its expiry
        // tick. `Option` is essential because zero is a valid wrapped
        // deadline.
        self.records.retain(|projectile| {
            pending_groups.contains(&projectile.shot_group_id)
                || projectile.expiry_tick.is_none_or(|expiry_tick| {
                    !crate::continuum::continuum_tick_reached(current_tick, expiry_tick)
                })
        });
        let last = match self.last_step_tick.replace(current_tick) {
            Some(prev) => prev,
            None => {
                return self
                    .pending_hits
                    .iter()
                    .take(MAX_PENDING_HIT_RETRIES_PER_TICK)
                    .take_while(|pending| harmful_eligibility(&pending.hit))
                    .map(|pending| pending.hit)
                    .collect();
            }
        };
        let dt_ticks = crate::continuum::continuum_tick_elapsed(current_tick, last);
        if dt_ticks == 0 {
            return self
                .pending_hits
                .iter()
                .take(MAX_PENDING_HIT_RETRIES_PER_TICK)
                .take_while(|pending| harmful_eligibility(&pending.hit))
                .map(|pending| pending.hit)
                .collect();
        }
        // Keep a prior cue through zero-time queries. Only a real new physics
        // step can decide which capacity pauses have resumed.
        self.capacity_paused.clear();
        // One server tick may prepare/retry at most 256 immutable damage
        // transactions. The global pending vector is one authority FIFO: its
        // oldest prefix consumes the work budget first. A new collision can
        // enter this batch only when every older pending hit is included, and
        // is appended behind all of them. Per-owner admission caps may delay a
        // new collision but can never let it overtake older authority.
        let retry_window = self
            .pending_hits
            .len()
            .min(MAX_PENDING_HIT_RETRIES_PER_TICK);
        let retry_limit = self
            .pending_hits
            .iter()
            .take(retry_window)
            .take_while(|pending| harmful_eligibility(&pending.hit))
            .count();
        let mut hits = self
            .pending_hits
            .iter()
            .take(retry_limit)
            .map(|pending| pending.hit)
            .collect::<Vec<_>>();
        let mut harmful_barrier = retry_limit < self.pending_hits.len();
        let new_hit_budget = if !harmful_barrier && retry_limit == self.pending_hits.len() {
            MAX_PENDING_HIT_RETRIES_PER_TICK.saturating_sub(retry_limit)
        } else {
            0
        };
        let mut new_pending_hits = Vec::new();
        let mut new_wall_explosions = Vec::new();
        let mut wall_explosions_by_owner = self.pending_wall_explosions.iter().fold(
            std::collections::HashMap::<_, usize>::new(),
            |mut counts, explosion| {
                *counts.entry(explosion.parent.owner_identity()).or_default() += 1;
                counts
            },
        );
        let mut new_harmful_readiness_deferrals = 0_u64;
        let mut pending_total = self.pending_hits.len();
        let mut pending_by_owner = self.pending_hits.iter().fold(
            std::collections::HashMap::<_, usize>::new(),
            |mut counts, pending| {
                *counts.entry(pending.hit.owner_identity()).or_default() += 1;
                counts
            },
        );
        // Mark projectiles that despawned mid-step (hit a wall with no
        // bounces left) so they get removed by the retain pass below.
        let mut despawned: Vec<usize> = Vec::new();
        let mut consumed_gun_groups = std::collections::HashSet::new();
        // P2: build the owner lookup table once per step rather than
        // rescanning `players` twice per projectile (for `owner_freq`
        // and for the owner-safety latch). At 200 players × 100 active
        // projectiles a linear scan is 40K extra iterations per step;
        // a single pre-pass turns the per-projectile work into O(1).
        let player_by_identity: std::collections::HashMap<_, &PlayerHullSnapshot> = players
            .iter()
            .map(|player| (player.identity(), player))
            .collect();
        for (index, projectile) in self.records.iter_mut().enumerate() {
            if pending_groups.contains(&projectile.shot_group_id) {
                continue;
            }
            if consumed_gun_groups.contains(&projectile.shot_group_id) {
                despawned.push(index);
                continue;
            }
            if new_pending_hits.len() >= new_hit_budget {
                // Stepping could discover a collision which cannot be frozen
                // and prepared within this tick's bounded work batch. Preserve
                // the complete projectile state and resume next tick.
                self.capacity_paused.insert(projectile.id);
                continue;
            }
            if pending_total >= MAX_PENDING_HITS_TOTAL
                || pending_by_owner
                    .get(&projectile.owner_identity())
                    .copied()
                    .unwrap_or(0)
                    >= MAX_PENDING_HITS_PER_OWNER
            {
                // Capacity is authority state, not a reason to advance and
                // lose a collision. Freeze this projectile until an older
                // pending transaction commits or lifecycle cleanup frees a
                // slot.
                self.capacity_paused.insert(projectile.id);
                continue;
            }
            let weapon_type = continuum_weapon_type(projectile.weapon_data);
            let captured_source = projectile.presentation_source.filter(|source| {
                source.source_player_id == projectile.owner_id
                    && source.source_life_seq == projectile.owner_life_seq
                    && source.source_session_id != 0
                    && source.shot_group_id == projectile.shot_group_id
            });
            let projectile_frequency = captured_source
                .map(|source| source.frequency)
                .or_else(|| {
                    player_by_identity
                        .get(&projectile.owner_identity())
                        .map(|owner| owner.frequency)
                })
                .unwrap_or(u16::MAX);
            let is_solid_for_projectile = |x, y| is_solid_tile_at(projectile_frequency, x, y);
            if matches!(weapon_type, 3 | 4)
                && projectile.presentation_source.is_some()
                && (self.pending_wall_explosions.len() + new_wall_explosions.len()
                    >= MAX_PENDING_HITS_TOTAL
                    || new_wall_explosions.len() >= MAX_PENDING_HIT_RETRIES_PER_TICK
                    || wall_explosions_by_owner
                        .get(&projectile.owner_identity())
                        .copied()
                        .unwrap_or(0)
                        >= MAX_PENDING_HITS_PER_OWNER)
            {
                // Preserve the parent before stepping when a newly discovered
                // victim-free wall explosion could not be retained.
                self.capacity_paused.insert(projectile.id);
                continue;
            }
            // Never simulate time preceding this projectile's creation. Bots
            // can fire during this very server tick, and human input can be
            // admitted between two registry steps. Their authenticated source
            // and client catch-up both start at created_tick, not `last`.
            let age = crate::continuum::continuum_tick_diff(current_tick, projectile.created_tick);
            if age < 0 || (weapon_type == 15 && age == 0) {
                // Keep the existing explosion-transaction rule: newborn
                // shrapnel cannot acquire a second same-tick contact.
                continue;
            }
            let motion_ticks = dt_ticks.min(age as u32);
            let dt_seconds = motion_ticks as f32 / 100.0;
            use crate::gameplay_rules::{RulesVector, weapon_motion};
            let mut position = RulesVector::new(projectile.pos_x, projectile.pos_y);
            let mut velocity = projectile.motion_velocity();
            let unlimited_bounce = matches!(
                projectile.behavior,
                crate::gameplay_rules::ProjectileBehavior::BouncingBullet
                    | crate::gameplay_rules::ProjectileBehavior::Burst { .. }
            );
            let mut wall_despawn = false;
            let mut timed_out = false;
            for step_index in 0..motion_ticks {
                let step = weapon_motion::STEP_SECONDS;
                let elapsed = (step_index + 1) as f32 / 100.0;
                let delta = gravity_delta(projectile, position, step);
                velocity.x += delta.x;
                velocity.y += delta.y;
                let contact = weapon_motion::step_terrain(
                    &mut position,
                    &mut velocity,
                    step,
                    unlimited_bounce || projectile.bounces_remaining > 0,
                    weapon_type == 8,
                    &is_solid_for_projectile,
                );
                if is_wormhole(position.x, position.y) {
                    timed_out = true;
                    break;
                }
                if contact.any() {
                    if !unlimited_bounce {
                        if projectile.bounces_remaining == 0 {
                            wall_despawn = true;
                        } else {
                            projectile.bounces_remaining -= 1;
                        }
                    }
                    if matches!(
                        projectile.behavior,
                        crate::gameplay_rules::ProjectileBehavior::Burst { .. }
                    ) {
                        projectile.behavior =
                            crate::gameplay_rules::ProjectileBehavior::Burst { active: true };
                    }
                    let contact_age = age as f32 / 100.0 - dt_seconds + elapsed;
                    if weapon_type == 15 && (wall_despawn || contact_age <= 0.25 + f32::EPSILON) {
                        timed_out = true;
                    }
                    if wall_despawn || timed_out {
                        break;
                    }
                }
            }
            projectile.pos_x = position.x;
            projectile.pos_y = position.y;
            projectile.precise_velocity = Some(velocity);
            projectile.velocity_x =
                crate::continuum_position::protocol_velocity_component(velocity.x);
            projectile.velocity_y =
                crate::continuum_position::protocol_velocity_component(velocity.y);
            projectile.x = position.x.round().clamp(0.0, u16::MAX as f32) as u16;
            projectile.y = position.y.round().clamp(0.0, u16::MAX as f32) as u16;
            if wall_despawn || timed_out {
                despawned.push(index);
            }
            if timed_out {
                continue;
            }

            // Phase 2d: player-hull collision. Check the projectile's
            // post-step position against every player hull. The first
            // qualifying player wins this tick; the projectile is
            // marked for despawn (or for non-bouncing weapons, the
            // single-hit despawn is the only choice). The reference game excludes the
            // projectile owner through the same-frequency filter before hull
            // collision. Bomb self-damage comes from the later explosion
            // transaction, not from the projectile directly intersecting its
            // launch hull.
            // A server-authored damage death retires a hull, not its live
            // weapons. The authenticated immutable carrier preserves their
            // source frequency. Explicit disconnect/session cleanup removes
            // records in the lifecycle owner instead of requiring a live hull.
            let owner = player_by_identity
                .get(&projectile.owner_identity())
                .copied();
            if owner.is_none() && captured_source.is_none() {
                despawned.push(index);
                continue;
            }
            let owner_freq = projectile_frequency;

            if matches!(
                projectile.behavior,
                crate::gameplay_rules::ProjectileBehavior::Burst { active: false }
            ) {
                continue;
            }

            // Phase 2e + Phase 2 polish (RTT): per-shooter rewind
            // tick. Use the exact current owner's measured
            // `lag_compensation_ticks` (smoothed RTT/2 capped at 20 ticks /
            // 200 ms). Retired source-native weapons use current exact target
            // history; human observer projection must still pass its separate
            // authenticated timeline query and never gains a native fallback.
            let rewind_ticks = owner.map_or(0, |owner| u32::from(owner.lag_compensation_ticks));
            let rewind_target_tick =
                crate::continuum::continuum_tick_sub(current_tick, rewind_ticks);

            if matches!(weapon_type, 4 | 8) {
                if let Some(target_identity) = projectile.proximity_target {
                    let Some(target) = player_by_identity.get(&target_identity).copied() else {
                        despawned.push(index);
                        continue;
                    };
                    if target_identity != projectile.owner_identity()
                        && target.frequency == owner_freq
                    {
                        despawned.push(index);
                        continue;
                    }
                    let exact_target_sample =
                        resolve_collision_target_witness(target, rewind_target_tick);
                    let target_presentation_geometry = evaluate_presentation_geometry(
                        projectile,
                        target,
                        current_tick,
                        &mut presentation_geometry_metrics,
                        &mut presentation_geometry,
                    );
                    let target_sample = select_collision_target_witness(
                        target,
                        exact_target_sample,
                        target_presentation_geometry,
                    );
                    if !target_sample.collision_available {
                        continue;
                    }
                    let dx = target_sample.x - projectile.pos_x;
                    let dy = target_sample.y - projectile.pos_y;
                    let highest_offset = dx.abs().max(dy.abs());
                    let deadline_reached = projectile
                        .proximity_sensor_end_tick
                        .is_some_and(|deadline| tick_gte(current_tick, deadline));
                    if highest_offset > projectile.proximity_highest_offset || deadline_reached {
                        let hit = ProjectileHit {
                            projectile_id: projectile.id,
                            owner_id: projectile.owner_id,
                            owner_life_seq: projectile.owner_life_seq,
                            victim_id: target_identity.player_id,
                            victim_life_seq: target_identity.life_seq,
                            weapon_data: projectile.weapon_data,
                            x: projectile.x,
                            y: projectile.y,
                            tick: current_tick,
                        };
                        pending_total += 1;
                        *pending_by_owner
                            .entry(projectile.owner_identity())
                            .or_default() += 1;
                        let harmful = harmful_eligibility(&hit);
                        if !harmful {
                            new_harmful_readiness_deferrals =
                                new_harmful_readiness_deferrals.saturating_add(1);
                        }
                        if !harmful_barrier && harmful {
                            hits.push(hit);
                        } else {
                            harmful_barrier = true;
                        }
                        new_pending_hits.push(PendingProjectileHit {
                            hit,
                            shot_group_id: projectile.shot_group_id,
                            consume_group: matches!(weapon_type, 1 | 2),
                            witness: projectile_collision_witness(
                                projectile,
                                target,
                                target_sample,
                                current_tick,
                                if deadline_reached {
                                    ProjectileContactReason::ProximityDeadline
                                } else {
                                    ProjectileContactReason::ProximityClosestApproach
                                },
                                proximity_distance,
                                bomb_explode_pixels,
                                target_presentation_geometry,
                            ),
                        });
                    } else {
                        projectile.proximity_highest_offset = highest_offset;
                    }
                    continue;
                }
            }

            let mut proximity_candidate = None;
            let mut proximity_hit = false;
            for player in players {
                if player.identity() == projectile.owner_identity()
                    || player.frequency == owner_freq
                {
                    if let Some(diagnostics) = &mut self.contact_diagnostics {
                        diagnostics.observe(
                            projectile,
                            player,
                            owner_freq,
                            current_tick,
                            None,
                            None,
                            super::ContactDiagnosticReason::SameFrequency,
                        );
                    }
                    // Direct projectile contact never damages the owner or a
                    // teammate. Bomb self-damage is evaluated once from the
                    // frozen explosion splash, so these impossible direct
                    // candidates need no observer projection.
                    continue;
                }
                let exact_target_sample =
                    resolve_collision_target_witness(player, rewind_target_tick);
                let target_presentation_geometry = evaluate_presentation_geometry(
                    projectile,
                    player,
                    current_tick,
                    &mut presentation_geometry_metrics,
                    &mut presentation_geometry,
                );
                let target_sample = select_collision_target_witness(
                    player,
                    exact_target_sample,
                    target_presentation_geometry,
                );
                if !target_sample.collision_available {
                    if let Some(diagnostics) = &mut self.contact_diagnostics {
                        diagnostics.observe(
                            projectile,
                            player,
                            owner_freq,
                            current_tick,
                            Some(target_sample),
                            Some(target_presentation_geometry),
                            super::ContactDiagnosticReason::GeometryUnavailable,
                        );
                    }
                    continue;
                }
                let dx = target_sample.x - projectile.pos_x;
                let dy = target_sample.y - projectile.pos_y;
                let (direct_hit, within_contact_envelope) = projectile_contact_envelope(
                    projectile,
                    player,
                    target_sample.x,
                    target_sample.y,
                    proximity_distance,
                );
                if !within_contact_envelope {
                    if let Some(diagnostics) = &mut self.contact_diagnostics {
                        diagnostics.observe(
                            projectile,
                            player,
                            owner_freq,
                            current_tick,
                            Some(target_sample),
                            Some(target_presentation_geometry),
                            super::ContactDiagnosticReason::OutsideContactEnvelope,
                        );
                    }
                    continue;
                }
                if !direct_hit {
                    if matches!(weapon_type, 4 | 8) && proximity_candidate.is_none() {
                        proximity_candidate = Some((
                            player.identity(),
                            dx.abs().max(dy.abs()),
                            target_presentation_geometry,
                        ));
                    }
                    continue;
                }
                let hit = ProjectileHit {
                    projectile_id: projectile.id,
                    owner_id: projectile.owner_id,
                    owner_life_seq: projectile.owner_life_seq,
                    victim_id: player.player_id,
                    victim_life_seq: player.life_seq,
                    weapon_data: projectile.weapon_data,
                    x: projectile.x,
                    y: projectile.y,
                    tick: current_tick,
                };
                pending_total += 1;
                *pending_by_owner
                    .entry(projectile.owner_identity())
                    .or_default() += 1;
                let harmful = harmful_eligibility(&hit);
                if let Some(diagnostics) = &mut self.contact_diagnostics {
                    diagnostics.observe(
                        projectile,
                        player,
                        owner_freq,
                        current_tick,
                        Some(target_sample),
                        Some(target_presentation_geometry),
                        if harmful {
                            super::ContactDiagnosticReason::ContactFrozenReady
                        } else {
                            super::ContactDiagnosticReason::ContactFrozenNotHarmful
                        },
                    );
                }
                if !harmful {
                    new_harmful_readiness_deferrals =
                        new_harmful_readiness_deferrals.saturating_add(1);
                }
                if !harmful_barrier && harmful {
                    hits.push(hit);
                } else {
                    harmful_barrier = true;
                }
                // The sort_unstable + dedup pass below tolerates a
                // duplicate push from the wall-despawn branch above,
                // so we no longer need a defensive `contains` guard.
                new_pending_hits.push(PendingProjectileHit {
                    hit,
                    shot_group_id: projectile.shot_group_id,
                    consume_group: matches!(weapon_type, 1 | 2),
                    witness: projectile_collision_witness(
                        projectile,
                        player,
                        target_sample,
                        current_tick,
                        ProjectileContactReason::Direct,
                        proximity_distance,
                        bomb_explode_pixels,
                        target_presentation_geometry,
                    ),
                });
                if matches!(weapon_type, 1 | 2) {
                    consumed_gun_groups.insert(projectile.shot_group_id);
                }
                proximity_hit = true;
                break;
            }
            if !proximity_hit
                && wall_despawn
                && matches!(weapon_type, 3 | 4)
                && bomb_explode_pixels > 0.0
            {
                let radius = bomb_explode_pixels
                    * f32::from(continuum_weapon_level(projectile.weapon_data).max(1));
                let radius_sq = radius * radius;
                let wall_target = players
                    .iter()
                    .filter(|player| {
                        player.player_id == projectile.owner_id || player.frequency != owner_freq
                    })
                    .filter_map(|player| {
                        let exact_target_sample =
                            resolve_collision_target_witness(player, rewind_target_tick);
                        let target_presentation_geometry = evaluate_presentation_geometry(
                            projectile,
                            player,
                            current_tick,
                            &mut presentation_geometry_metrics,
                            &mut presentation_geometry,
                        );
                        let target_sample = select_collision_target_witness(
                            player,
                            exact_target_sample,
                            target_presentation_geometry,
                        );
                        if !target_sample.collision_available {
                            return None;
                        }
                        let dx = target_sample.x - projectile.pos_x;
                        let dy = target_sample.y - projectile.pos_y;
                        let distance_sq = dx * dx + dy * dy;
                        (distance_sq < radius_sq).then_some((
                            distance_sq,
                            player.player_id,
                            player,
                            target_sample,
                            target_presentation_geometry,
                        ))
                    })
                    .min_by(|left, right| {
                        left.0
                            .total_cmp(&right.0)
                            .then_with(|| left.1.cmp(&right.1))
                    })
                    .map(|(_, _, player, sample, presentation)| (player, sample, presentation));
                if let Some((target, target_sample, target_presentation_geometry)) = wall_target {
                    let target_identity = target.identity();
                    let hit = ProjectileHit {
                        projectile_id: projectile.id,
                        owner_id: projectile.owner_id,
                        owner_life_seq: projectile.owner_life_seq,
                        victim_id: target_identity.player_id,
                        victim_life_seq: target_identity.life_seq,
                        weapon_data: projectile.weapon_data,
                        x: projectile.x,
                        y: projectile.y,
                        tick: current_tick,
                    };
                    pending_total += 1;
                    *pending_by_owner
                        .entry(projectile.owner_identity())
                        .or_default() += 1;
                    let harmful = harmful_eligibility(&hit);
                    if !harmful {
                        new_harmful_readiness_deferrals =
                            new_harmful_readiness_deferrals.saturating_add(1);
                    }
                    if !harmful_barrier && harmful {
                        hits.push(hit);
                    } else {
                        harmful_barrier = true;
                    }
                    new_pending_hits.push(PendingProjectileHit {
                        hit,
                        shot_group_id: projectile.shot_group_id,
                        consume_group: false,
                        witness: projectile_collision_witness(
                            projectile,
                            target,
                            target_sample,
                            current_tick,
                            ProjectileContactReason::WallExplosion,
                            proximity_distance,
                            bomb_explode_pixels,
                            target_presentation_geometry,
                        ),
                    });
                    proximity_hit = true;
                }
            }
            if !proximity_hit && !wall_despawn {
                if let Some((target_identity, highest_offset, target_presentation_geometry)) =
                    proximity_candidate
                {
                    if bomb_explode_delay_ticks == 0 {
                        let Some(target) = player_by_identity.get(&target_identity).copied() else {
                            continue;
                        };
                        let target_sample = select_collision_target_witness(
                            target,
                            resolve_collision_target_witness(target, rewind_target_tick),
                            target_presentation_geometry,
                        );
                        if !target_sample.collision_available {
                            continue;
                        }
                        let hit = ProjectileHit {
                            projectile_id: projectile.id,
                            owner_id: projectile.owner_id,
                            owner_life_seq: projectile.owner_life_seq,
                            victim_id: target_identity.player_id,
                            victim_life_seq: target_identity.life_seq,
                            weapon_data: projectile.weapon_data,
                            x: projectile.x,
                            y: projectile.y,
                            tick: current_tick,
                        };
                        pending_total += 1;
                        *pending_by_owner
                            .entry(projectile.owner_identity())
                            .or_default() += 1;
                        let harmful = harmful_eligibility(&hit);
                        if !harmful {
                            new_harmful_readiness_deferrals =
                                new_harmful_readiness_deferrals.saturating_add(1);
                        }
                        if !harmful_barrier && harmful {
                            hits.push(hit);
                        } else {
                            harmful_barrier = true;
                        }
                        new_pending_hits.push(PendingProjectileHit {
                            hit,
                            shot_group_id: projectile.shot_group_id,
                            consume_group: matches!(weapon_type, 1 | 2),
                            witness: projectile_collision_witness(
                                projectile,
                                target,
                                target_sample,
                                current_tick,
                                ProjectileContactReason::ProximityImmediate,
                                proximity_distance,
                                bomb_explode_pixels,
                                target_presentation_geometry,
                            ),
                        });
                    } else {
                        projectile.proximity_target = Some(target_identity);
                        projectile.proximity_sensor_end_tick =
                            Some(crate::continuum::continuum_tick_add(
                                current_tick,
                                bomb_explode_delay_ticks,
                            ));
                        projectile.proximity_highest_offset = highest_offset;
                    }
                }
            }
            if wall_despawn
                && !proximity_hit
                && matches!(weapon_type, 3 | 4)
                && projectile.presentation_source.is_some()
            {
                new_wall_explosions.push(ProjectileWallExplosion {
                    parent: *projectile,
                    collision_tick: current_tick,
                });
                *wall_explosions_by_owner
                    .entry(projectile.owner_identity())
                    .or_default() += 1;
            }
        }
        if !consumed_gun_groups.is_empty() {
            for (index, projectile) in self.records.iter().enumerate() {
                if consumed_gun_groups.contains(&projectile.shot_group_id) {
                    despawned.push(index);
                }
            }
        }
        let newly_pending_groups = new_pending_hits
            .iter()
            .map(|pending| pending.shot_group_id)
            .collect::<std::collections::HashSet<_>>();
        despawned.retain(|&index| {
            self.records
                .get(index)
                .is_some_and(|projectile| !newly_pending_groups.contains(&projectile.shot_group_id))
        });
        // Apply despawn marks (in reverse so indices stay valid).
        despawned.sort_unstable();
        despawned.dedup();
        for index in despawned.into_iter().rev() {
            self.records.remove(index);
        }
        self.pending_hits.extend(new_pending_hits);
        self.pending_wall_explosions.extend(new_wall_explosions);
        self.harmful_readiness_deferrals_total = self
            .harmful_readiness_deferrals_total
            .saturating_add(new_harmful_readiness_deferrals);
        self.record_presentation_geometry_metrics(presentation_geometry_metrics);
        hits
    }

    /// Include secondary splash projection work performed while freezing the
    /// collision transaction, as well as primary registry collision checks.
    pub(crate) fn record_presentation_geometry_metrics(
        &mut self,
        presentation_geometry_metrics: PresentationGeometryMetrics,
    ) {
        self.presentation_geometry_lookups_total = self
            .presentation_geometry_lookups_total
            .saturating_add(presentation_geometry_metrics.lookups_total);
        self.presentation_geometry_resolved_total = self
            .presentation_geometry_resolved_total
            .saturating_add(presentation_geometry_metrics.resolved_total);
        self.presentation_geometry_unavailable_total = self
            .presentation_geometry_unavailable_total
            .saturating_add(presentation_geometry_metrics.unavailable_total);
        self.presentation_geometry_server_native_total = self
            .presentation_geometry_server_native_total
            .saturating_add(presentation_geometry_metrics.server_native_total);
        self.presentation_geometry_local_self_total = self
            .presentation_geometry_local_self_total
            .saturating_add(presentation_geometry_metrics.local_self_total);
        self.presentation_geometry_server_advanced_total = self
            .presentation_geometry_server_advanced_total
            .saturating_add(presentation_geometry_metrics.server_advanced_total);
        self.presentation_geometry_eval_nanos_total = self
            .presentation_geometry_eval_nanos_total
            .saturating_add(presentation_geometry_metrics.eval_nanos_total);
        self.presentation_geometry_eval_nanos_max = self
            .presentation_geometry_eval_nanos_max
            .max(presentation_geometry_metrics.eval_nanos_max);
    }

    pub(crate) fn expire_older_than_by_weapon(
        &mut self,
        tick: u32,
        default_max_age_ticks: u32,
        mine_max_age_ticks: u32,
    ) {
        let pending_groups = self.pending_group_ids();
        self.records.retain(|projectile| {
            let max_age_ticks = if continuum_weapon_is_mine(projectile.weapon_data) {
                mine_max_age_ticks
            } else {
                default_max_age_ticks
            };
            pending_groups.contains(&projectile.shot_group_id)
                || crate::continuum::continuum_tick_elapsed(tick, projectile.created_tick)
                    <= max_age_ticks
        });
    }

    pub(crate) fn has_recent_weapon(
        &self,
        owner_id: u16,
        weapon_data: u16,
        tick: u32,
        max_age_ticks: u32,
    ) -> bool {
        self.records.iter().any(|projectile| {
            projectile.owner_id == owner_id
                && crate::continuum::continuum_tick_elapsed(tick, projectile.created_tick)
                    <= max_age_ticks
                && projectile.matches_damage_weapon(weapon_data)
        })
    }

    fn trim_owner(&mut self, owner_id: u16, max_per_owner: usize) {
        if max_per_owner == 0 {
            let pending_groups = self.pending_group_ids();
            self.records.retain(|projectile| {
                projectile.owner_id != owner_id
                    || pending_groups.contains(&projectile.shot_group_id)
            });
            return;
        }

        let pending_groups = self.pending_group_ids();
        let mut owner_count = self
            .records
            .iter()
            .filter(|projectile| projectile.owner_id == owner_id)
            .count();
        if owner_count <= max_per_owner {
            return;
        }

        self.records.retain(|projectile| {
            if projectile.owner_id != owner_id || owner_count <= max_per_owner {
                return true;
            }
            if pending_groups.contains(&projectile.shot_group_id) {
                return true;
            }
            owner_count -= 1;
            false
        });
    }

    /// Index of the record that one unit of global capacity is charged to.
    ///
    /// Charging the globally oldest record let one continuously firing owner
    /// keep paying with somebody else's history: whenever the registry crossed
    /// its cap, whichever honest owner happened to hold the oldest record lost
    /// it, while the heaviest publisher never shed its own volume. Charging the
    /// busiest owner first keeps every owner's retained share near its fair
    /// share, ties break to the globally oldest record, and a group with a
    /// pending collision is still never evictable.
    fn global_trim_eviction_index(
        &self,
        pending_groups: &std::collections::HashSet<u64>,
    ) -> Option<usize> {
        #[derive(Default)]
        struct OwnerCapacity {
            count: usize,
            oldest_evictable: Option<usize>,
        }

        let mut by_owner: std::collections::BTreeMap<u16, OwnerCapacity> =
            std::collections::BTreeMap::new();
        for (index, record) in self.records.iter().enumerate() {
            let entry = by_owner.entry(record.owner_id).or_default();
            entry.count += 1;
            if entry.oldest_evictable.is_none() && !pending_groups.contains(&record.shot_group_id) {
                entry.oldest_evictable = Some(index);
            }
        }

        let mut chosen: Option<(usize, usize)> = None; // (busiest count, index)
        for capacity in by_owner.values() {
            let Some(index) = capacity.oldest_evictable else {
                continue;
            };
            let better = match chosen {
                None => true,
                Some((best_count, best_index)) => {
                    capacity.count > best_count
                        || (capacity.count == best_count && index < best_index)
                }
            };
            if better {
                chosen = Some((capacity.count, index));
            }
        }
        chosen.map(|(_, index)| index)
    }

    fn trim_total(&mut self, max_total: usize) {
        let pending_groups = self.pending_group_ids();
        while self.records.len() > max_total {
            let Some(index) = self.global_trim_eviction_index(&pending_groups) else {
                break;
            };
            self.records.remove(index);
        }
    }

    fn trim_owner_groups(&mut self, owner_id: u16, max_per_owner: usize) {
        let pending_groups = self.pending_group_ids();
        while self
            .records
            .iter()
            .filter(|projectile| projectile.owner_id == owner_id)
            .count()
            > max_per_owner
        {
            let Some(oldest_group) = self
                .records
                .iter()
                .find(|projectile| {
                    projectile.owner_id == owner_id
                        && !pending_groups.contains(&projectile.shot_group_id)
                })
                .map(|projectile| projectile.shot_group_id)
            else {
                break;
            };
            self.records.retain(|projectile| {
                projectile.owner_id != owner_id || projectile.shot_group_id != oldest_group
            });
        }
    }

    fn trim_total_groups(&mut self, max_total: usize) {
        let pending_groups = self.pending_group_ids();
        while self.records.len() > max_total {
            let Some(index) = self.global_trim_eviction_index(&pending_groups) else {
                break;
            };
            let evicted_group = self.records[index].shot_group_id;
            self.records
                .retain(|projectile| projectile.shot_group_id != evicted_group);
        }
    }
}

pub(crate) fn evaluate_presentation_geometry(
    projectile: &ProjectileRecord,
    target: &PlayerHullSnapshot,
    current_tick: u32,
    metrics: &mut PresentationGeometryMetrics,
    evaluator: &mut impl FnMut(
        &ProjectileRecord,
        &PlayerHullSnapshot,
        u32,
    ) -> crate::combat_timeline::ProjectilePresentationWitnessV1,
) -> crate::combat_timeline::ProjectilePresentationWitnessV1 {
    if projectile.combat_timeline.is_none() {
        let witness = crate::combat_timeline::ProjectilePresentationWitnessV1::server_native();
        metrics.observe_result(witness);
        return witness;
    }
    let started = std::time::Instant::now();
    let witness = evaluator(projectile, target, current_tick);
    let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
    metrics.eval_nanos_total = metrics.eval_nanos_total.saturating_add(elapsed);
    metrics.eval_nanos_max = metrics.eval_nanos_max.max(elapsed);
    metrics.observe_result(witness);
    witness
}

fn tick_gte(a: u32, b: u32) -> bool {
    crate::continuum::continuum_tick_reached(a, b)
}

/// Resolve the sole exact production motion segment at `target_tick`. Missing
/// history fails closed instead of substituting current or legacy position.
pub(crate) fn resolve_collision_target_witness(
    player: &PlayerHullSnapshot,
    target_tick: u32,
) -> CollisionTargetWitness {
    match exact_motion_witness(player, target_tick) {
        Ok(exact) => CollisionTargetWitness {
            x: exact.x,
            y: exact.y,
            requested_tick: target_tick,
            selected_tick: Some(exact.selected_tick),
            source: MotionHistorySource::UnifiedExactSegment,
            collision_available: true,
        },
        Err(_) => CollisionTargetWitness {
            x: player.x,
            y: player.y,
            requested_tick: target_tick,
            selected_tick: None,
            source: MotionHistorySource::UnifiedHistoryUnavailable,
            collision_available: false,
        },
    }
}

/// Select the one production collision anchor for a projectile-target pair.
/// Human projectiles may only use their verified observer presentation; the
/// exact server sample remains its authority bound and diagnostic witness, not
/// a fallback into a different visible world. Server-native projectiles retain
/// the exact authoritative-motion path.
pub(crate) fn select_collision_target_witness(
    player: &PlayerHullSnapshot,
    exact: CollisionTargetWitness,
    presentation: crate::combat_timeline::ProjectilePresentationWitnessV1,
) -> CollisionTargetWitness {
    use crate::combat_timeline::ProjectilePresentationStatusV1;

    let mut selected = match presentation.status {
        ProjectilePresentationStatusV1::ServerNative => exact,
        ProjectilePresentationStatusV1::LocalSelf | ProjectilePresentationStatusV1::Resolved(_) => {
            let (x_q8, y_q8) = presentation
                .target_anchor_q8()
                .expect("resolved presentation status carries an anchor");
            CollisionTargetWitness {
                x: x_q8 as f32 / 256.0,
                y: y_q8 as f32 / 256.0,
                requested_tick: presentation.render_tick,
                selected_tick: Some(presentation.render_tick),
                source: if matches!(
                    presentation.status,
                    ProjectilePresentationStatusV1::LocalSelf
                ) {
                    MotionHistorySource::LocalSelf
                } else {
                    MotionHistorySource::ObserverPresentation
                },
                collision_available: true,
            }
        }
        ProjectilePresentationStatusV1::NotEvaluated
        | ProjectilePresentationStatusV1::QueryUnavailable(_)
        | ProjectilePresentationStatusV1::ProjectionUnavailable(_) => CollisionTargetWitness {
            x: exact.x,
            y: exact.y,
            requested_tick: presentation.render_tick,
            selected_tick: None,
            source: MotionHistorySource::ObserverPresentationUnavailable,
            collision_available: false,
        },
    };
    // A live weapon may outlast a victim's prior life. It can hit the new
    // hull using current-life geometry, never a held/rewound pre-birth sample.
    if selected.selected_tick.is_none_or(|tick| {
        player
            .life_started_tick
            .is_some_and(|started| crate::continuum::continuum_tick_diff(tick, started) < 0)
    }) {
        selected.collision_available = false;
    }
    selected
}

fn exact_motion_witness(
    player: &PlayerHullSnapshot,
    target_tick: u32,
) -> Result<ExactMotionSample, crate::combat_timeline::HistoryUnavailable> {
    let Some(segment) = player.motion_segment else {
        return Err(crate::combat_timeline::HistoryUnavailable::Empty);
    };
    if player.motion_samples.is_empty() {
        return Err(crate::combat_timeline::HistoryUnavailable::Empty);
    }
    let Some(oldest) = player
        .motion_samples
        .iter()
        .find(|sample| sample.segment == segment)
    else {
        return Err(crate::combat_timeline::HistoryUnavailable::WrongSegment);
    };
    if crate::continuum::continuum_tick_diff(target_tick, oldest.server_tick) < 0 {
        return Err(crate::combat_timeline::HistoryUnavailable::BeforeRetainedWindow);
    }
    player
        .motion_samples
        .iter()
        .rev()
        .find(|sample| {
            sample.segment == segment
                && crate::continuum::continuum_tick_diff(target_tick, sample.server_tick) >= 0
        })
        .map(|sample| {
            let (x, y) = sample.position();
            ExactMotionSample {
                x,
                y,
                selected_tick: sample.server_tick,
            }
        })
        .ok_or(crate::combat_timeline::HistoryUnavailable::NoSampleAtOrBefore)
}

fn rewound_position(player: &PlayerHullSnapshot, target_tick: u32) -> (f32, f32) {
    let sample = resolve_collision_target_witness(player, target_tick);
    (sample.x, sample.y)
}

fn projectile_contact_envelope(
    projectile: &ProjectileRecord,
    target: &PlayerHullSnapshot,
    target_x: f32,
    target_y: f32,
    proximity_distance: f32,
) -> (bool, bool) {
    let weapon_type = continuum_weapon_type(projectile.weapon_data);
    // The reference game uses this same 4px weapon/ship box for guns, activated Burst,
    // fragments and explosives; a point/circle check silently misses corners.
    let direct_hit = continuum_weapon_contact_overlaps_ship(
        projectile.pos_x,
        projectile.pos_y,
        target_x,
        target_y,
        target.radius_px,
    );
    let within_contact_envelope = if matches!(weapon_type, 4 | 8) {
        // Proximity uses the source-authored square weapon box, not radial
        // distance. Every production anchor consumes this one helper so the
        // contact shape cannot drift between authority sources.
        continuum_proximity_sensor_overlaps_ship(
            projectile.pos_x,
            projectile.pos_y,
            target_x,
            target_y,
            continuum_weapon_proximity_trigger_radius(proximity_distance, projectile.weapon_data),
            target.radius_px,
        )
    } else {
        direct_hit
    };
    (direct_hit, within_contact_envelope)
}

fn projectile_collision_witness(
    projectile: &ProjectileRecord,
    target: &PlayerHullSnapshot,
    target_sample: CollisionTargetWitness,
    collision_tick: u32,
    contact_reason: ProjectileContactReason,
    proximity_distance: f32,
    bomb_explode_pixels: f32,
    presentation_geometry: crate::combat_timeline::ProjectilePresentationWitnessV1,
) -> ProjectileCollisionWitness {
    let dx = target_sample.x - projectile.pos_x;
    let dy = target_sample.y - projectile.pos_y;
    let weapon_type = continuum_weapon_type(projectile.weapon_data);
    ProjectileCollisionWitness {
        projectile_id: projectile.id,
        shot_group_id: projectile.shot_group_id,
        owner: projectile.owner_identity(),
        target: target.identity(),
        projectile_created_tick: projectile.created_tick,
        collision_tick,
        requested_target_tick: target_sample.requested_tick,
        selected_target_tick: target_sample.selected_tick,
        target_motion_segment: target.motion_segment,
        rewind_ticks: crate::continuum::continuum_tick_elapsed(
            collision_tick,
            target_sample.requested_tick,
        ),
        projectile_x: projectile.pos_x,
        projectile_y: projectile.pos_y,
        target_current_x: target.x,
        target_current_y: target.y,
        target_anchor_x: target_sample.x,
        target_anchor_y: target_sample.y,
        center_distance: (dx * dx + dy * dy).sqrt(),
        target_radius: target.radius_px,
        proximity_trigger_radius: matches!(weapon_type, 4 | 8).then(|| {
            continuum_weapon_proximity_trigger_radius(proximity_distance, projectile.weapon_data)
        }),
        explosion_radius: matches!(weapon_type, 3 | 4 | 8).then_some(
            bomb_explode_pixels
                * f32::from(continuum_weapon_explosion_level(projectile.weapon_data)),
        ),
        history_source: target_sample.source,
        contact_reason,
        presentation_geometry,
    }
}
