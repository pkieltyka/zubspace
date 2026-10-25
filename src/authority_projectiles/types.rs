use crate::authority_identity::LifeIdentity;
use crate::continuum_weapon::{
    continuum_weapon_level, continuum_weapon_shrapnel_count, continuum_weapon_shrapnel_level,
    continuum_weapon_type,
};
use crate::gameplay_rules::ProjectileBehavior;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectileRecord {
    pub(crate) id: u32,
    /// Never-reused server-owned id shared by every child created from one
    /// accepted weapon packet. Gun children use it for the reference game's link removal.
    pub(crate) shot_group_id: u64,
    /// Stable child index within the immutable accepted weapon carrier.
    /// Repel presentation reconciliation uses it after siblings despawn.
    pub(crate) shot_index: u8,
    /// Immutable self-contained source used to reconstruct presentation for a
    /// recipient that enters this projectile's AOI after the original weapon
    /// fan-out. Production projectiles always carry it; `None` is reserved for
    /// isolated authority fixtures that do not exercise network presentation.
    pub(crate) presentation_source: Option<crate::realtime_event::RealtimeWeaponEvent>,
    pub(crate) owner_id: u16,
    pub(crate) owner_life_seq: u32,
    /// Human-fired projectiles carry the immutable, server-verified observer
    /// view that authorized their weapon edge. `None` is reserved for the
    /// server-native bot path and isolated authority fixtures; it never grants a
    /// human weapon fallback.
    pub(crate) combat_timeline: Option<crate::combat_timeline::ProjectileCombatTimelineV1>,
    pub(crate) weapon_data: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) timestamp: u32,
    pub(crate) created_tick: u32,
    /// Immutable accepted carrier tick, inherited by explosion fragments.
    /// Repel can restart fragment age without changing its presentation identity.
    pub(crate) source_server_tick: Option<u32>,
    /// Wire-format velocity mirrors, in tenths of px/s. Production integration
    /// keeps unquantized velocity below; quantization belongs at serialization.
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    /// Unquantized simulation velocity; legacy wire-only fixtures start at None.
    pub(crate) precise_velocity: Option<crate::gameplay_rules::RulesVector>,
    /// Phase 2a: Continuum tick at which this projectile despawns
    /// regardless of collisions. Computed from spawn tick + the arena
    /// weapon-alive-time setting (BulletAliveTime, BombAliveTime,
    /// MineAliveTime). `None` means the weapon has no configured lifetime;
    /// an expiry whose wrapped wire value is exactly zero remains `Some(0)`.
    pub(crate) expiry_tick: Option<u32>,
    /// Phase 2b: precise position in pixels, kept as f32 so the
    /// per-tick stepper can integrate sub-pixel motion without
    /// accumulating rounding error. The wire-format `x` / `y` fields
    /// are set to `pos_x.round()` / `pos_y.round()` at spawn and after
    /// each step, so all the existing damage-validation paths that
    /// read `x` / `y` continue to work unmodified.
    pub(crate) pos_x: f32,
    pub(crate) pos_y: f32,
    /// Phase 2c: number of wall bounces this projectile has left.
    /// At spawn:
    /// - Bouncing bombs (type 3 or 4 with the alternate bit set):
    ///   `BombBounceCount` from the firing ship's settings.
    /// - Bursts (type 7): u8::MAX — bursts bounce until their
    ///   alive-time expires, never on a bounce-count limit.
    /// - Everything else (regular bullets/bombs/mines/Thors): `0`.
    /// Each tile collision decrements; despawn on the bounce after
    /// the counter reaches `0`.
    pub(crate) bounces_remaining: u8,
    /// Behavior that cannot be recovered from a finite bounce counter alone.
    pub(crate) behavior: ProjectileBehavior,
    /// Proximity bombs arm on the first enemy that enters the proximity
    /// trigger radius, then continue flying until they get closer/direct,
    /// the target starts moving away, or BombExplodeDelay expires.
    pub(crate) proximity_target: Option<LifeIdentity>,
    pub(crate) proximity_sensor_end_tick: Option<u32>,
    pub(crate) proximity_highest_offset: f32,
}

/// Phase 2d: snapshot of one player's hull for projectile hit
/// detection. Built by the caller (`run_server_tick`) from the
/// authoritative server-side view of every alive client.
///
/// `motion_segment` plus `motion_samples` are the sole production collision
/// history. Missing exact history fails closed; current position is never a
/// collision fallback.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlayerHullSnapshot {
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) frequency: u16,
    pub(crate) radius_px: f32,
    /// Exact-segment production history.
    pub(crate) motion_segment: Option<crate::combat_timeline::MotionSegmentIdentity>,
    pub(crate) motion_samples: Vec<crate::combat_timeline::ActorMotionSampleV1>,
    pub(crate) motion_source_kind: Option<crate::combat_timeline::ActorMotionSourceKind>,
    /// Phase 2 polish (RTT): per-shooter rewind window in Continuum
    /// ticks. Populated from `ClientHandle.lag_compensation_ticks`,
    /// which is derived from EWMA-smoothed snapshot-ack RTT. The
    /// projectile step uses the owner's value when rewinding victim
    /// hulls.
    pub(crate) lag_compensation_ticks: u8,
    /// Server tick at which this player's current life began. Collision
    /// geometry must be sampled at or after this boundary; live projectiles
    /// fired earlier may still hit the new hull through current-life geometry.
    /// This gate is cleared after every projectile that could predate the
    /// life has aged out, preventing a dormant 31-bit timestamp from being
    /// reinterpreted after the signed half-range.
    pub(crate) life_started_tick: Option<u32>,
}

/// Phase 2d: a server-detected projectile-on-player hit. Phase 2f
/// will turn these into authoritative damage events; for now the
/// caller logs them so we can soak in Chrome and verify the
/// detector fires when expected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectileHit {
    pub(crate) projectile_id: u32,
    pub(crate) owner_id: u16,
    pub(crate) owner_life_seq: u32,
    pub(crate) victim_id: u16,
    pub(crate) victim_life_seq: u32,
    pub(crate) weapon_data: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) tick: u32,
}

/// Why the authoritative projectile step froze a collision. Keeping this
/// distinct from the weapon type makes proximity-delay, closest-approach, and
/// wall-explosion disagreements diagnosable without reverse engineering the
/// mutable projectile record afterward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectileContactReason {
    Direct,
    ProximityImmediate,
    ProximityClosestApproach,
    ProximityDeadline,
    WallExplosion,
}

impl ProjectileContactReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ProximityImmediate => "proximity_immediate",
            Self::ProximityClosestApproach => "proximity_closest_approach",
            Self::ProximityDeadline => "proximity_deadline",
            Self::WallExplosion => "wall_explosion",
        }
    }
}

/// Exact source of the selected target anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionHistorySource {
    /// A server-native projectile (currently bots and isolated authority
    /// fixtures) used the target's exact authoritative motion segment.
    UnifiedExactSegment,
    /// A human projectile used the exact hull that the firing client was
    /// verified to have rendered when the input edge was accepted.
    ObserverPresentation,
    /// A human self-hit used the owner's current accepted local hull because
    /// local prediction is not represented in the remote-observer timeline.
    LocalSelf,
    UnifiedHistoryUnavailable,
    /// The human projectile's immutable observer timeline could not produce a
    /// verified target hull. Contact fails closed; exact server motion is not
    /// a compatibility fallback for a different visible world.
    ObserverPresentationUnavailable,
}

impl MotionHistorySource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UnifiedExactSegment => "unified_exact_segment",
            Self::ObserverPresentation => "observer_presentation",
            Self::LocalSelf => "local_self",
            Self::UnifiedHistoryUnavailable => "unified_history_unavailable",
            Self::ObserverPresentationUnavailable => "observer_presentation_unavailable",
        }
    }
}

/// Immutable authority witness produced at the same instant as projectile
/// contact. It is stored beside the pending hit so later queue retries and
/// actor motion cannot alter either collision geometry or its explanation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectileCollisionWitness {
    pub(crate) projectile_id: u32,
    pub(crate) shot_group_id: u64,
    pub(crate) owner: LifeIdentity,
    pub(crate) target: LifeIdentity,
    pub(crate) projectile_created_tick: u32,
    pub(crate) collision_tick: u32,
    pub(crate) requested_target_tick: u32,
    pub(crate) selected_target_tick: Option<u32>,
    /// Exact actor-history segment that supplied or bounded
    /// `target_anchor_*`. Production remote contact always carries one.
    pub(crate) target_motion_segment: Option<crate::combat_timeline::MotionSegmentIdentity>,
    pub(crate) rewind_ticks: u32,
    pub(crate) projectile_x: f32,
    pub(crate) projectile_y: f32,
    pub(crate) target_current_x: f32,
    pub(crate) target_current_y: f32,
    pub(crate) target_anchor_x: f32,
    pub(crate) target_anchor_y: f32,
    pub(crate) center_distance: f32,
    pub(crate) target_radius: f32,
    pub(crate) proximity_trigger_radius: Option<f32>,
    pub(crate) explosion_radius: Option<f32>,
    pub(crate) history_source: MotionHistorySource,
    pub(crate) contact_reason: ProjectileContactReason,
    /// Verified projection through the human shooter's immutable combat-view
    /// schedule. For a human projectile this is the production collision
    /// owner, not a diagnostic comparison path.
    pub(crate) presentation_geometry: crate::combat_timeline::ProjectilePresentationWitnessV1,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RepelMutationSummary {
    pub(crate) projectiles_repelled: u32,
    pub(crate) mines_converted: u32,
    pub(crate) states: Vec<RepelledProjectileState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RepelledProjectileState {
    pub(crate) motion_frozen: bool,
    pub(crate) projectile_id: u32,
    /// Original committed source, independent of the current roster slot.
    /// Zero is reserved for isolated legacy fixtures without a carrier.
    pub(crate) source_session_id: u64,
    pub(crate) owner_id: u16,
    pub(crate) owner_life_seq: u32,
    pub(crate) created_tick: u32,
    pub(crate) source_weapon_data: u16,
    pub(crate) source_projectile_index: u8,
    pub(crate) result_weapon_data: u16,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) expires_at_tick: u32,
    pub(crate) bounces_remaining: u8,
}

impl ProjectileRecord {
    pub(crate) fn captured_source(self) -> Option<crate::realtime_event::RealtimeWeaponEvent> {
        self.presentation_source.filter(|source| {
            source.source_player_id == self.owner_id
                && source.source_life_seq == self.owner_life_seq
                && source.source_session_id != 0
                && source.shot_group_id == self.shot_group_id
                && source.ship < 8
        })
    }

    pub(super) fn repel_owner_state(
        self,
        owner_state: &std::collections::HashMap<LifeIdentity, (u16, u8)>,
    ) -> Option<(u16, u8, u64)> {
        let &(legacy_frequency, bomb_bounces) = owner_state.get(&self.owner_identity())?;
        match self.presentation_source {
            Some(_) => {
                let source = self.captured_source()?;
                Some((source.frequency, bomb_bounces, source.source_session_id))
            }
            None => Some((legacy_frequency, bomb_bounces, 0)),
        }
    }

    pub(crate) fn motion_velocity(self) -> crate::gameplay_rules::RulesVector {
        self.precise_velocity.unwrap_or_else(|| {
            crate::gameplay_rules::RulesVector::new(
                self.velocity_x as f32 / 10.0,
                self.velocity_y as f32 / 10.0,
            )
        })
    }

    pub(crate) fn owner_identity(self) -> LifeIdentity {
        LifeIdentity::new(self.owner_id, self.owner_life_seq)
    }

    pub(crate) fn matches_damage_weapon(self, reported_weapon_data: u16) -> bool {
        let fired_type = continuum_weapon_type(self.weapon_data);
        let reported_type = continuum_weapon_type(reported_weapon_data);
        fired_type == reported_type
            || shrapnel_report_matches_source(self.weapon_data, reported_weapon_data)
    }
}

impl PlayerHullSnapshot {
    pub(crate) fn identity(&self) -> LifeIdentity {
        LifeIdentity::new(self.player_id, self.life_seq)
    }
}

impl ProjectileHit {
    pub(crate) fn owner_identity(self) -> LifeIdentity {
        LifeIdentity::new(self.owner_id, self.owner_life_seq)
    }

    pub(crate) fn victim_identity(self) -> LifeIdentity {
        LifeIdentity::new(self.victim_id, self.victim_life_seq)
    }
}

fn shrapnel_report_matches_source(source_weapon_data: u16, reported_weapon_data: u16) -> bool {
    if continuum_weapon_type(reported_weapon_data) != 15
        || continuum_weapon_shrapnel_count(source_weapon_data) == 0
    {
        return false;
    }

    let reported_level = continuum_weapon_level(reported_weapon_data);
    match continuum_weapon_type(source_weapon_data) {
        1 | 2 => continuum_weapon_level(source_weapon_data) == reported_level,
        3 | 4 | 8 => continuum_weapon_shrapnel_level(source_weapon_data) == reported_level,
        _ => false,
    }
}
