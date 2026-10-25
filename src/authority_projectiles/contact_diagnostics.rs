use super::{CollisionTargetWitness, PlayerHullSnapshot, ProjectileRecord};
use crate::authority_identity::LifeIdentity;

/// One arena-wide budget, with no unbounded per-player/projectile map. This is
/// diagnostic sampling; missing samples must never be interpreted as no contact.
pub(crate) const MAX_CONTACT_DIAGNOSTIC_SAMPLES: usize = 64;
const WINDOW_TICKS: i32 = 100;
const NEAR_HULL_MARGIN: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContactDiagnosticReason {
    SameFrequency,
    GeometryUnavailable,
    OutsideContactEnvelope,
    ContactFrozenReady,
    ContactFrozenNotHarmful,
}

impl ContactDiagnosticReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SameFrequency => "same_frequency",
            Self::GeometryUnavailable => "geometry_unavailable",
            Self::OutsideContactEnvelope => "outside_contact_envelope",
            Self::ContactFrozenReady => "contact_frozen_ready",
            Self::ContactFrozenNotHarmful => "contact_frozen_not_harmful",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ContactDiagnostic {
    pub(crate) tick: u32,
    pub(crate) projectile: ProjectileRecord,
    pub(crate) source_frequency: u16,
    pub(crate) target: LifeIdentity,
    pub(crate) target_session_id: u64,
    pub(crate) target_frequency: u16,
    pub(crate) target_x: f32,
    pub(crate) target_y: f32,
    pub(crate) target_radius: f32,
    pub(crate) selected: Option<CollisionTargetWitness>,
    pub(crate) presentation: Option<crate::combat_timeline::ProjectilePresentationWitnessV1>,
    pub(crate) reason: ContactDiagnosticReason,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ContactDiagnostics {
    samples: Vec<ContactDiagnostic>,
    seen: Vec<(u32, u64, LifeIdentity)>,
    window_started_tick: Option<u32>,
    suppressed: u64,
}

impl ContactDiagnostics {
    pub(super) fn begin_step(&mut self, tick: u32) {
        if self
            .window_started_tick
            .is_none_or(|start| crate::continuum::continuum_tick_diff(tick, start) >= WINDOW_TICKS)
        {
            self.window_started_tick = Some(tick);
            self.seen.clear();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn observe(
        &mut self,
        projectile: &ProjectileRecord,
        target: &PlayerHullSnapshot,
        source_frequency: u16,
        tick: u32,
        selected: Option<CollisionTargetWitness>,
        presentation: Option<crate::combat_timeline::ProjectilePresentationWitnessV1>,
        reason: ContactDiagnosticReason,
    ) {
        // Record actual decisions for foreign gun/fragment contacts against
        // human hulls. Native bot shots bypass presentation-query callbacks,
        // so sampling belongs in the registry's real candidate loop.
        if target.motion_source_kind
            != Some(crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness)
            || target.player_id == projectile.owner_id
            || !matches!(projectile.weapon_data & 0x1f, 1 | 2 | 7 | 15)
        {
            return;
        }
        let near = |x: f32, y: f32| {
            (projectile.pos_x - x).abs() <= target.radius_px + NEAR_HULL_MARGIN
                && (projectile.pos_y - y).abs() <= target.radius_px + NEAR_HULL_MARGIN
        };
        if !near(target.x, target.y) && !selected.is_some_and(|anchor| near(anchor.x, anchor.y)) {
            return;
        }
        let target_session_id = target
            .motion_segment
            .map_or(0, |segment| segment.actor.session.session_id.get());
        let key = (projectile.id, target_session_id, target.identity());
        if self.seen.contains(&key) {
            return;
        }
        if self.seen.len() >= MAX_CONTACT_DIAGNOSTIC_SAMPLES
            || self.samples.len() >= MAX_CONTACT_DIAGNOSTIC_SAMPLES
        {
            self.suppressed = self.suppressed.saturating_add(1);
            return;
        }
        self.seen.push(key);
        self.samples.push(ContactDiagnostic {
            tick,
            projectile: *projectile,
            source_frequency,
            target: target.identity(),
            target_session_id,
            target_frequency: target.frequency,
            target_x: target.x,
            target_y: target.y,
            target_radius: target.radius_px,
            selected,
            presentation,
            reason,
        });
    }

    pub(super) fn drain(&mut self) -> Option<(Vec<ContactDiagnostic>, u64)> {
        if self.samples.is_empty() {
            return None;
        }
        let samples = self.samples.drain(..).collect();
        Some((samples, std::mem::take(&mut self.suppressed)))
    }
}
