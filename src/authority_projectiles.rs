#![allow(dead_code)]

#[path = "authority_projectiles/contact_diagnostics.rs"]
mod contact_diagnostics;
#[allow(unused_imports)]
pub(crate) use contact_diagnostics::{
    ContactDiagnostic, ContactDiagnosticReason, MAX_CONTACT_DIAGNOSTIC_SAMPLES,
};

#[path = "authority_projectiles/registry.rs"]
mod registry;
#[path = "authority_projectiles/shrapnel.rs"]
mod shrapnel;
#[path = "authority_projectiles/types.rs"]
mod types;

#[allow(unused_imports)]
pub(crate) use registry::{
    CollisionTargetWitness, MAX_PENDING_HIT_RECORD_BYTES, MAX_PENDING_HIT_RETRIES_PER_TICK,
    MAX_PENDING_HITS_PER_OWNER, MAX_PENDING_HITS_TOTAL, PresentationGeometryMetrics,
    ProjectileRegistry, ProjectileWallExplosion, evaluate_presentation_geometry,
    resolve_collision_target_witness, select_collision_target_witness,
};
#[allow(unused_imports)]
pub(crate) use types::{
    MotionHistorySource, PlayerHullSnapshot, ProjectileCollisionWitness, ProjectileContactReason,
    ProjectileHit, ProjectileRecord, RepelMutationSummary, RepelledProjectileState,
};

pub(crate) use shrapnel::{ExplosionFragmentTuning, build_shrapnel_records};

#[cfg(test)]
#[path = "authority_projectiles/tests.rs"]
mod tests;
