//! Server-emitted authoritative damage events.
//!
//! Each `ProjectileHit` returned by the per-tick projectile step
//! becomes an authoritative damage application here. The server
//! computes damage from arena settings, reduces the victim's
//! `server_energy`, queues one snapshot `WatchDamage` event, and runs
//! the server-side kill pipeline for lethal hits.
//!
//! Client-reported damage is not accepted. Production authority flows
//! through this module from server-simulated projectile hits.

use crate::arena_settings::{
    arena_bounty_increase_for_kill, arena_death_prize_lifetime_ticks,
    arena_emp_bomb_shutdown_ticks, arena_exact_damage, arena_flagger_damage_multiplier,
    arena_prize_weight_total, arena_prize_weights, arena_respawn_delay, arena_respawn_delay_ticks,
    arena_ship_bomb_bounce_count, arena_ship_damage_factor, arena_ship_emp_bomb,
    arena_ship_shield_lifetime_ticks, arena_weapon_damage_settings,
};
use crate::authority::EnergyPool;
use crate::authority_damage::{
    authoritative_unprotected_direct_damage, authoritative_unprotected_shrapnel_damage,
    inexact_direct_damage, maximum_reported_weapon_damage,
};
use crate::authority_projectiles::ProjectileHit;
use crate::continuum_weapon::{
    continuum_weapon_explosion_level, continuum_weapon_level, continuum_weapon_type,
    continuum_weapon_uses_bullet_exactness,
};
use crate::hidden_prizes::create_death_collectible;
use crate::identity::persist_stats_for_client;
use crate::objectives::{
    ObjectiveLifecycleReason, objective_player, player_lifecycle_packets_for_context,
    remove_player_turret_link_packets,
};
use crate::player_events::roll_death_green_prize_id;
use crate::reliable::{
    send_reliable_packet_to_sessions_for_runtime, send_reliable_packets_to_session_identities,
};
use crate::roster::score_update;
use crate::state::ServerState;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

// Per-event lines log under this module; see `log_targets`.
const _: () = assert!(crate::log_targets::is_child_of(
    crate::log_targets::SERVER_DAMAGE_COMBAT,
    module_path!()
));

/// Phase 2g: PLAYER_DEATH packet type (matches `combat::PLAYER_DEATH`).
const PLAYER_DEATH_PACKET: u8 = 0x06;

/// Per-event record of one server-applied damage delta. Returned to
/// the caller (today: `run_server_tick`) for structured logging and
/// delivery over snapshot `WatchDamage` events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerComputedDamage {
    pub(crate) damage_seq: u32,
    pub(crate) projectile_id: u32,
    pub(crate) server_tick: u32,
    pub(crate) victim_id: u16,
    pub(crate) shooter_id: u16,
    pub(crate) victim_life_seq: u32,
    pub(crate) shooter_life_seq: u32,
    pub(crate) weapon_data: u16,
    pub(crate) damage: u16,
    pub(crate) unprotected_damage: u16,
    pub(crate) shield_until_tick: Option<u32>,
    pub(crate) shield_remaining_ticks: u32,
    pub(crate) shield_lifetime_ticks: u32,
    pub(crate) emp_until_tick: Option<u32>,
    pub(crate) energy_before: u16,
    pub(crate) energy_after: u16,
    pub(crate) lethal: bool,
}

fn server_damage_event_json_line(event: &ServerComputedDamage, kind: &str) -> String {
    format!(
        "zubspace_event {}",
        server_damage_event_json_value(event, kind)
    )
}

fn server_damage_event_json_value(event: &ServerComputedDamage, kind: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "server_damage_applied",
        "damage_seq": event.damage_seq,
        "projectile_id": event.projectile_id,
        "server_tick": event.server_tick,
        "victim_id": event.victim_id,
        "shooter_id": event.shooter_id,
        "victim_life_seq": event.victim_life_seq,
        "shooter_life_seq": event.shooter_life_seq,
        "weapon": event.weapon_data,
        "damage": event.damage,
        "unprotected_damage": event.unprotected_damage,
        "shield_until_tick": event.shield_until_tick.unwrap_or(0),
        "shield_remaining_ticks": event.shield_remaining_ticks,
        "shield_lifetime_ticks": event.shield_lifetime_ticks,
        "emp_until_tick": event.emp_until_tick.unwrap_or(0),
        "energy_before": event.energy_before,
        "energy_after": event.energy_after,
        "lethal": event.lethal,
        "kind": kind,
    })
}

fn server_player_death_event_json_line(
    event: &ServerComputedDamage,
    victim_next_life_seq: u32,
    killed_bounty: u16,
    flag_transfer: u16,
    green_id: i8,
    green_collectible_id: u32,
    respawn_delay_ms: u128,
) -> String {
    format!(
        "zubspace_event {{\"type\":\"server_player_death\",\"damage_seq\":{},\"projectile_id\":{},\"server_tick\":{},\"killer_id\":{},\"victim_id\":{},\"victim_life_seq\":{},\"victim_next_life_seq\":{},\"killer_life_seq\":{},\"weapon\":{},\"energy_before\":{},\"energy_after\":{},\"killed_bounty\":{},\"flag_transfer\":{},\"green_id\":{},\"green_collectible_id\":{},\"respawn_delay_ms\":{}}}",
        event.damage_seq,
        event.projectile_id,
        event.server_tick,
        event.shooter_id,
        event.victim_id,
        event.victim_life_seq,
        victim_next_life_seq,
        event.shooter_life_seq,
        event.weapon_data,
        event.energy_before,
        event.energy_after,
        killed_bounty,
        flag_transfer,
        green_id,
        green_collectible_id,
        respawn_delay_ms,
    )
}

/// Build a `PLAYER_DEATH`-shaped packet (matches
/// `combat::player_death`). Carries the victim's bounty at time of
/// death, the flag-transfer count to be added to the killer's
/// inventory, and the optional death-green prize id rolled from
/// the arena's weighted prize table.
fn build_player_death_packet(
    killer_id: u16,
    victim_id: u16,
    killed_bounty: u16,
    flag_transfer: u16,
    green_id: i8,
    green_collectible_id: u32,
    life_seq: u32,
    respawn_at_tick: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(22);
    payload.push(PLAYER_DEATH_PACKET);
    payload.push(green_id as u8);
    payload.extend(killer_id.to_le_bytes());
    payload.extend(victim_id.to_le_bytes());
    payload.extend(killed_bounty.to_le_bytes());
    payload.extend(flag_transfer.to_le_bytes());
    payload.extend(green_collectible_id.to_le_bytes());
    payload.extend(life_seq.to_le_bytes());
    payload.extend(respawn_at_tick.to_le_bytes());
    payload
}

/// Phase 2g: server-detected hit → authoritative damage + event queue.
///
/// On every detected hit:
/// - Damage is computed from arena settings and applied to the
///   primary victim's `server_energy` (Phase 2f core).
/// - A snapshot `WatchDamage` event is queued so clients see the
///   energy delta in their HUDs exactly once.
///
/// **Splash damage (Phase 2g)**: when the weapon is a bomb-type
/// (3, 4, 8), the server scans every alive client within the
/// bomb's explosion radius (`BombExplodePixels * level`). Bystanders
/// inside that radius take falloff damage scaled by
/// `(explode_pixels - distance) * (full_dmg / explode_pixels)`.
/// Each splash victim gets their own snapshot `WatchDamage` event, and a
/// lethal splash kill runs the full death pipeline. Teammates and
/// the primary victim are skipped to avoid double-counting; the
/// shooter is hit by their own bomb only when the safety latch
/// would normally allow it (bomb has cleared the shooter's hull).
///
/// On lethal hits (primary or splash), the full death pipeline
/// applies the same Continuum death bookkeeping: bounty + flag
/// transfer, score updates, persisted stats, turret detach, powerball
/// drop, death-green prize roll.
///
/// Recent valid damagers receive assist credit through the same pipeline.
#[cfg(test)]
pub(crate) fn apply_server_detected_hit_with_broadcast(
    state: &Arc<Mutex<ServerState>>,
    hit: &ProjectileHit,
) -> Option<ServerComputedDamage> {
    apply_server_detected_hit_with_delivery_hook(state, hit, || {})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetectedHitApplication {
    Applied,
    Invalid,
    DeferredCapacity,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedDamageTransaction {
    root_hit: ProjectileHit,
    shooter_runtime: crate::authority_identity::PlayerRuntimeIdentity,
    collision_witness: Option<crate::authority_projectiles::ProjectileCollisionWitness>,
    hits: Vec<PreparedDamageHit>,
    bot_hits: Vec<PreparedBotDamageHit>,
    explosion: Option<crate::server_explosions::PreparedProjectileExplosion>,
}

/// Complete immutable per-victim geometry derived from the projectile-step
/// collision witness. Production primary and splash entries both name the
/// selected motion/presentation segment and sample that own admission and
/// falloff; synthetic shapes exist only in test-only prepared transactions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CollisionDamageWitness {
    pub(crate) collision: crate::authority_projectiles::ProjectileCollisionWitness,
    pub(crate) target: crate::authority_identity::LifeIdentity,
    pub(crate) primary: bool,
    pub(crate) target_motion_segment: Option<crate::combat_timeline::MotionSegmentIdentity>,
    pub(crate) requested_target_tick: u32,
    pub(crate) selected_target_tick: Option<u32>,
    pub(crate) history_source: crate::authority_projectiles::MotionHistorySource,
    pub(crate) target_presentation_geometry:
        crate::combat_timeline::ProjectilePresentationWitnessV1,
    pub(crate) target_current_x: f32,
    pub(crate) target_current_y: f32,
    pub(crate) damage_anchor_x: f32,
    pub(crate) damage_anchor_y: f32,
    pub(crate) damage_center_distance: f32,
    pub(crate) unprotected_damage: u16,
    pub(crate) adjusted_damage: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedDamagePrizeLoss {
    prize_id: i16,
    replacement_prize_id: Option<i16>,
}

#[derive(Clone, Debug)]
struct PreparedDamageHit {
    order: usize,
    hit: ProjectileHit,
    victim_runtime: crate::authority_identity::PlayerRuntimeIdentity,
    geometry: FrozenDamageGeometry,
    recipients: Vec<crate::authority_identity::PlayerSessionIdentity>,
    shooter_ship: Option<u8>,
    damage: u16,
    unprotected_damage: u16,
    emp_shutdown_ticks: u32,
    shield_until_tick: Option<u32>,
    shield_remaining_ticks: u32,
    shield_lifetime_ticks: u32,
}

#[derive(Clone, Debug)]
struct PreparedBotDamageHit {
    order: usize,
    hit: ProjectileHit,
    victim_runtime: crate::authority_identity::PlayerRuntimeIdentity,
    geometry: FrozenDamageGeometry,
    recipients: Vec<crate::authority_identity::PlayerSessionIdentity>,
    shooter_ship: Option<u8>,
    damage: u16,
    unprotected_damage: u16,
    emp_shutdown_ticks: u32,
    shield_until_tick: Option<u32>,
    shield_remaining_ticks: u32,
    shield_lifetime_ticks: u32,
}

#[derive(Clone, Copy, Debug)]
struct PreviewDamageHit {
    hit: ProjectileHit,
    geometry: FrozenDamageGeometry,
    shooter_ship: Option<u8>,
    unprotected_damage: u16,
}

/// One immutable target-history selection owns splash admission, falloff,
/// shield/EMP scaling, death, score, AOI publication, and capacity retries.
/// Production values always name a verified motion segment and selected tick.
#[derive(Clone, Copy, Debug)]
struct FrozenDamageGeometry {
    target: crate::authority_identity::LifeIdentity,
    target_motion_segment: Option<crate::combat_timeline::MotionSegmentIdentity>,
    requested_target_tick: u32,
    selected_target_tick: Option<u32>,
    history_source: crate::authority_projectiles::MotionHistorySource,
    presentation_geometry: crate::combat_timeline::ProjectilePresentationWitnessV1,
    explosion_radius: Option<f32>,
    target_current_x: f32,
    target_current_y: f32,
    target_anchor_x: f32,
    target_anchor_y: f32,
}

impl FrozenDamageGeometry {
    #[cfg(test)]
    fn synthetic_fixture(hit: ProjectileHit, target_anchor_x: f32, target_anchor_y: f32) -> Self {
        Self {
            target: hit.victim_identity(),
            target_motion_segment: None,
            requested_target_tick: hit.tick,
            selected_target_tick: None,
            history_source:
                crate::authority_projectiles::MotionHistorySource::UnifiedHistoryUnavailable,
            presentation_geometry:
                crate::combat_timeline::ProjectilePresentationWitnessV1::not_evaluated(),
            explosion_radius: None,
            target_current_x: target_anchor_x,
            target_current_y: target_anchor_y,
            target_anchor_x,
            target_anchor_y,
        }
    }

    fn is_finite(self) -> bool {
        self.target_anchor_x.is_finite() && self.target_anchor_y.is_finite()
    }
}

fn damage_shooter_runtime(
    state: &ServerState,
    hit: &ProjectileHit,
) -> Option<crate::authority_identity::PlayerRuntimeIdentity> {
    state
        .active_actor_runtime_identity(hit.owner_id)
        .filter(|runtime| runtime.life == hit.owner_identity())
        .or_else(|| {
            state
                .retired_projectile_owners
                .iter()
                .find_map(|(&runtime, &_)| {
                    (runtime.life == hit.owner_identity()
                        && state.projectile_flight_is_authoritative(hit)
                        && state.actor_runtime_matches_damage_collision(runtime, hit.tick))
                    .then_some(runtime)
                })
        })
}

fn damage_shooter_context(
    state: &ServerState,
    hit: &ProjectileHit,
) -> Option<(
    crate::authority_identity::PlayerRuntimeIdentity,
    u16,
    u8,
    u8,
)> {
    let runtime = damage_shooter_runtime(state, hit)?;
    if let Some(source) = state
        .active_projectiles
        .projectile(hit.projectile_id)
        .and_then(|projectile| projectile.presentation_source)
        .filter(|source| {
            source.source_player_id == runtime.player_id()
                && source.source_session_id == runtime.session.session_id.get()
                && source.source_life_seq == runtime.life.life_seq
        })
    {
        let rewind = state
            .clients
            .get(&hit.owner_id)
            .filter(|client| client.life_seq == runtime.life.life_seq)
            .map_or(0, |client| client.lag_compensation_ticks);
        return Some((runtime, source.frequency, rewind, source.ship));
    }
    if let Some(shooter) = state.clients.get(&hit.owner_id).filter(|client| {
        client.joined && client.session_id == runtime.session.session_id && client.player.ship < 8
    }) {
        return Some((
            runtime,
            shooter.player.frequency,
            shooter.lag_compensation_ticks,
            shooter.player.ship,
        ));
    }
    let shooter = state
        .bot(hit.owner_id)
        .filter(|bot| bot.session_id == runtime.session.session_id && bot.player.ship < 8)?;
    Some((runtime, shooter.player.frequency, 0, shooter.player.ship))
}

/// Mutable authority outcome staged immediately before one FIFO transaction
/// reserves delivery. Collision geometry, damage, identities, and recipients
/// remain frozen in `PreparedDamageHit`; energy/lethality intentionally derive
/// from the exact predecessor produced by earlier committed transactions.
#[derive(Clone, Debug)]
struct StagedDamageHit {
    prepared: PreparedDamageHit,
    energy_before: u16,
    energy_remainder_before: f32,
    #[allow(dead_code)] // retained for multi-victim transaction proofs
    energy_after: u16,
    energy_remainder_after: f32,
    lethal: bool,
    self_bomb_survival: bool,
    damage_prize_loss: Option<PreparedDamagePrizeLoss>,
}

#[derive(Clone, Debug)]
struct StagedBotDamageHit {
    prepared: PreparedBotDamageHit,
    energy_before: u16,
    energy_remainder_before: f32,
    energy_after: u16,
    energy_remainder_after: f32,
    lethal: bool,
    self_bomb_survival: bool,
}

impl PreparedDamageTransaction {
    pub(crate) fn collision_damage_witnesses(&self) -> Vec<CollisionDamageWitness> {
        let Some(collision) = self.collision_witness else {
            return Vec::new();
        };
        let mut targets = self
            .hits
            .iter()
            .map(|prepared| {
                (
                    prepared.order,
                    prepared.hit.victim_identity(),
                    prepared.geometry,
                    prepared.unprotected_damage,
                    prepared.damage,
                )
            })
            .chain(self.bot_hits.iter().map(|prepared| {
                (
                    prepared.order,
                    prepared.hit.victim_identity(),
                    prepared.geometry,
                    prepared.unprotected_damage,
                    prepared.damage,
                )
            }))
            .collect::<Vec<_>>();
        targets.sort_unstable_by_key(|target| target.0);
        targets
            .into_iter()
            .map(
                |(order, target, geometry, unprotected_damage, adjusted_damage)| {
                    // Damage falloff is frozen from the integer protocol impact
                    // point carried by the root hit. The collision witness keeps
                    // its subpixel contact point independently so diagnostics do
                    // not conflate the two coordinate domains.
                    let dx = geometry.target_anchor_x - f32::from(self.root_hit.x);
                    let dy = geometry.target_anchor_y - f32::from(self.root_hit.y);
                    CollisionDamageWitness {
                        collision,
                        target,
                        primary: order == 0,
                        target_motion_segment: geometry.target_motion_segment,
                        requested_target_tick: geometry.requested_target_tick,
                        selected_target_tick: geometry.selected_target_tick,
                        history_source: geometry.history_source,
                        target_presentation_geometry: geometry.presentation_geometry,
                        target_current_x: geometry.target_current_x,
                        target_current_y: geometry.target_current_y,
                        damage_anchor_x: geometry.target_anchor_x,
                        damage_anchor_y: geometry.target_anchor_y,
                        damage_center_distance: (dx * dx + dy * dy).sqrt(),
                        unprotected_damage,
                        adjusted_damage,
                    }
                },
            )
            .collect()
    }

    pub(crate) fn involves_player(&self, player_id: u16) -> bool {
        self.shooter_runtime.player_id() == player_id
            || self
                .hits
                .iter()
                .any(|prepared| prepared.victim_runtime.player_id() == player_id)
            || self
                .bot_hits
                .iter()
                .any(|prepared| prepared.victim_runtime.player_id() == player_id)
    }

    pub(crate) fn shooter_runtime(&self) -> crate::authority_identity::PlayerRuntimeIdentity {
        self.shooter_runtime
    }

    pub(crate) fn projectile_id(&self) -> u32 {
        self.root_hit.projectile_id
    }

    pub(crate) fn human_victim_runtimes(
        &self,
    ) -> impl Iterator<Item = crate::authority_identity::PlayerRuntimeIdentity> + '_ {
        self.hits.iter().map(|prepared| prepared.victim_runtime)
    }

    fn matches_root(&self, hit: &ProjectileHit) -> bool {
        self.root_hit == *hit
    }

    pub(crate) fn identities_are_current(&self, state: &ServerState) -> bool {
        state.actor_runtime_matches_damage_collision(self.shooter_runtime, self.root_hit.tick)
            && self
                .hits
                .iter()
                .all(|prepared| state.player_runtime_matches(prepared.victim_runtime))
            && self
                .bot_hits
                .iter()
                .all(|prepared| state.actor_runtime_matches(prepared.victim_runtime))
    }

    pub(crate) fn survives_same_tick_shooter_death(
        &self,
        runtime: crate::authority_identity::PlayerRuntimeIdentity,
        collision_tick: u32,
    ) -> bool {
        self.shooter_runtime == runtime && self.root_hit.tick == collision_tick
    }

    /// Heap retained by the immutable collision outcome itself. The map node
    /// is deliberately excluded; `/v1/status` reports transaction count beside
    /// this payload allocation so both bounds remain visible.
    pub(crate) fn retained_payload_bytes(&self) -> usize {
        self.hits
            .capacity()
            .saturating_mul(std::mem::size_of::<PreparedDamageHit>())
            .saturating_add(self.hits.iter().fold(0usize, |total, prepared| {
                total.saturating_add(prepared.recipients.capacity().saturating_mul(
                    std::mem::size_of::<crate::authority_identity::PlayerSessionIdentity>(),
                ))
            }))
            .saturating_add(
                self.bot_hits
                    .capacity()
                    .saturating_mul(std::mem::size_of::<PreparedBotDamageHit>()),
            )
            .saturating_add(self.bot_hits.iter().fold(0usize, |total, prepared| {
                total.saturating_add(prepared.recipients.capacity().saturating_mul(
                    std::mem::size_of::<crate::authority_identity::PlayerSessionIdentity>(),
                ))
            }))
    }
}

/// Freeze the complete damage transaction while the collision's position and
/// rewind history are still authoritative. This is called under the same
/// `ServerState` lock as projectile stepping; retries consume only these
/// captured targets, damage values, lifecycle identities, and audience sets.
#[cfg(test)]
pub(crate) fn prepare_server_detected_damage_transaction(
    state: &ServerState,
    hit: &ProjectileHit,
) -> Option<PreparedDamageTransaction> {
    let preview_hits = preview_detected_damage_hits(state, hit)?;
    prepare_server_detected_damage_transaction_from_previews(
        state,
        hit,
        state
            .active_projectiles
            .collision_witness(hit.projectile_id),
        preview_hits,
    )
}

/// Production collision boundary: the primary anchor is the immutable witness
/// selected by projectile contact, and every splash candidate consumes the
/// same per-tick hull snapshot plus the same projectile-owned presentation
/// resolver. Neither a client position cache nor current-position fallback
/// participates.
pub(crate) fn prepare_server_detected_damage_transaction_from_collision(
    state: &ServerState,
    hit: &ProjectileHit,
    hulls: &[crate::authority_projectiles::PlayerHullSnapshot],
    metrics: &mut crate::authority_projectiles::PresentationGeometryMetrics,
) -> Option<PreparedDamageTransaction> {
    let collision = state
        .active_projectiles
        .collision_witness(hit.projectile_id)?;
    let preview_hits =
        preview_detected_damage_hits_from_collision(state, hit, collision, hulls, metrics)?;
    prepare_server_detected_damage_transaction_from_previews(
        state,
        hit,
        Some(collision),
        preview_hits,
    )
}

/// Apply the arena's inexact-damage roll to a bullet/burst/shrapnel hit.
///
/// The reference game clears the deterministic path for these weapons when the arena's
/// `ExactDamage` flag is clear, re-rolling the already shield-reduced damage
/// as `sqrt((rand() * 1000) % (damage * damage + 1))`. Bombs never roll.
///
/// The value is seeded by the hit identity rather than a stream RNG so that
/// re-preparing or re-staging the same collision commits the same damage.
fn bullet_exactness_damage(hit: &ProjectileHit, damage: u16, exact_damage: bool) -> u16 {
    if exact_damage || !continuum_weapon_uses_bullet_exactness(hit.weapon_data) {
        return damage;
    }
    inexact_direct_damage(damage, exactness_roll_seed(hit))
}

fn exactness_roll_seed(hit: &ProjectileHit) -> u32 {
    hit.projectile_id
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add(u32::from(hit.victim_id) << 16)
        .wrapping_add(hit.victim_life_seq)
        .wrapping_add(hit.tick)
}

fn prepare_server_detected_damage_transaction_from_previews(
    state: &ServerState,
    hit: &ProjectileHit,
    collision_witness: Option<crate::authority_projectiles::ProjectileCollisionWitness>,
    mut preview_hits: Vec<PreviewDamageHit>,
) -> Option<PreparedDamageTransaction> {
    let shooter_runtime = damage_shooter_runtime(state, hit)?;
    if preview_hits.len() > 1 {
        preview_hits[1..].sort_unstable_by_key(|candidate| candidate.hit.victim_id);
    }
    let mut hits = Vec::new();
    let mut bot_hits = Vec::new();
    hits.try_reserve(preview_hits.len()).ok()?;
    bot_hits.try_reserve(preview_hits.len()).ok()?;
    for (order, preview) in preview_hits.into_iter().enumerate() {
        let preview_hit = preview.hit;
        let mut recipients = crate::snapshot::watch_damage_recipient_identities(
            state,
            preview_hit.victim_id,
            &[preview_hit.owner_id],
        )
        .into_iter()
        .map(|runtime| runtime.session)
        .collect::<Vec<_>>();
        recipients.sort_unstable();
        recipients.dedup();
        if let Some(victim) = state.clients.get(&preview_hit.victim_id) {
            let victim_runtime = state.active_player_runtime_identity(preview_hit.victim_id)?;
            if victim_runtime.life != preview_hit.victim_identity() {
                return None;
            }
            let shield =
                authoritative_shield_damage(victim, preview_hit.tick, preview.unprotected_damage);
            // The reference game re-rolls inexact bullet/burst/shrapnel damage after
            // timed protection and *before* the flagger multiplier. The
            // reservation preview deliberately keeps the pre-shield maximum;
            // this is the value the committed transaction will apply.
            let damage = bullet_exactness_damage(&preview_hit, shield.damage, arena_exact_damage());
            // FlaggerDamagePercent: the client applies the flagger multiplier
            // last, after timed protection, so the server scales the post-shield
            // damage the same way. Objective carriers are human players; the bot
            // branch below has no carrier state to consult.
            let damage = flagger_damage_after_carry(
                damage,
                arena_flagger_damage_multiplier(),
                victim_carries_objective(state, victim_runtime),
            );
            let emp_shutdown_ticks = emp_shutdown_ticks_for_damage_radius(
                &preview_hit,
                preview.shooter_ship,
                preview.unprotected_damage,
                preview.geometry.explosion_radius,
            );
            hits.push(PreparedDamageHit {
                order,
                hit: preview_hit,
                victim_runtime,
                geometry: preview.geometry,
                recipients,
                shooter_ship: preview.shooter_ship,
                damage,
                unprotected_damage: preview.unprotected_damage,
                emp_shutdown_ticks,
                shield_until_tick: shield.until_tick,
                shield_remaining_ticks: shield.remaining_ticks,
                shield_lifetime_ticks: shield.lifetime_ticks,
            });
        } else {
            let victim = state
                .bot(preview_hit.victim_id)
                .filter(|bot| bot.alive() && bot.life_seq == preview_hit.victim_life_seq)?;
            let victim_runtime = victim.runtime_identity();
            let shield = authoritative_bot_shield_damage(
                victim,
                preview_hit.tick,
                preview.unprotected_damage,
            );
            let emp_shutdown_ticks = emp_shutdown_ticks_for_damage_radius(
                &preview_hit,
                preview.shooter_ship,
                preview.unprotected_damage,
                preview.geometry.explosion_radius,
            );
            // Bots get the same inexact roll as human victims (no flagger
            // multiplier applies to them).
            let damage = bullet_exactness_damage(&preview_hit, shield.damage, arena_exact_damage());
            bot_hits.push(PreparedBotDamageHit {
                order,
                hit: preview_hit,
                victim_runtime,
                geometry: preview.geometry,
                recipients,
                shooter_ship: preview.shooter_ship,
                damage,
                unprotected_damage: preview.unprotected_damage,
                emp_shutdown_ticks,
                shield_until_tick: shield.until_tick,
                shield_remaining_ticks: shield.remaining_ticks,
                shield_lifetime_ticks: shield.lifetime_ticks,
            });
        }
    }
    Some(PreparedDamageTransaction {
        root_hit: *hit,
        shooter_runtime,
        collision_witness,
        hits,
        bot_hits,
        explosion: crate::server_explosions::prepare_collision_explosion(state, hit),
    })
}

fn stage_prepared_damage_transaction(
    state: &ServerState,
    transaction: &PreparedDamageTransaction,
) -> Option<(Vec<StagedDamageHit>, Vec<StagedBotDamageHit>, i32)> {
    let mut shadow = std::collections::BTreeMap::<
        crate::authority_identity::PlayerRuntimeIdentity,
        crate::state::ClientHandle,
    >::new();
    let mut lethal_victims = std::collections::BTreeSet::new();
    let mut staged = Vec::new();
    let mut staged_bots = Vec::new();
    let mut damage_rng_seed = state.damage_prize_rng_seed;
    staged.try_reserve(transaction.hits.len()).ok()?;
    for prepared in &transaction.hits {
        if !prepared.geometry.is_finite()
            || prepared.geometry.target != prepared.hit.victim_identity()
        {
            return None;
        }
        if lethal_victims.contains(&prepared.victim_runtime) {
            // A single transaction should currently contain each splash
            // target once. Keep the authority rule explicit if a future
            // weapon shape repeats one victim: the lethal fact owns the life
            // and later same-life damage is inert.
            continue;
        }
        let victim = state.clients.get(&prepared.hit.victim_id)?;
        if state.active_player_runtime_identity(prepared.hit.victim_id)
            != Some(prepared.victim_runtime)
            || !victim.joined
            || !victim.alive
            || victim.player.ship >= 8
        {
            return None;
        }
        let mut projected = shadow
            .get(&prepared.victim_runtime)
            .cloned()
            .unwrap_or_else(|| victim.clone());
        let energy_before = projected.server_energy;
        let energy_remainder_before = projected.server_energy_remainder;
        let raw_lethal = energy_before < prepared.damage;
        let self_bomb_survival = prepared.hit.owner_id == prepared.hit.victim_id
            && matches!(continuum_weapon_type(prepared.hit.weapon_data), 3 | 4 | 8)
            && raw_lethal;
        let lethal = raw_lethal && !self_bomb_survival;
        let damage_factor = arena_ship_damage_factor(victim.player.ship);
        let damage_prize_loss = if !lethal
            && !self_bomb_survival
            && prepared.damage > 0
            && projected.server_bounty > 0
            && energy_before < projected.server_max_energy
            && damage_factor != 0
        {
            if damage_rng_seed == 0 {
                damage_rng_seed = initial_damage_prize_rng_seed();
            }
            prepare_damage_prize_loss_selection(
                &mut damage_rng_seed,
                damage_factor,
                prepared.damage,
            )
        } else {
            None
        };
        // The reference game performs the selected ApplyPrize mutation before the
        // nonlethal subtraction. Equality is nonlethal, so None→FullCharge
        // at energy==damage ends at max_energy-damage.
        if let Some(loss) = damage_prize_loss {
            apply_prepared_damage_prize_mutation(&mut projected, prepared.hit.tick, loss);
        }
        if self_bomb_survival {
            projected.server_energy = 1;
            projected.server_energy_remainder = 0.0;
        } else {
            let mut pool = EnergyPool::new(
                projected.server_energy,
                projected.server_max_energy,
                projected.server_energy_remainder,
            );
            pool.apply_damage(prepared.damage);
            projected.server_energy = pool.current();
            projected.server_energy_remainder = pool.recharge_remainder();
        }
        let energy_after = projected.server_energy;
        let energy_remainder_after = projected.server_energy_remainder;
        shadow.insert(prepared.victim_runtime, projected);
        if lethal {
            lethal_victims.insert(prepared.victim_runtime);
        }
        staged.push(StagedDamageHit {
            prepared: prepared.clone(),
            energy_before,
            energy_remainder_before,
            energy_after,
            energy_remainder_after,
            lethal,
            self_bomb_survival,
            damage_prize_loss,
        });
    }
    let mut bot_shadow = std::collections::BTreeMap::<
        crate::authority_identity::PlayerRuntimeIdentity,
        crate::state::ServerBot,
    >::new();
    staged_bots.try_reserve(transaction.bot_hits.len()).ok()?;
    for prepared in &transaction.bot_hits {
        if !prepared.geometry.is_finite()
            || prepared.geometry.target != prepared.hit.victim_identity()
        {
            return None;
        }
        let victim = state.bot(prepared.hit.victim_id)?;
        if victim.runtime_identity() != prepared.victim_runtime || !victim.alive() {
            return None;
        }
        let mut projected = bot_shadow
            .get(&prepared.victim_runtime)
            .cloned()
            .unwrap_or_else(|| victim.clone());
        let energy_before = projected.energy;
        let energy_remainder_before = projected.energy_remainder;
        let raw_lethal = energy_before < prepared.damage;
        let self_bomb_survival = prepared.hit.owner_id == prepared.hit.victim_id
            && matches!(continuum_weapon_type(prepared.hit.weapon_data), 3 | 4 | 8)
            && raw_lethal;
        let lethal = raw_lethal && !self_bomb_survival;
        if self_bomb_survival {
            projected.energy = 1;
            projected.energy_remainder = 0.0;
        } else {
            let mut pool = EnergyPool::new(
                projected.energy,
                projected.max_energy,
                projected.energy_remainder,
            );
            pool.apply_damage(prepared.damage);
            projected.energy = pool.current();
            projected.energy_remainder = pool.recharge_remainder();
        }
        let energy_after = projected.energy;
        let energy_remainder_after = projected.energy_remainder;
        bot_shadow.insert(prepared.victim_runtime, projected);
        staged_bots.push(StagedBotDamageHit {
            prepared: prepared.clone(),
            energy_before,
            energy_remainder_before,
            energy_after,
            energy_remainder_after,
            lethal,
            self_bomb_survival,
        });
    }
    (!staged.is_empty() || !staged_bots.is_empty()).then_some((
        staged,
        staged_bots,
        damage_rng_seed,
    ))
}

fn staged_predecessors_are_current(
    state: &ServerState,
    staged: &[StagedDamageHit],
    staged_bots: &[StagedBotDamageHit],
) -> bool {
    let mut checked = std::collections::BTreeSet::new();
    staged.iter().all(|hit| {
        if !checked.insert(hit.prepared.victim_runtime) {
            return true;
        }
        state
            .clients
            .get(&hit.prepared.hit.victim_id)
            .is_some_and(|victim| {
                state.active_player_runtime_identity(hit.prepared.hit.victim_id)
                    == Some(hit.prepared.victim_runtime)
                    && victim.server_energy == hit.energy_before
                    && victim.server_energy_remainder.to_bits()
                        == hit.energy_remainder_before.to_bits()
            })
    }) && staged_bots.iter().all(|hit| {
        state.bot(hit.prepared.hit.victim_id).is_some_and(|bot| {
            bot.runtime_identity() == hit.prepared.victim_runtime
                && bot.energy == hit.energy_before
                && bot.energy_remainder.to_bits() == hit.energy_remainder_before.to_bits()
        })
    })
}

/// Production projectile-tick boundary. Capacity pressure is distinct from a
/// stale/invalid collision so the registry can retain and retry the exact hit.
pub(crate) fn try_apply_server_detected_hit_with_broadcast(
    state: &Arc<Mutex<ServerState>>,
    hit: &ProjectileHit,
) -> DetectedHitApplication {
    apply_server_detected_hit_with_delivery_hook_outcome(state, hit, || {}).0
}

/// Compute the exact upper bound of damage facts this hit can commit without
/// mutating energy, lifecycle, scores, projectile state, or event queues.
/// Reservation uses this before the first authoritative write; the later
/// damage pass may produce fewer facts (for example after falloff rounding)
/// but can never exceed it.
#[cfg(test)]
fn preview_detected_damage_hits(
    state: &ServerState,
    hit: &ProjectileHit,
) -> Option<Vec<PreviewDamageHit>> {
    let (_, shooter_frequency, lag_compensation_ticks, _) = damage_shooter_context(state, hit)?;
    let rewind_target_tick =
        crate::continuum::continuum_tick_sub(hit.tick, lag_compensation_ticks as u32);
    // A production registry hit always has a hull snapshot. Keep the direct
    // helper path total for legacy tests by treating an otherwise positionless
    // client primary as co-located; splash candidates never receive this
    // fallback. Server bots retain their exact authoritative position.
    let (primary_x, primary_y, primary_is_bot) =
        if let Some(primary) = state.clients.get(&hit.victim_id) {
            let (x, y) = rewound_client_position(primary, rewind_target_tick)
                .unwrap_or((hit.x as f32, hit.y as f32));
            (x, y, false)
        } else {
            let primary = state
                .bot(hit.victim_id)
                .filter(|bot| bot.alive() && bot.life_seq == hit.victim_life_seq)?;
            (primary.x, primary.y, true)
        };
    let (shooter_ship, unprotected_damage) = if primary_is_bot {
        preview_server_detected_bot_hit_at_anchor(state, hit, (primary_x, primary_y))?
    } else {
        preview_server_detected_hit_at_anchor(state, hit, (primary_x, primary_y))?
    };
    let mut hits = vec![PreviewDamageHit {
        hit: *hit,
        geometry: FrozenDamageGeometry::synthetic_fixture(*hit, primary_x, primary_y),
        shooter_ship,
        unprotected_damage,
    }];
    if !matches!(continuum_weapon_type(hit.weapon_data), 3 | 4 | 8) {
        return Some(hits);
    }

    let level = continuum_weapon_level(hit.weapon_data);
    let radius = bomb_explosion_radius_pixels(level);
    let radius_sq = radius * radius;
    for (&player_id, client) in &state.clients {
        if player_id == hit.victim_id
            || !client.alive
            || client.player.ship >= 8
            || (client.player.frequency == shooter_frequency && player_id != hit.owner_id)
        {
            continue;
        }
        let Some((x, y)) = rewound_client_position(client, rewind_target_tick) else {
            continue;
        };
        let dx = hit.x as f32 - x;
        let dy = hit.y as f32 - y;
        if dx * dx + dy * dy > radius_sq {
            continue;
        }
        let splash = ProjectileHit {
            projectile_id: hit.projectile_id,
            owner_id: hit.owner_id,
            owner_life_seq: hit.owner_life_seq,
            victim_id: player_id,
            victim_life_seq: client.life_seq,
            weapon_data: hit.weapon_data,
            x: hit.x,
            y: hit.y,
            tick: hit.tick,
        };
        if let Some((shooter_ship, unprotected_damage)) =
            preview_server_detected_hit_at_anchor(state, &splash, (x, y))
        {
            hits.try_reserve(1).ok()?;
            hits.push(PreviewDamageHit {
                hit: splash,
                geometry: FrozenDamageGeometry::synthetic_fixture(splash, x, y),
                shooter_ship,
                unprotected_damage,
            });
        }
    }
    for bot in state.bots.iter().filter(|bot| bot.alive()) {
        let player_id = bot.player.id;
        if player_id == hit.victim_id
            || (bot.player.frequency == shooter_frequency && player_id != hit.owner_id)
        {
            continue;
        }
        let dx = hit.x as f32 - bot.x;
        let dy = hit.y as f32 - bot.y;
        if dx * dx + dy * dy > radius_sq {
            continue;
        }
        let splash = ProjectileHit {
            projectile_id: hit.projectile_id,
            owner_id: hit.owner_id,
            owner_life_seq: hit.owner_life_seq,
            victim_id: player_id,
            victim_life_seq: bot.life_seq,
            weapon_data: hit.weapon_data,
            x: hit.x,
            y: hit.y,
            tick: hit.tick,
        };
        if let Some((shooter_ship, unprotected_damage)) =
            preview_server_detected_bot_hit_at_anchor(state, &splash, (bot.x, bot.y))
        {
            hits.try_reserve(1).ok()?;
            hits.push(PreviewDamageHit {
                hit: splash,
                geometry: FrozenDamageGeometry {
                    target: splash.victim_identity(),
                    target_motion_segment: None,
                    requested_target_tick: splash.tick,
                    selected_target_tick: None,
                    history_source:
                        crate::authority_projectiles::MotionHistorySource::UnifiedHistoryUnavailable,
                    presentation_geometry:
                        crate::combat_timeline::ProjectilePresentationWitnessV1::not_evaluated(),
                    explosion_radius: None,
                    target_current_x: bot.x,
                    target_current_y: bot.y,
                    target_anchor_x: bot.x,
                    target_anchor_y: bot.y,
                },
                shooter_ship,
                unprotected_damage,
            });
        }
    }
    Some(hits)
}

fn preview_detected_damage_hits_from_collision(
    state: &ServerState,
    hit: &ProjectileHit,
    collision: crate::authority_projectiles::ProjectileCollisionWitness,
    hulls: &[crate::authority_projectiles::PlayerHullSnapshot],
    metrics: &mut crate::authority_projectiles::PresentationGeometryMetrics,
) -> Option<Vec<PreviewDamageHit>> {
    let collision_source_valid = matches!(
        (
            collision.history_source,
            collision.presentation_geometry.status,
        ),
        (
            crate::authority_projectiles::MotionHistorySource::UnifiedExactSegment,
            crate::combat_timeline::ProjectilePresentationStatusV1::ServerNative,
        ) | (
            crate::authority_projectiles::MotionHistorySource::ObserverPresentation,
            crate::combat_timeline::ProjectilePresentationStatusV1::Resolved(_),
        ) | (
            crate::authority_projectiles::MotionHistorySource::LocalSelf,
            crate::combat_timeline::ProjectilePresentationStatusV1::LocalSelf,
        )
    );
    if collision.projectile_id != hit.projectile_id
        || collision.owner != hit.owner_identity()
        || collision.target != hit.victim_identity()
        || collision.collision_tick != hit.tick
        || !collision_source_valid
        || collision.selected_target_tick.is_none()
        || collision.target_motion_segment.is_none()
        || !collision.target_anchor_x.is_finite()
        || !collision.target_anchor_y.is_finite()
    {
        return None;
    }
    let (_, shooter_frequency, _, _) = damage_shooter_context(state, hit)?;
    let primary = hulls
        .iter()
        .find(|hull| hull.player_id == hit.victim_id && hull.life_seq == hit.victim_life_seq)?;
    if primary.motion_segment != collision.target_motion_segment
        || primary.motion_source_kind.is_none()
    {
        return None;
    }
    let primary_anchor = (collision.target_anchor_x, collision.target_anchor_y);
    let primary_geometry = FrozenDamageGeometry {
        target: hit.victim_identity(),
        target_motion_segment: collision.target_motion_segment,
        requested_target_tick: collision.requested_target_tick,
        selected_target_tick: collision.selected_target_tick,
        history_source: collision.history_source,
        presentation_geometry: collision.presentation_geometry,
        explosion_radius: collision.explosion_radius,
        target_current_x: collision.target_current_x,
        target_current_y: collision.target_current_y,
        target_anchor_x: primary_anchor.0,
        target_anchor_y: primary_anchor.1,
    };
    let (shooter_ship, unprotected_damage) = if state.clients.contains_key(&primary.player_id) {
        preview_server_detected_hit_at_geometry(state, hit, primary_geometry)?
    } else {
        preview_server_detected_bot_hit_at_geometry(state, hit, primary_geometry)?
    };
    let mut hits = vec![PreviewDamageHit {
        hit: *hit,
        geometry: primary_geometry,
        shooter_ship,
        unprotected_damage,
    }];
    if !matches!(continuum_weapon_type(hit.weapon_data), 3 | 4 | 8) {
        return Some(hits);
    }

    let radius = collision.explosion_radius?;
    let radius_sq = radius * radius;
    let projectile = state.active_projectiles.projectile(hit.projectile_id);
    for hull in hulls {
        if hull.player_id == hit.victim_id
            || (hull.frequency == shooter_frequency && hull.player_id != hit.owner_id)
        {
            continue;
        }
        let presentation = if collision.history_source
            == crate::authority_projectiles::MotionHistorySource::UnifiedExactSegment
        {
            crate::combat_timeline::ProjectilePresentationWitnessV1::server_native()
        } else {
            crate::authority_projectiles::evaluate_presentation_geometry(
                projectile?,
                hull,
                collision.collision_tick,
                metrics,
                &mut |projectile, hull, tick| {
                    super::server_tick::projectile_presentation_geometry(
                        &state.clients,
                        projectile,
                        hull,
                        tick,
                    )
                },
            )
        };
        let target = crate::authority_projectiles::select_collision_target_witness(
            hull,
            crate::authority_projectiles::resolve_collision_target_witness(
                hull,
                collision.requested_target_tick,
            ),
            presentation,
        );
        if !target.collision_available
            || target.selected_tick.is_none()
            || hull.motion_segment.is_none()
        {
            continue;
        }
        let dx = f32::from(hit.x) - target.x;
        let dy = f32::from(hit.y) - target.y;
        if dx * dx + dy * dy > radius_sq {
            continue;
        }
        let splash = ProjectileHit {
            projectile_id: hit.projectile_id,
            owner_id: hit.owner_id,
            owner_life_seq: hit.owner_life_seq,
            victim_id: hull.player_id,
            victim_life_seq: hull.life_seq,
            weapon_data: hit.weapon_data,
            x: hit.x,
            y: hit.y,
            tick: hit.tick,
        };
        let geometry = FrozenDamageGeometry {
            target: splash.victim_identity(),
            target_motion_segment: hull.motion_segment,
            requested_target_tick: target.requested_tick,
            selected_target_tick: target.selected_tick,
            history_source: target.source,
            presentation_geometry: presentation,
            explosion_radius: collision.explosion_radius,
            target_current_x: hull.x,
            target_current_y: hull.y,
            target_anchor_x: target.x,
            target_anchor_y: target.y,
        };
        let preview = if state.clients.contains_key(&hull.player_id) {
            preview_server_detected_hit_at_geometry(state, &splash, geometry)
        } else {
            preview_server_detected_bot_hit_at_geometry(state, &splash, geometry)
        };
        if let Some((shooter_ship, unprotected_damage)) = preview {
            hits.try_reserve(1).ok()?;
            hits.push(PreviewDamageHit {
                hit: splash,
                geometry,
                shooter_ship,
                unprotected_damage,
            });
        }
    }
    Some(hits)
}

#[cfg(test)]
fn apply_server_detected_hit_with_delivery_hook(
    state: &Arc<Mutex<ServerState>>,
    hit: &ProjectileHit,
    after_application: impl FnOnce(),
) -> Option<ServerComputedDamage> {
    apply_server_detected_hit_with_delivery_hook_outcome(state, hit, after_application).1
}

fn apply_prepared_damage_hit(
    state: &mut ServerState,
    staged: &StagedDamageHit,
) -> Result<Option<ServerComputedDamage>, ()> {
    let prepared = &staged.prepared;
    let predecessor_matches = state
        .clients
        .get(&prepared.hit.victim_id)
        .is_some_and(|client| {
            state.active_player_runtime_identity(prepared.hit.victim_id)
                == Some(prepared.victim_runtime)
                && client.server_energy == staged.energy_before
                && client.server_energy_remainder.to_bits()
                    == staged.energy_remainder_before.to_bits()
        });
    if !predecessor_matches {
        return Err(());
    }
    let client = state.clients.get_mut(&prepared.hit.victim_id).ok_or(())?;
    let weapon_type = continuum_weapon_type(prepared.hit.weapon_data);
    let mut emp_until_tick = None;
    if prepared.hit.owner_id != prepared.hit.victim_id
        && matches!(weapon_type, 3 | 4)
        && prepared.unprotected_damage > 0
        && prepared.shooter_ship.is_some_and(arena_ship_emp_bomb)
    {
        let ticks = prepared.emp_shutdown_ticks;
        let until = crate::continuum::continuum_tick_add(prepared.hit.tick, ticks);
        if !client.server_emp_active(prepared.hit.tick) {
            client.server_emp_start_sequence = client
                .last_input_seq
                .map(|sequence| sequence.wrapping_add(1));
        }
        // The reference game assigns the new EMP timer. A later shorter bomb shortens
        // the source; prize Shutdown remains an independent union deadline.
        client.server_emp_until_tick = Some(until);
        emp_until_tick = Some(until);
    }
    if prepared.damage == 0 {
        // EMP is derived from raw pre-Shield bomb damage and applies first,
        // but a fully absorbed hit has no damage identity or assist fact.
        return Ok(None);
    }
    let damage_seq = state.allocate_damage_seq();
    let client = state.clients.get_mut(&prepared.hit.victim_id).ok_or(())?;
    if let Some(loss) = staged.damage_prize_loss {
        apply_prepared_damage_prize_loss(client, prepared.hit.tick, damage_seq, loss);
    }
    if staged.self_bomb_survival {
        client.server_energy = 1;
        client.server_energy_remainder = 0.0;
    } else {
        let mut pool = EnergyPool::new(
            client.server_energy,
            client.server_max_energy,
            client.server_energy_remainder,
        );
        pool.apply_damage(prepared.damage);
        client.server_energy = pool.current();
        client.server_energy_remainder = pool.recharge_remainder();
    }
    let energy_after = client.server_energy;
    debug_assert_eq!(energy_after, staged.energy_after);
    debug_assert_eq!(
        client.server_energy_remainder.to_bits(),
        staged.energy_remainder_after.to_bits()
    );
    advance_damage_recharge_anchor(client, prepared.hit.tick);
    client.last_reported_energy = energy_after;
    update_cached_energy_after_damage(client, energy_after);
    if prepared.hit.owner_id != prepared.hit.victim_id {
        client.recent_damagers.retain(|(_, at_tick)| {
            crate::continuum::continuum_tick_elapsed(prepared.hit.tick, *at_tick)
                <= crate::state::ASSIST_DAMAGE_WINDOW_TICKS
        });
        if client.recent_damagers.len() >= crate::state::RECENT_DAMAGERS_CAPACITY {
            client.recent_damagers.remove(0);
        }
        client
            .recent_damagers
            .push((prepared.hit.owner_identity(), prepared.hit.tick));
    }
    Ok(Some(ServerComputedDamage {
        damage_seq,
        projectile_id: prepared.hit.projectile_id,
        server_tick: prepared.hit.tick,
        victim_id: prepared.hit.victim_id,
        shooter_id: prepared.hit.owner_id,
        victim_life_seq: prepared.hit.victim_life_seq,
        shooter_life_seq: prepared.hit.owner_life_seq,
        weapon_data: prepared.hit.weapon_data,
        damage: prepared.damage,
        unprotected_damage: prepared.unprotected_damage,
        shield_until_tick: prepared.shield_until_tick,
        shield_remaining_ticks: prepared.shield_remaining_ticks,
        shield_lifetime_ticks: prepared.shield_lifetime_ticks,
        emp_until_tick,
        energy_before: staged.energy_before,
        energy_after,
        lethal: staged.lethal,
    }))
}

fn apply_prepared_bot_damage_hit(
    state: &mut ServerState,
    staged: &StagedBotDamageHit,
) -> Result<Option<ServerComputedDamage>, ()> {
    let prepared = &staged.prepared;
    let current_human_source = state
        .active_player_runtime_identity(prepared.hit.owner_id)
        .is_some_and(|runtime| runtime.life == prepared.hit.owner_identity());
    let predecessor_matches = state.bot(prepared.hit.victim_id).is_some_and(|bot| {
        bot.runtime_identity() == prepared.victim_runtime
            && bot.energy == staged.energy_before
            && bot.energy_remainder.to_bits() == staged.energy_remainder_before.to_bits()
    });
    if !predecessor_matches {
        return Err(());
    }
    let weapon_type = continuum_weapon_type(prepared.hit.weapon_data);
    let mut emp_until_tick = None;
    if prepared.hit.owner_id != prepared.hit.victim_id
        && matches!(weapon_type, 3 | 4)
        && prepared.unprotected_damage > 0
        && prepared.shooter_ship.is_some_and(arena_ship_emp_bomb)
    {
        let ticks = prepared.emp_shutdown_ticks;
        let until = crate::continuum::continuum_tick_add(prepared.hit.tick, ticks);
        state
            .bot_mut(prepared.hit.victim_id)
            .ok_or(())?
            .emp_until_tick = Some(until);
        emp_until_tick = Some(until);
    }
    if prepared.damage == 0 {
        return Ok(None);
    }
    let damage_seq = state.allocate_damage_seq();
    let bot = state.bot_mut(prepared.hit.victim_id).ok_or(())?;
    if staged.self_bomb_survival {
        bot.energy = 1;
        bot.energy_remainder = 0.0;
    } else {
        let mut pool = EnergyPool::new(bot.energy, bot.max_energy, bot.energy_remainder);
        pool.apply_damage(prepared.damage);
        bot.energy = pool.current();
        bot.energy_remainder = pool.recharge_remainder();
    }
    debug_assert_eq!(bot.energy, staged.energy_after);
    debug_assert_eq!(
        bot.energy_remainder.to_bits(),
        staged.energy_remainder_after.to_bits()
    );
    if current_human_source && bot.arm_e2e_stationary_after_human_damage(prepared.hit.tick) {
        tracing::info!(
            server_tick = prepared.hit.tick,
            bot_id = bot.player.id,
            life_seq = bot.life_seq,
            projectile_id = prepared.hit.projectile_id,
            shooter_id = prepared.hit.owner_id,
            "server bot stationary E2E geometry fixture armed by authoritative human damage"
        );
    }
    Ok(Some(ServerComputedDamage {
        damage_seq,
        projectile_id: prepared.hit.projectile_id,
        server_tick: prepared.hit.tick,
        victim_id: prepared.hit.victim_id,
        shooter_id: prepared.hit.owner_id,
        victim_life_seq: prepared.hit.victim_life_seq,
        shooter_life_seq: prepared.hit.owner_life_seq,
        weapon_data: prepared.hit.weapon_data,
        damage: prepared.damage,
        unprotected_damage: prepared.unprotected_damage,
        shield_until_tick: prepared.shield_until_tick,
        shield_remaining_ticks: prepared.shield_remaining_ticks,
        shield_lifetime_ticks: prepared.shield_lifetime_ticks,
        emp_until_tick,
        energy_before: staged.energy_before,
        energy_after: bot.energy,
        lethal: staged.lethal,
    }))
}

fn initial_damage_prize_rng_seed() -> i32 {
    std::env::var("ZUBSPACE_DAMAGE_PRIZE_RNG_SEED")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|seed| *seed != 0)
        .unwrap_or(crate::authority_prizes::SECURITY_PRIZE_SEED)
}

fn prepare_damage_prize_loss_selection(
    rng_seed: &mut i32,
    damage_factor: u16,
    damage: u16,
) -> Option<PreparedDamagePrizeLoss> {
    let prize_id = crate::authority_prizes::damage_prize_loss_id(rng_seed, damage_factor, damage)?;
    let replacement_prize_id = if prize_id == 0 {
        crate::authority_prizes::none_replacement_prize_id(
            &arena_prize_weights(),
            arena_prize_weight_total(),
            rng_seed,
        )
    } else {
        None
    };
    Some(PreparedDamagePrizeLoss {
        prize_id,
        replacement_prize_id,
    })
}

fn apply_prepared_damage_prize_loss(
    client: &mut crate::state::ClientHandle,
    tick: u32,
    damage_seq: u32,
    prepared: PreparedDamagePrizeLoss,
) {
    let energy_before_prize = client.server_energy;
    apply_prepared_damage_prize_mutation(client, tick, prepared);
    client.last_damage_prize_loss_seq = Some(damage_seq);
    client.last_damage_prize_loss_id = Some(prepared.prize_id);
    info!(
        player_id = client.player.id,
        damage_seq,
        prize_id = prepared.prize_id,
        energy_before_prize,
        energy_after_prize = client.server_energy,
        revision = client.server_prize_state_revision,
        "server damage prize lost"
    );
}

fn apply_prepared_damage_prize_mutation(
    client: &mut crate::state::ClientHandle,
    tick: u32,
    prepared: PreparedDamagePrizeLoss,
) {
    if prepared.prize_id == 0 {
        client.server_bounty = client.server_bounty.saturating_add(1);
        if let Some(replacement) = prepared.replacement_prize_id {
            client.apply_server_prize_effect_at_tick(replacement, tick);
        } else {
            client.advance_server_prize_state_revision();
        }
    } else {
        client.apply_server_prize_effect_at_tick(-prepared.prize_id, tick);
    }
}

#[cfg(test)]
fn apply_damage_prize_loss_with_factor(
    state: &mut ServerState,
    victim_id: u16,
    tick: u32,
    damage_seq: u32,
    damage: u16,
    energy_before: u16,
    lethal: bool,
    damage_factor: u16,
) -> Option<i16> {
    let eligible = state.clients.get(&victim_id).is_some_and(|client| {
        !lethal
            && client.server_bounty > 0
            && energy_before < client.server_max_energy
            && damage_factor != 0
    });
    if !eligible {
        return None;
    }
    if state.damage_prize_rng_seed == 0 {
        state.damage_prize_rng_seed = initial_damage_prize_rng_seed();
    }
    let prepared = prepare_damage_prize_loss_selection(
        &mut state.damage_prize_rng_seed,
        damage_factor,
        damage,
    )?;
    let client = state.clients.get_mut(&victim_id)?;
    apply_prepared_damage_prize_loss(client, tick, damage_seq, prepared);
    Some(prepared.prize_id)
}

/// Freeze a projectile terminal and its actual presentation recipients before
/// damage/lifecycle writes. Terminal delivery shares the damage reservation.
fn nonexplosive_terminal_delivery(
    state: &ServerState,
    hit: &ProjectileHit,
    shooter: crate::authority_identity::PlayerRuntimeIdentity,
) -> Option<crate::snapshot::RealtimeEventSessionDelivery> {
    if !matches!(continuum_weapon_type(hit.weapon_data), 1 | 2 | 7 | 15) {
        return None;
    }
    let projectile = state.active_projectiles.projectile(hit.projectile_id)?;
    let source = projectile.presentation_source?;
    let identity = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
        shooter,
        projectile.shot_group_id,
        projectile.id,
    )?;
    let recipients = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            let own_reconciliation = id == hit.owner_id
                && client.session_id == shooter.session.session_id
                && projectile.combat_timeline.is_some();
            (client.joined
                && (own_reconciliation
                    || ((id != hit.owner_id || continuum_weapon_type(hit.weapon_data) == 15)
                        && if matches!(continuum_weapon_type(hit.weapon_data), 1 | 2) {
                            client.threat_presentation.projectile_group_known(identity)
                        } else {
                            client.threat_presentation.projectile_known(identity)
                        })))
            .then_some(
                crate::authority_identity::PlayerSessionIdentity::new(id, client.session_id),
            )
        })
        .collect();
    Some(crate::snapshot::RealtimeEventSessionDelivery {
        recipients,
        server_tick: hit.tick,
        payload: crate::realtime_event::RealtimeEventPayload::ProjectileState(
            crate::realtime_event::RealtimeProjectileStateEvent {
                source_input_sequence: projectile
                    .combat_timeline
                    .map(|binding| binding.owner_input_sequence),
                terminal: true,
                motion_frozen: false,
                source,
                projectile_id: projectile.id,
                source_server_tick: projectile.created_tick,
                source_projectile_index: projectile.shot_index,
                weapon_data: projectile.weapon_data,
                position_x: f32::from(hit.x),
                position_y: f32::from(hit.y),
                velocity_x: 0.0,
                velocity_y: 0.0,
                expiry_tick: Some(hit.tick),
                behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
                bounces_remaining: 0,
                proximity: None,
            },
        ),
    })
}

fn apply_server_detected_hit_with_delivery_hook_outcome(
    state: &Arc<Mutex<ServerState>>,
    hit: &ProjectileHit,
    after_application: impl FnOnce(),
) -> (DetectedHitApplication, Option<ServerComputedDamage>) {
    let mut death_artifacts = Vec::new();
    let mut bot_death_artifacts = Vec::new();
    let mut watch_artifacts = Vec::new();
    let event = {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let transaction = match state_guard
            .pending_damage_transactions
            .get(&hit.projectile_id)
            .cloned()
        {
            Some(transaction) if transaction.matches_root(hit) => transaction,
            Some(_) => {
                state_guard
                    .pending_damage_transactions
                    .remove(&hit.projectile_id);
                return (DetectedHitApplication::Invalid, None);
            }
            None => {
                #[cfg(not(test))]
                {
                    // Production must arrive through the projectile-tick
                    // collision owner, which installs the immutable primary
                    // and splash transaction before releasing the state lock.
                    return (DetectedHitApplication::Invalid, None);
                }
                #[cfg(test)]
                {
                    let Some(transaction) =
                        prepare_server_detected_damage_transaction(&state_guard, hit)
                    else {
                        return (DetectedHitApplication::Invalid, None);
                    };
                    state_guard
                        .pending_damage_transactions
                        .insert(hit.projectile_id, transaction.clone());
                    transaction
                }
            }
        };
        if !transaction.identities_are_current(&state_guard) {
            state_guard
                .pending_damage_transactions
                .remove(&hit.projectile_id);
            return (DetectedHitApplication::Invalid, None);
        }
        if transaction
            .explosion
            .as_ref()
            .is_some_and(|explosion| !explosion.can_commit(&state_guard))
        {
            return (DetectedHitApplication::DeferredCapacity, None);
        }
        let Some((staged_hits, staged_bot_hits, damage_rng_seed_after)) =
            stage_prepared_damage_transaction(&state_guard, &transaction)
        else {
            state_guard
                .pending_damage_transactions
                .remove(&hit.projectile_id);
            return (DetectedHitApplication::Invalid, None);
        };
        let mut staged_order = staged_hits
            .iter()
            .enumerate()
            .map(|(index, staged)| (staged.prepared.order, false, index))
            .chain(
                staged_bot_hits
                    .iter()
                    .enumerate()
                    .map(|(index, staged)| (staged.prepared.order, true, index)),
            )
            .collect::<Vec<_>>();
        staged_order.sort_unstable_by_key(|(order, _, _)| *order);
        let terminal_delivery =
            nonexplosive_terminal_delivery(&state_guard, hit, transaction.shooter_runtime);
        let mut recipient_slices = staged_order
            .iter()
            .filter_map(|(_, bot_victim, index)| {
                let (damage, recipients) = if *bot_victim {
                    let staged = &staged_bot_hits[*index];
                    (
                        staged.prepared.damage,
                        staged.prepared.recipients.as_slice(),
                    )
                } else {
                    let staged = &staged_hits[*index];
                    (
                        staged.prepared.damage,
                        staged.prepared.recipients.as_slice(),
                    )
                };
                (damage > 0).then_some(recipients)
            })
            .collect::<Vec<_>>();
        if let Some(terminal) = &terminal_delivery {
            recipient_slices.push(terminal.recipients.as_slice());
        }
        if let Some(explosion) = &transaction.explosion {
            recipient_slices.extend(explosion.recipient_groups(&state_guard));
        }
        let Some(realtime_reservation) = crate::snapshot::reserve_realtime_event_session_deliveries(
            &mut state_guard,
            &recipient_slices,
            crate::snapshot::RealtimeEventPressureDisposition::Deferred,
        ) else {
            debug!(
                projectile_id = hit.projectile_id,
                shooter_id = hit.owner_id,
                victim_id = hit.victim_id,
                damage_event_count = staged_hits.len() + staged_bot_hits.len(),
                "deferring immutable authoritative damage transaction because dedicated realtime event capacity is full"
            );
            let realtime_queue_current = state_guard.realtime_event_queue_current();
            let pending_damage_transactions = state_guard.pending_damage_transactions.len();
            state_guard.qa_traces.record_for_all(serde_json::json!({
                "type": "server_damage_transaction_deferred",
                "reason": "realtime_event_capacity",
                "projectileId": hit.projectile_id,
                "shooterId": hit.owner_id,
                "victimId": hit.victim_id,
                "serverTick": hit.tick,
                "damageEventCount": staged_hits.len() + staged_bot_hits.len(),
                "realtimeQueueCurrent": realtime_queue_current,
                "realtimeQueueCapacity": crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                "pendingDamageTransactions": pending_damage_transactions,
            }));
            return (DetectedHitApplication::DeferredCapacity, None);
        };

        // Reservation does not publish or mutate gameplay. Revalidate every
        // first victim predecessor before the first authoritative write so an
        // outcome can never partially apply against a changed ledger.
        if !transaction.identities_are_current(&state_guard)
            || !staged_predecessors_are_current(&state_guard, &staged_hits, &staged_bot_hits)
        {
            state_guard
                .pending_damage_transactions
                .remove(&hit.projectile_id);
            return (DetectedHitApplication::Invalid, None);
        }
        state_guard.damage_prize_rng_seed = damage_rng_seed_after;
        let now = crate::wire::current_tick();
        let commit_tick = if crate::continuum::continuum_tick_diff(now, hit.tick) < 0 {
            hit.tick
        } else {
            now
        };
        // Install the fully reserved fan before death cleanup so a self-kill
        // retains this source life's still-live children. No flight can step
        // until this exclusive transaction returns to the server tick.
        let explosion_deliveries = transaction
            .explosion
            .as_ref()
            .map(|explosion| explosion.commit(&mut state_guard, commit_tick))
            .unwrap_or_default();

        for (_, bot_victim, index) in staged_order {
            if bot_victim {
                let staged = &staged_bot_hits[index];
                let prepared = &staged.prepared;
                let Ok(event) = apply_prepared_bot_damage_hit(&mut state_guard, staged) else {
                    state_guard
                        .pending_damage_transactions
                        .remove(&hit.projectile_id);
                    return (DetectedHitApplication::Invalid, None);
                };
                let Some(event) = event else {
                    continue;
                };
                if event.lethal {
                    if let Some(artifacts) = build_bot_kill_pipeline(
                        &mut state_guard,
                        &event,
                        transaction.shooter_runtime,
                    ) {
                        bot_death_artifacts.push(artifacts);
                    }
                }
                watch_artifacts.push(FrozenWatchDamageArtifacts {
                    event,
                    recipients: prepared.recipients.clone(),
                    snapshot_recipients: Vec::new(),
                    bot_victim: true,
                });
            } else {
                let staged = &staged_hits[index];
                let prepared = &staged.prepared;
                let Ok(event) = apply_prepared_damage_hit(&mut state_guard, staged) else {
                    state_guard
                        .pending_damage_transactions
                        .remove(&hit.projectile_id);
                    return (DetectedHitApplication::Invalid, None);
                };
                let Some(event) = event else {
                    continue;
                };
                if event.lethal {
                    if let Some(artifacts) = build_kill_pipeline(
                        &mut state_guard,
                        &prepared.hit,
                        transaction.shooter_runtime,
                        Some(&event),
                    ) {
                        death_artifacts.push(artifacts);
                    }
                }
                watch_artifacts.push(FrozenWatchDamageArtifacts {
                    event,
                    recipients: prepared.recipients.clone(),
                    snapshot_recipients: Vec::new(),
                    bot_victim: false,
                });
            }
        }
        for artifact in &mut watch_artifacts {
            artifact.snapshot_recipients = artifact
                .recipients
                .iter()
                .filter_map(|recipient| {
                    state_guard
                        .player_session_matches(*recipient)
                        .then(|| state_guard.active_player_runtime_identity(recipient.player_id))
                        .flatten()
                })
                .collect();
        }
        let mut realtime_deliveries = watch_artifacts
            .iter()
            .filter_map(|artifact| {
                let event = &artifact.event;
                let pending = crate::snapshot::PendingWatchDamage {
                    victim_id: event.victim_id,
                    shooter_id: event.shooter_id,
                    victim_life_seq: event.victim_life_seq,
                    shooter_life_seq: event.shooter_life_seq,
                    weapon_data: event.weapon_data,
                    damage: event.damage,
                    energy_before: event.energy_before,
                    energy_after: event.energy_after,
                    server_tick: event.server_tick,
                    damage_seq: event.damage_seq,
                    projectile_id: event.projectile_id,
                    lethal: event.lethal,
                };
                crate::snapshot::watch_damage_realtime_payload(&state_guard, pending).map(
                    |payload| crate::snapshot::RealtimeEventSessionDelivery {
                        recipients: artifact.recipients.clone(),
                        server_tick: event.server_tick,
                        payload,
                    },
                )
            })
            .collect::<Vec<_>>();
        if let Some(terminal) = terminal_delivery {
            realtime_deliveries.push(terminal);
        }
        realtime_deliveries.extend(explosion_deliveries);
        realtime_reservation.commit_sessions(&mut state_guard, realtime_deliveries);
        for artifact in &watch_artifacts {
            let damage_kind = if artifact.event.victim_id == hit.victim_id {
                "primary"
            } else {
                "splash"
            };
            if state_guard.qa_traces.is_recording() {
                state_guard.qa_traces.record_for_all(serde_json::json!({
                    "type": "server_damage_transaction_committed",
                    "damage": server_damage_event_json_value(&artifact.event, damage_kind),
                    "delivery": {
                        "realtimeRecipientCount": artifact.recipients.len(),
                        "snapshotHintRecipientCount": artifact.snapshot_recipients.len(),
                        "botVictim": artifact.bot_victim,
                    },
                }));
            }
            if artifact.event.lethal {
                let (next_life_seq, respawn_at_tick) = state_guard
                    .clients
                    .get(&artifact.event.victim_id)
                    .map(|victim| (victim.life_seq, victim.respawn_at_tick))
                    .or_else(|| {
                        state_guard
                            .bot(artifact.event.victim_id)
                            .map(|victim| (victim.life_seq, victim.respawn_at_tick))
                    })
                    .unwrap_or((artifact.event.victim_life_seq, None));
                state_guard.qa_traces.record_for_all(serde_json::json!({
                    "type": "server_actor_death_committed",
                    "projectileId": artifact.event.projectile_id,
                    "damageSeq": artifact.event.damage_seq,
                    "serverTick": artifact.event.server_tick,
                    "killerId": artifact.event.shooter_id,
                    "killerLifeSeq": artifact.event.shooter_life_seq,
                    "victimId": artifact.event.victim_id,
                    "retiredLifeSeq": artifact.event.victim_life_seq,
                    "nextLifeSeq": next_life_seq,
                    "respawnAtTick": respawn_at_tick,
                    "weapon": artifact.event.weapon_data,
                }));
            }
        }
        state_guard
            .pending_damage_transactions
            .remove(&hit.projectile_id);
        watch_artifacts
            .iter()
            .find(|artifact| artifact.event.victim_id == hit.victim_id)
            .or_else(|| watch_artifacts.first())
            .map(|artifact| artifact.event)
    };

    after_application();

    for artifact in &watch_artifacts {
        let damage_kind = if artifact.event.victim_id == hit.victim_id {
            "primary"
        } else {
            "splash"
        };
        info!(
            target: crate::log_targets::SERVER_DAMAGE_COMBAT,
            "{}",
            server_damage_event_json_line(&artifact.event, damage_kind)
        );
        if artifact.bot_victim {
            info!(
                target: crate::log_targets::SERVER_DAMAGE_COMBAT,
                damage_seq = artifact.event.damage_seq,
                projectile_id = artifact.event.projectile_id,
                server_tick = artifact.event.server_tick,
                shooter_id = artifact.event.shooter_id,
                shooter_life_seq = artifact.event.shooter_life_seq,
                bot_id = artifact.event.victim_id,
                bot_life_seq = artifact.event.victim_life_seq,
                weapon = artifact.event.weapon_data,
                damage = artifact.event.damage,
                energy_before = artifact.event.energy_before,
                energy_after = artifact.event.energy_after,
                lethal = artifact.event.lethal,
                damage_kind,
                "server bot damage applied"
            );
        }
        queue_watch_damage_snapshot_hint(state, artifact);
    }
    for artifacts in death_artifacts {
        if let Some(event_line) = artifacts.death_event_line {
            debug!("{event_line}");
        }
        send_reliable_packet_to_sessions_for_runtime(
            state,
            artifacts.victim_runtime,
            &artifacts.recipients,
            &artifacts.death_packet,
        );
        let mut packets = Vec::with_capacity(
            artifacts.score_packets.len()
                + artifacts.powerball_packets.len()
                + artifacts.turret_packets.len(),
        );
        packets.extend(artifacts.score_packets);
        packets.extend(artifacts.powerball_packets);
        packets.extend(artifacts.turret_packets);
        send_reliable_packets_to_session_identities(state, &artifacts.recipients, &packets);
    }
    for artifacts in bot_death_artifacts {
        debug!("{}", artifacts.death_event_line);
        send_reliable_packets_to_session_identities(
            state,
            &artifacts.recipients,
            &artifacts.packets,
        );
    }
    (DetectedHitApplication::Applied, event)
}

struct FrozenWatchDamageArtifacts {
    event: ServerComputedDamage,
    recipients: Vec<crate::authority_identity::PlayerSessionIdentity>,
    snapshot_recipients: Vec<crate::authority_identity::PlayerRuntimeIdentity>,
    bot_victim: bool,
}

fn queue_watch_damage_snapshot_hint(
    state: &Arc<Mutex<ServerState>>,
    artifacts: &FrozenWatchDamageArtifacts,
) {
    let event = &artifacts.event;
    crate::snapshot::queue_watch_damage_snapshot_event_to_recipients(
        state,
        &artifacts.snapshot_recipients,
        crate::snapshot::PendingWatchDamage {
            victim_id: event.victim_id,
            shooter_id: event.shooter_id,
            victim_life_seq: event.victim_life_seq,
            shooter_life_seq: event.shooter_life_seq,
            weapon_data: event.weapon_data,
            damage: event.damage,
            energy_before: event.energy_before,
            energy_after: event.energy_after,
            server_tick: event.server_tick,
            damage_seq: event.damage_seq,
            projectile_id: event.projectile_id,
            lethal: event.lethal,
        },
    );
}

struct DeathArtifacts {
    victim_runtime: crate::authority_identity::PlayerRuntimeIdentity,
    recipients: Vec<crate::authority_identity::PlayerSessionIdentity>,
    death_event_line: Option<String>,
    death_packet: Vec<u8>,
    score_packets: Vec<Vec<u8>>,
    powerball_packets: Vec<Vec<u8>>,
    turret_packets: Vec<Vec<u8>>,
}

struct BotDeathArtifacts {
    recipients: Vec<crate::authority_identity::PlayerSessionIdentity>,
    death_event_line: String,
    packets: Vec<Vec<u8>>,
}

fn build_bot_kill_pipeline(
    state: &mut ServerState,
    event: &ServerComputedDamage,
    shooter_runtime: crate::authority_identity::PlayerRuntimeIdentity,
) -> Option<BotDeathArtifacts> {
    let hit = ProjectileHit {
        projectile_id: event.projectile_id,
        owner_id: event.shooter_id,
        owner_life_seq: event.shooter_life_seq,
        victim_id: event.victim_id,
        victim_life_seq: event.victim_life_seq,
        weapon_data: event.weapon_data,
        x: 0,
        y: 0,
        tick: event.server_tick,
    };
    let victim_runtime = state
        .bot(hit.victim_id)
        .filter(|bot| bot.alive() && bot.life_seq == hit.victim_life_seq)?
        .runtime_identity();
    let mut packets = Vec::new();
    if let Some(player) = objective_player(state, hit.victim_id) {
        match player_lifecycle_packets_for_context(
            state,
            player,
            Some(hit.owner_id),
            ObjectiveLifecycleReason::Death { killer: None },
            hit.tick,
        ) {
            Ok(objective_packets) => packets.extend(objective_packets),
            Err(error) => {
                tracing::error!(
                    victim_id = hit.victim_id,
                    ?error,
                    "bot death retained: objective cleanup could not commit"
                );
                return None;
            }
        }
    }
    let respawn_at_tick =
        crate::continuum::continuum_tick_add(hit.tick, arena_respawn_delay_ticks());
    let (killed_bounty, death_x, death_y, death_velocity, next_life_seq, victim_score) = {
        let victim = state.bot_mut(hit.victim_id)?;
        let captured = (
            victim.bounty,
            victim.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            victim.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            (victim.velocity_x, victim.velocity_y),
        );
        victim.energy = 0;
        victim.energy_remainder = 0.0;
        victim.velocity_x = 0.0;
        victim.velocity_y = 0.0;
        victim.respawn_at_tick = Some(respawn_at_tick);
        victim.player.losses = victim.player.losses.saturating_add(1);
        victim.player.flags = 0;
        let next_life_seq = victim.advance_life();
        (
            captured.0,
            captured.1,
            captured.2,
            captured.3,
            next_life_seq,
            victim.player.clone(),
        )
    };
    // Bot lifecycle cleanup removes incoming collision markers. Resolve this
    // explosion first so a committed old-life hit cannot be stranded or later
    // retarget the replacement life.
    state.active_projectiles.resolve_hit(hit.projectile_id);
    state.retire_projectile_owner_after_damage_death(victim_runtime, hit.tick);
    packets.push(score_update(&victim_score));

    // Damage admission already validated this immutable source runtime. A
    // still-joined source session owns cumulative credit across lives; only
    // the launching life can receive current-ship bounty/flag benefits.
    let transfer_on_wire = 0;
    let mut persist_killer = false;
    if let Some(killer) = state
        .clients
        .get_mut(&hit.owner_id)
        .filter(|killer| killer.joined && killer.session_id == shooter_runtime.session.session_id)
    {
        let killer_same_life = killer.life_seq == hit.owner_life_seq;
        killer.player.wins = killer.player.wins.saturating_add(1);
        if hit.owner_id != hit.victim_id {
            killer.player.kill_points = killer
                .player
                .kill_points
                .saturating_add(u32::from(killed_bounty));
        }
        if killer_same_life && hit.owner_id != hit.victim_id && killed_bounty > 0 {
            killer.server_bounty = killer
                .server_bounty
                .saturating_add(arena_bounty_increase_for_kill());
        }
        packets.push(score_update(&killer.player));
        persist_killer = true;
    } else if let Some(killer) = state
        .bot_mut(hit.owner_id)
        .filter(|killer| killer.session_id == shooter_runtime.session.session_id)
    {
        let killer_same_life = killer.life_seq == hit.owner_life_seq;
        killer.player.wins = killer.player.wins.saturating_add(1);
        if hit.owner_id != hit.victim_id {
            killer.player.kill_points = killer
                .player
                .kill_points
                .saturating_add(u32::from(killed_bounty));
        }
        if killer_same_life && hit.owner_id != hit.victim_id && killed_bounty > 0 {
            killer.bounty = killer
                .bounty
                .saturating_add(arena_bounty_increase_for_kill());
        }
        packets.push(score_update(&killer.player));
    }
    if persist_killer {
        persist_stats_for_client(state, hit.owner_id);
    }

    let rolled_green_id = roll_death_green_prize_id(state);
    let (green_id, green_collectible_id) =
        if rolled_green_id > 0 && (death_velocity.0 != 0.0 || death_velocity.1 != 0.0) {
            create_death_collectible(
                state,
                death_x,
                death_y,
                rolled_green_id as i16,
                hit.tick,
                arena_death_prize_lifetime_ticks(),
            )
            .map(|id| (rolled_green_id, id))
            .unwrap_or((-1, 0))
        } else {
            (-1, 0)
        };
    packets.insert(
        0,
        build_player_death_packet(
            hit.owner_id,
            hit.victim_id,
            killed_bounty,
            transfer_on_wire,
            green_id,
            green_collectible_id,
            next_life_seq,
            respawn_at_tick,
        ),
    );
    let death_event_line = server_player_death_event_json_line(
        event,
        next_life_seq,
        killed_bounty,
        transfer_on_wire,
        green_id,
        green_collectible_id,
        arena_respawn_delay().as_millis(),
    );
    Some(BotDeathArtifacts {
        recipients: state.joined_player_session_identities(),
        death_event_line,
        packets,
    })
}

/// Run the full kill bookkeeping for a server-detected lethal hit.
/// Returns the packets that need broadcasting (caller drops the
/// state lock and broadcasts them in order). Returns `None` if the
/// victim was already dead or otherwise invalid.
fn build_kill_pipeline(
    state: &mut ServerState,
    hit: &ProjectileHit,
    shooter_runtime: crate::authority_identity::PlayerRuntimeIdentity,
    damage_event: Option<&ServerComputedDamage>,
) -> Option<DeathArtifacts> {
    let victim_id = hit.victim_id;
    let killer_id = hit.owner_id;
    let mut score_packets: Vec<Vec<u8>> = Vec::new();
    let respawn_delay = arena_respawn_delay();
    let respawn_delay_ms = respawn_delay.as_millis();

    // Capture bounty + recent damagers BEFORE clearing
    // runtime — `clear_server_runtime` drains the assist ring as
    // part of its respawn reset, so we must snapshot it first.
    let (killed_bounty, flag_transfer, assist_candidates, death_motion) = {
        let victim = state.clients.get(&victim_id)?;
        if !victim.joined || !victim.alive || victim.life_seq != hit.victim_life_seq {
            return None;
        }
        let recent: Vec<crate::authority_identity::LifeIdentity> = victim
            .recent_damagers
            .iter()
            .filter(|(_, at_tick)| {
                crate::continuum::continuum_tick_elapsed(hit.tick, *at_tick)
                    <= crate::state::ASSIST_DAMAGE_WINDOW_TICKS
            })
            .map(|(identity, _)| *identity)
            .collect();
        (
            victim.server_bounty,
            // Death transfer is a profile policy; the profile-neutral
            // foundation cannot infer it from the client-visible carry count.
            0,
            recent,
            victim
                .last_position
                .as_deref()
                .and_then(crate::arena::large_position_coordinates_and_velocity),
        )
    };
    let objective_player = objective_player(state, victim_id);
    let objective_packets = if let Some(player) = objective_player {
        match player_lifecycle_packets_for_context(
            state,
            player,
            Some(killer_id),
            ObjectiveLifecycleReason::Death { killer: None },
            hit.tick,
        ) {
            Ok(packets) => packets,
            Err(error) => {
                tracing::error!(
                    victim_id,
                    ?error,
                    "damage death retained: objective cleanup could not commit"
                );
                return None;
            }
        }
    } else {
        Vec::new()
    };

    // Apply victim death state.
    let victim_next_life_seq;
    let victim_respawn_at_tick;
    {
        let victim = state.clients.get_mut(&victim_id)?;
        victim.alive = false;
        victim.respawn_at_tick = Some(crate::continuum::continuum_tick_add(
            hit.tick,
            arena_respawn_delay_ticks(),
        ));
        victim_respawn_at_tick = victim
            .respawn_at_tick
            .expect("death always installs a respawn deadline");
        victim.player.losses = victim.player.losses.saturating_add(1);
        victim.player.flags = 0;
        victim.clear_server_runtime();
        score_packets.push(score_update(&victim.player));
    }
    victim_next_life_seq = state
        .advance_client_life_after_damage_death(victim_id, hit.tick)?
        .life_seq;
    let victim_runtime = state.active_player_runtime_identity(victim_id)?;
    debug_assert_eq!(victim_runtime.life.life_seq, victim_next_life_seq);
    let retired_victim_runtime = crate::authority_identity::PlayerRuntimeIdentity::new(
        victim_runtime.session,
        crate::authority_identity::LifeIdentity::new(victim_id, hit.victim_life_seq),
    );
    state.clear_client_position_after_damage_death(retired_victim_runtime, hit.tick);
    let powerball_packets = objective_packets;
    let turret_packets = remove_player_turret_link_packets(state, victim_id);
    persist_stats_for_client(state, victim_id);

    // Cumulative credit belongs to the authenticated session even when an
    // earlier-life shot lands after respawn. Keep current-life bounty/flag
    // transfer separate, and never credit a replacement session at this id.
    // Award killer side. Killer might be the victim themselves
    // (self-bomb death); self-kills skip bounty/flag transfer but
    // still increment wins, matching the existing server semantics.
    let mut transfer_on_wire = flag_transfer;
    if let Some(killer) = state
        .clients
        .get_mut(&killer_id)
        .filter(|killer| killer.joined && killer.session_id == shooter_runtime.session.session_id)
    {
        let killer_same_life = killer.life_seq == hit.owner_life_seq;
        killer.player.wins = killer.player.wins.saturating_add(1);
        if hit.owner_id != hit.victim_id {
            killer.player.kill_points = killer
                .player
                .kill_points
                .saturating_add(u32::from(killed_bounty));
        }
        if killer_same_life && killer_id != victim_id {
            if killed_bounty > 0 {
                killer.server_bounty = killer
                    .server_bounty
                    .saturating_add(arena_bounty_increase_for_kill());
            }
            killer.player.flags = killer.player.flags.saturating_add(flag_transfer);
        } else {
            transfer_on_wire = 0;
        }
        score_packets.push(score_update(&killer.player));
    } else if let Some(killer) = state
        .bot_mut(killer_id)
        .filter(|killer| killer.session_id == shooter_runtime.session.session_id)
    {
        let killer_same_life = killer.life_seq == hit.owner_life_seq;
        killer.player.wins = killer.player.wins.saturating_add(1);
        if hit.owner_id != hit.victim_id {
            killer.player.kill_points = killer
                .player
                .kill_points
                .saturating_add(u32::from(killed_bounty));
        }
        if killer_same_life && killed_bounty > 0 {
            killer.bounty = killer
                .bounty
                .saturating_add(arena_bounty_increase_for_kill());
        }
        if killer_same_life {
            killer.player.flags = killer.player.flags.saturating_add(flag_transfer);
        } else {
            transfer_on_wire = 0;
        }
        score_packets.push(score_update(&killer.player));
    } else {
        // Killer no longer in the arena (disconnected) — still emit
        // the kill packet but skip the transfer.
        transfer_on_wire = 0;
    }
    if state.player_session_matches(shooter_runtime.session) {
        persist_stats_for_client(state, killer_id);
    }

    // Phase 2 polish: award assist credit to every distinct recent
    // damager other than the killer and the victim. Mirrors the
    // assist semantics of the legacy client-reported
    // `award_assists_from_report` path. Skips clients who have
    // since disconnected.
    let mut seen = std::collections::BTreeSet::new();
    for assister_identity in assist_candidates {
        let assister_id = assister_identity.player_id;
        if assister_id == killer_id || assister_id == victim_id || !seen.insert(assister_id) {
            continue;
        }
        if let Some(assister) = state
            .clients
            .get_mut(&assister_id)
            .filter(|assister| assister.joined && assister.life_seq == assister_identity.life_seq)
        {
            assister.player.assists = assister.player.assists.saturating_add(1);
            score_packets.push(crate::roster::score_update(&assister.player));
            persist_stats_for_client(state, assister_id);
        } else if let Some(assister) = state
            .bot_mut(assister_id)
            .filter(|assister| assister.alive() && assister.life_seq == assister_identity.life_seq)
        {
            assister.player.assists = assister.player.assists.saturating_add(1);
            score_packets.push(crate::roster::score_update(&assister.player));
        }
    }

    let rolled_green_id = roll_death_green_prize_id(state);
    let (green_id, green_collectible_id) = death_motion
        .filter(|(_, (velocity_x, velocity_y))| {
            rolled_green_id > 0 && (*velocity_x != 0 || *velocity_y != 0)
        })
        .and_then(|((x, y), _)| {
            create_death_collectible(
                state,
                x,
                y,
                rolled_green_id as i16,
                hit.tick,
                arena_death_prize_lifetime_ticks(),
            )
            .map(|collectible_id| (rolled_green_id, collectible_id))
        })
        .unwrap_or((-1, 0));
    let death_packet = build_player_death_packet(
        killer_id,
        victim_id,
        killed_bounty,
        transfer_on_wire,
        green_id,
        green_collectible_id,
        victim_next_life_seq,
        victim_respawn_at_tick,
    );
    let victim_name = state
        .clients
        .get(&victim_id)
        .map(|client| client.player.name.clone())
        .unwrap_or_default();
    let killer_name = state
        .clients
        .get(&killer_id)
        .map(|client| client.player.name.clone())
        .or_else(|| state.bot(killer_id).map(|bot| bot.player.name.clone()))
        .unwrap_or_default();
    info!(
        damage_seq = damage_event.map(|event| event.damage_seq),
        projectile_id = hit.projectile_id,
        server_tick = hit.tick,
        killer_id,
        killer_name = %killer_name,
        victim_id,
        victim_name = %victim_name,
        victim_life_seq = damage_event.map(|event| event.victim_life_seq),
        victim_next_life_seq,
        killer_life_seq = damage_event.map(|event| event.shooter_life_seq),
        weapon = hit.weapon_data,
        weapon_type = continuum_weapon_type(hit.weapon_data),
        weapon_level = continuum_weapon_level(hit.weapon_data),
        energy_before = damage_event.map(|event| event.energy_before),
        energy_after = damage_event.map(|event| event.energy_after),
        killed_bounty,
        flag_transfer = transfer_on_wire,
        green_id,
        green_collectible_id,
        respawn_delay_ms,
        self_kill = killer_id == victim_id,
        "server player death"
    );
    let death_event_line = damage_event.map(|event| {
        server_player_death_event_json_line(
            event,
            victim_next_life_seq,
            killed_bounty,
            transfer_on_wire,
            green_id,
            green_collectible_id,
            respawn_delay_ms,
        )
    });
    Some(DeathArtifacts {
        victim_runtime,
        recipients: state.joined_player_session_identities(),
        death_event_line,
        death_packet,
        score_packets,
        powerball_packets,
        turret_packets,
    })
}

#[cfg(test)]
fn apply_server_detected_hit_inner(
    state: &mut ServerState,
    hit: &ProjectileHit,
    authority_active: bool,
) -> Option<ServerComputedDamage> {
    if !authority_active {
        return None;
    }
    let (shooter_ship, unprotected_damage) = preview_server_detected_hit(state, hit)?;
    let shield = authoritative_shield_damage(
        state.clients.get(&hit.victim_id)?,
        hit.tick,
        unprotected_damage,
    );
    let damage = shield.damage;
    let weapon_type = continuum_weapon_type(hit.weapon_data);
    let client = state.clients.get_mut(&hit.victim_id)?;
    // EMP is evaluated from raw bomb damage before Shield, but never applies
    // to the shooter themselves. The reference game assigns (replaces) the source
    // deadline even when a later hit is shorter.
    let mut emp_until_tick = None;
    if hit.owner_id != hit.victim_id
        && matches!(weapon_type, 3 | 4)
        && unprotected_damage > 0
        && shooter_ship.is_some_and(arena_ship_emp_bomb)
    {
        let ticks = emp_shutdown_ticks_for_damage(hit, shooter_ship, unprotected_damage);
        let until = crate::continuum::continuum_tick_add(hit.tick, ticks);
        if !client.server_emp_active(hit.tick) {
            client.server_emp_start_sequence = client
                .last_input_seq
                .map(|sequence| sequence.wrapping_add(1));
        }
        client.server_emp_until_tick = Some(until);
        emp_until_tick = Some(until);
    }
    if damage == 0 {
        // A fully absorbed EMP bomb can still establish EMP above, but it has
        // no damage identity, energy/assist mutation, or WATCH_DAMAGE fact.
        return None;
    }
    let damage_seq = state.allocate_damage_seq();
    let client = state.clients.get_mut(&hit.victim_id)?;
    let energy_before = client.server_energy;
    let victim_life_seq = hit.victim_life_seq;
    let mut pool = EnergyPool::new(
        client.server_energy,
        client.server_max_energy,
        client.server_energy_remainder,
    );
    let raw_lethal = energy_before < damage;
    let self_bomb_survival =
        hit.owner_id == hit.victim_id && matches!(weapon_type, 3 | 4 | 8) && raw_lethal;
    let lethal = raw_lethal && !self_bomb_survival;
    let energy_after = if self_bomb_survival {
        1
    } else {
        pool.apply_damage(damage);
        pool.current()
    };
    client.server_energy = energy_after;
    client.server_energy_remainder = if self_bomb_survival {
        0.0
    } else {
        pool.recharge_remainder()
    };
    advance_damage_recharge_anchor(client, hit.tick);
    client.last_reported_energy = energy_after;
    update_cached_energy_after_damage(client, energy_after);
    // Phase 2 polish: push the shooter onto the victim's recent-
    // damagers ring so the kill pipeline can award assist credit on
    // a subsequent lethal hit. Skip self-damage so a player can't
    // assist their own death; the ring is also pruned of stale
    // entries to keep its size bounded.
    if hit.owner_id != hit.victim_id {
        client.recent_damagers.retain(|(_, at_tick)| {
            crate::continuum::continuum_tick_elapsed(hit.tick, *at_tick)
                <= crate::state::ASSIST_DAMAGE_WINDOW_TICKS
        });
        if client.recent_damagers.len() >= crate::state::RECENT_DAMAGERS_CAPACITY {
            client.recent_damagers.remove(0);
        }
        client
            .recent_damagers
            .push((hit.owner_identity(), hit.tick));
    }
    Some(ServerComputedDamage {
        damage_seq,
        projectile_id: hit.projectile_id,
        server_tick: hit.tick,
        victim_id: hit.victim_id,
        shooter_id: hit.owner_id,
        victim_life_seq,
        shooter_life_seq: hit.owner_life_seq,
        weapon_data: hit.weapon_data,
        damage,
        unprotected_damage,
        shield_until_tick: shield.until_tick,
        shield_remaining_ticks: shield.remaining_ticks,
        shield_lifetime_ticks: shield.lifetime_ticks,
        emp_until_tick,
        energy_before,
        energy_after,
        lethal,
    })
}

#[cfg(test)]
fn preview_server_detected_hit(
    state: &ServerState,
    hit: &ProjectileHit,
) -> Option<(Option<u8>, u16)> {
    let shooter = state
        .clients
        .get(&hit.owner_id)
        .filter(|client| client.life_seq == hit.owner_life_seq)?;
    let target_tick =
        crate::continuum::continuum_tick_sub(hit.tick, shooter.lag_compensation_ticks as u32);
    // The legacy unit-only inner helper predates production projectile
    // collision snapshots. Fixtures without position state use the collision
    // point as their direct-hit center; production admission always supplies
    // the exact rewound anchor through `preview_detected_damage_hits`.
    let anchor = rewound_client_position(state.clients.get(&hit.victim_id)?, target_tick)
        .unwrap_or((hit.x as f32, hit.y as f32));
    preview_server_detected_hit_at_anchor(state, hit, anchor)
}

#[cfg(test)]
fn preview_server_detected_hit_at_anchor(
    state: &ServerState,
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
) -> Option<(Option<u8>, u16)> {
    preview_server_detected_hit_at_anchor_with_radius(state, hit, victim_anchor, None)
}

fn preview_server_detected_hit_at_geometry(
    state: &ServerState,
    hit: &ProjectileHit,
    geometry: FrozenDamageGeometry,
) -> Option<(Option<u8>, u16)> {
    preview_server_detected_hit_at_anchor_with_radius(
        state,
        hit,
        (geometry.target_anchor_x, geometry.target_anchor_y),
        geometry.explosion_radius,
    )
}

fn preview_server_detected_hit_at_anchor_with_radius(
    state: &ServerState,
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
    explosion_radius: Option<f32>,
) -> Option<(Option<u8>, u16)> {
    let weapon_type = continuum_weapon_type(hit.weapon_data);
    if hit.owner_id == hit.victim_id && !matches!(weapon_type, 3 | 4 | 8) {
        return None;
    }
    let (_, _, _, shooter_ship) = damage_shooter_context(state, hit)?;
    let shooter_ship = Some(shooter_ship);
    let client = state.clients.get(&hit.victim_id)?;
    if !client.joined
        || !client.alive
        || client.player.ship >= 8
        || client.life_seq != hit.victim_life_seq
    {
        return None;
    }
    let damage_settings = arena_weapon_damage_settings();
    let damage = if matches!(weapon_type, 3 | 4 | 8) {
        unprotected_explosive_contact_damage(
            hit,
            victim_anchor,
            damage_settings,
            shooter_ship,
            explosion_radius.unwrap_or_else(|| {
                bomb_explosion_radius_pixels(continuum_weapon_explosion_level(hit.weapon_data))
            }),
        )?
    } else {
        unprotected_direct_hit_damage(state, hit, damage_settings)?
    };
    (damage != 0 || matches!(weapon_type, 3 | 4 | 8 | 15)).then_some((shooter_ship, damage))
}

#[cfg(test)]
fn preview_server_detected_bot_hit_at_anchor(
    state: &ServerState,
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
) -> Option<(Option<u8>, u16)> {
    preview_server_detected_bot_hit_at_anchor_with_radius(state, hit, victim_anchor, None)
}

fn preview_server_detected_bot_hit_at_geometry(
    state: &ServerState,
    hit: &ProjectileHit,
    geometry: FrozenDamageGeometry,
) -> Option<(Option<u8>, u16)> {
    preview_server_detected_bot_hit_at_anchor_with_radius(
        state,
        hit,
        (geometry.target_anchor_x, geometry.target_anchor_y),
        geometry.explosion_radius,
    )
}

fn preview_server_detected_bot_hit_at_anchor_with_radius(
    state: &ServerState,
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
    explosion_radius: Option<f32>,
) -> Option<(Option<u8>, u16)> {
    let weapon_type = continuum_weapon_type(hit.weapon_data);
    if hit.owner_id == hit.victim_id && !matches!(weapon_type, 3 | 4 | 8) {
        return None;
    }
    let (_, _, _, shooter_ship) = damage_shooter_context(state, hit)?;
    let shooter_ship = Some(shooter_ship);
    let victim = state
        .bot(hit.victim_id)
        .filter(|bot| bot.alive() && bot.life_seq == hit.victim_life_seq)?;
    let damage_settings = arena_weapon_damage_settings();
    let damage = if matches!(weapon_type, 3 | 4 | 8) {
        unprotected_explosive_contact_damage(
            hit,
            victim_anchor,
            damage_settings,
            shooter_ship,
            explosion_radius.unwrap_or_else(|| {
                bomb_explosion_radius_pixels(continuum_weapon_explosion_level(hit.weapon_data))
            }),
        )?
    } else {
        unprotected_direct_hit_damage(state, hit, damage_settings)?
    };
    let _ = victim;
    (damage != 0 || matches!(weapon_type, 3 | 4 | 8 | 15)).then_some((shooter_ship, damage))
}

fn unprotected_direct_hit_damage(
    state: &ServerState,
    hit: &ProjectileHit,
    settings: crate::authority_damage::ArenaWeaponDamageSettings,
) -> Option<u16> {
    if continuum_weapon_type(hit.weapon_data) == 15 {
        let projectile = state.active_projectiles.projectile(hit.projectile_id)?;
        if projectile.owner_identity() != hit.owner_identity()
            || projectile.weapon_data != hit.weapon_data
        {
            return None;
        }
        let age = crate::continuum::continuum_tick_diff(hit.tick, projectile.created_tick);
        if age < 0 {
            return None;
        }
        return authoritative_unprotected_shrapnel_damage(hit.weapon_data, age as u32, settings);
    }
    authoritative_unprotected_direct_damage(hit.weapon_data, settings, arena_exact_damage())
        .or_else(|| maximum_reported_weapon_damage(hit.weapon_data, settings))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuthoritativeShieldDamage {
    damage: u16,
    until_tick: Option<u32>,
    remaining_ticks: u32,
    lifetime_ticks: u32,
}

fn authoritative_shield_damage(
    victim: &crate::state::ClientHandle,
    hit_tick: u32,
    unprotected_damage: u16,
) -> AuthoritativeShieldDamage {
    authoritative_shield_damage_from_deadline(
        victim.server_shield_until_tick,
        arena_ship_shield_lifetime_ticks(victim.player.ship),
        hit_tick,
        unprotected_damage,
    )
}

/// The reference game's `FlaggerDamagePercent`: a player carrying a flag or the ball takes
/// scaled damage. The client applies this multiplier last — after super/shield
/// protection and bullet exactness — and truncates, so the server mirrors that
/// order against its own post-shield damage.
fn flagger_damage_after_carry(damage: u16, multiplier: f32, carrying: bool) -> u16 {
    if !carrying {
        return damage;
    }
    (f32::from(damage) * multiplier.max(0.0))
        .trunc()
        .clamp(0.0, f32::from(u16::MAX)) as u16
}

/// Server-side twin of the client's `carrying_flag` damage state: an objective
/// carrier takes the flagger damage modifier.
fn victim_carries_objective(
    state: &ServerState,
    victim: crate::authority_identity::PlayerRuntimeIdentity,
) -> bool {
    state.objectives.flag_carrier_count(victim) > 0
        || !state.objectives.carried_powerball_ids(victim).is_empty()
}

fn authoritative_bot_shield_damage(
    victim: &crate::state::ServerBot,
    hit_tick: u32,
    unprotected_damage: u16,
) -> AuthoritativeShieldDamage {
    authoritative_shield_damage_from_deadline(
        victim.shield_until_tick,
        arena_ship_shield_lifetime_ticks(victim.player.ship),
        hit_tick,
        unprotected_damage,
    )
}

fn authoritative_shield_damage_from_deadline(
    until_tick: Option<u32>,
    lifetime_ticks: u32,
    hit_tick: u32,
    unprotected_damage: u16,
) -> AuthoritativeShieldDamage {
    let Some(until_tick) = until_tick else {
        return AuthoritativeShieldDamage {
            damage: unprotected_damage,
            until_tick: None,
            remaining_ticks: 0,
            lifetime_ticks,
        };
    };
    let remaining_ticks = (crate::continuum::continuum_tick_diff(until_tick, hit_tick).max(0)
        as u32)
        .min(lifetime_ticks);
    if lifetime_ticks == 0 || remaining_ticks == 0 {
        return AuthoritativeShieldDamage {
            damage: unprotected_damage,
            until_tick: None,
            remaining_ticks: 0,
            lifetime_ticks,
        };
    }
    // The reference game uses the live remaining-Shield fraction and truncates the
    // protected part when assigning the float expression back to integer
    // damage. Capture all operands with the prepared transaction so retries,
    // splash hits, and wrap cannot observe a later timer value.
    let protected =
        (unprotected_damage as f32 * (remaining_ticks as f32 / lifetime_ticks as f32)) as u16;
    AuthoritativeShieldDamage {
        damage: unprotected_damage.saturating_sub(protected),
        until_tick: Some(until_tick),
        remaining_ticks,
        lifetime_ticks,
    }
}

fn advance_damage_recharge_anchor(client: &mut crate::state::ClientHandle, collision_tick: u32) {
    // Damage debits the pool as it stands at commit. Presentation/capacity
    // waits may have advanced it beyond the immutable collision tick. Never
    // refund that already-counted interval on the next recharge step.
    if client
        .last_energy_recharge_tick
        .is_none_or(|last| crate::continuum::continuum_tick_diff(collision_tick, last) > 0)
    {
        client.last_energy_recharge_tick = Some(collision_tick);
    }
}

fn update_cached_energy_after_damage(client: &mut crate::state::ClientHandle, energy_after: u16) {
    if let Some(position) = client.last_position.as_mut() {
        if position.len() >= 23 {
            position[21..23].copy_from_slice(&energy_after.to_le_bytes());
        }
    }
    if let Some(sim) = client.server_sim_state.as_mut() {
        sim.energy = sim.energy.min(energy_after as f32);
        sim.max_energy = client.server_max_energy as f32;
    }
}

/// Phase 2g: the reference game's bomb-falloff damage formula. Returns the
/// damage delivered to a hull at `distance` from a bomb whose
/// explosion radius is `BombExplodePixels * level` and full damage
/// is `BombDamageLevel / 1000`. EMP and bouncing-bomb percent
/// multipliers are applied multiplicatively (as the reference game does)
/// when the appropriate weapon flags / shooter capabilities are set.
/// Picks the position-history sample closest to (and not after) `target_tick`,
/// mirroring `rewound_position` in the projectile registry. Falls back to the
/// latest reported position when no history sample is in range (fresh join or
/// truncated history). The returned center is copied into the prepared damage
/// transaction so eligibility and falloff always consume the same anchor.
#[cfg(test)]
fn rewound_client_position(
    client: &crate::state::ClientHandle,
    target_tick: u32,
) -> Option<(f32, f32)> {
    let mut best: Option<(u32, f32, f32)> = None;
    for sample in &client.position_history {
        let age = crate::continuum::continuum_tick_diff(target_tick, sample.tick);
        if age < 0 {
            // Sample is in the future relative to the target — skip.
            continue;
        }
        match best {
            Some((best_tick, _, _))
                if crate::continuum::continuum_tick_diff(target_tick, best_tick) <= age => {}
            _ => best = Some((sample.tick, sample.x, sample.y)),
        }
    }
    if let Some((_, x, y)) = best {
        return Some((x, y));
    }
    let (x, y) = crate::arena::large_position_coordinates(client.last_position.as_deref()?)?;
    Some((x as f32, y as f32))
}

#[cfg(test)]
fn bomb_damage_with_falloff(
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
    settings: crate::authority_damage::ArenaWeaponDamageSettings,
    level: u8,
    shooter_ship: Option<u8>,
) -> u16 {
    bomb_damage_with_falloff_radius(
        hit,
        victim_anchor,
        settings,
        shooter_ship,
        bomb_explosion_radius_pixels(level),
    )
}

// A real explosive contact still terminates the parent and creates its fan
// when authored damage or falloff truncates to zero. Reject geometry outside
// the blast separately, rather than treating zero damage as no collision.
fn unprotected_explosive_contact_damage(
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
    settings: crate::authority_damage::ArenaWeaponDamageSettings,
    shooter_ship: Option<u8>,
    explosion_radius: f32,
) -> Option<u16> {
    let radius = explosion_radius.max(1.0);
    let dx = hit.x as f32 - victim_anchor.0;
    let dy = hit.y as f32 - victim_anchor.1;
    if !dx.is_finite() || !dy.is_finite() || dx.hypot(dy) >= radius {
        return None;
    }
    Some(bomb_damage_with_falloff_radius(
        hit,
        victim_anchor,
        settings,
        shooter_ship,
        radius,
    ))
}

fn bomb_damage_with_falloff_radius(
    hit: &ProjectileHit,
    victim_anchor: (f32, f32),
    settings: crate::authority_damage::ArenaWeaponDamageSettings,
    shooter_ship: Option<u8>,
    explosion_radius: f32,
) -> u16 {
    let explode_pixels = explosion_radius.max(1.0); // avoid div-by-zero
    let dx = hit.x as f32 - victim_anchor.0;
    let dy = hit.y as f32 - victim_anchor.1;
    let distance = (dx * dx + dy * dy).sqrt();
    if distance >= explode_pixels {
        return 0;
    }

    // The reference game's `OnWeaponHit`: the full bomb/Thor damage is `BombDamageLevel`
    // (Thor scales by `1 + raw_level^2`), then the *integer* `/1000` lands
    // before either percent multiplier.
    let mut adjusted =
        (crate::authority_damage::bomb_full_raw_damage(hit.weapon_data, settings) / 1000) as f32;
    // The reference game's SetWeaponSprite marks only Bomb/ProximityBomb as EMP.
    // A Thor never inherits that flag from an EMP-capable shooter.
    if matches!(continuum_weapon_type(hit.weapon_data), 3 | 4)
        && shooter_ship.is_some_and(arena_ship_emp_bomb)
    {
        adjusted = (adjusted * settings.emp_bomb_damage_percent as f32 / 1000.0).trunc();
    }
    // Bouncing-bomb percent: the reference gates this on the shooter ship's
    // `BombBounceCount > 0`, not on the individual weapon's alternate bit, and
    // truncates.
    if shooter_ship.is_some_and(|ship| arena_ship_bomb_bounce_count(ship) > 0) {
        adjusted = (adjusted * settings.bouncing_bomb_damage_percent as f32 / 1000.0).trunc();
    }

    let damage = (explode_pixels - distance) * (adjusted / explode_pixels);
    damage.trunc().clamp(0.0, u16::MAX as f32) as u16
}

#[cfg(test)]
fn emp_shutdown_ticks_for_damage(
    hit: &ProjectileHit,
    shooter_ship: Option<u8>,
    unprotected_damage: u16,
) -> u32 {
    emp_shutdown_ticks_for_damage_radius(hit, shooter_ship, unprotected_damage, None)
}

fn emp_shutdown_ticks_for_damage_radius(
    hit: &ProjectileHit,
    shooter_ship: Option<u8>,
    unprotected_damage: u16,
    explosion_radius: Option<f32>,
) -> u32 {
    let full_damage = bomb_damage_with_falloff_radius(
        hit,
        (hit.x as f32, hit.y as f32),
        arena_weapon_damage_settings(),
        shooter_ship,
        explosion_radius.unwrap_or_else(|| {
            bomb_explosion_radius_pixels(continuum_weapon_explosion_level(hit.weapon_data))
        }),
    );
    if full_damage == 0 {
        return 0;
    }
    let maximum = arena_emp_bomb_shutdown_ticks().max(0) as u32;
    ((u64::from(maximum) * u64::from(unprotected_damage)) / u64::from(full_damage))
        .min(u64::from(maximum)) as u32
}

/// Per-level bomb explosion radius in pixels. Continuum:
/// `BombExplodePixels * level` (with level ≥ 1). The setting is
/// at short-setting offset 6.
fn bomb_explosion_radius_pixels(level: u8) -> f32 {
    crate::arena_settings::arena_bomb_explode_pixels() * level.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{join_arena, register_connection};
    use crate::authority::WeaponInventory;
    use crate::authority_projectiles::{PlayerHullSnapshot, ProjectileHit, ProjectileRecord};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    fn make_hit(shooter: u16, victim: u16, weapon_data: u16) -> ProjectileHit {
        ProjectileHit {
            projectile_id: 101,
            owner_id: shooter,
            owner_life_seq: 1,
            victim_id: victim,
            victim_life_seq: 1,
            weapon_data,
            x: 100,
            y: 100,
            tick: 0,
        }
    }

    #[test]
    fn zero_authored_explosive_damage_preserves_contact_but_rejects_outside_radius() {
        let mut settings = arena_weapon_damage_settings();
        settings.bomb_damage_level = 0;
        for kind in [3, 4, 8] {
            let hit = make_hit(1, 2, kind | (2 << 5));
            for ship in [0, 5, 6] {
                assert_eq!(
                    unprotected_explosive_contact_damage(
                        &hit,
                        (100.0, 100.0),
                        settings,
                        Some(ship),
                        40.0
                    ),
                    Some(0)
                );
                assert_eq!(
                    unprotected_explosive_contact_damage(
                        &hit,
                        (139.0, 100.0),
                        settings,
                        Some(ship),
                        40.0
                    ),
                    Some(0)
                );
                for anchor in [(140.0, 100.0), (141.0, 100.0), (f32::NAN, 100.0)] {
                    assert_eq!(
                        unprotected_explosive_contact_damage(
                            &hit,
                            anchor,
                            settings,
                            Some(ship),
                            40.0
                        ),
                        None
                    );
                }
            }
        }
    }

    #[test]
    fn shrapnel_damage_uses_exact_fragment_creation_tick_across_clock_wrap() {
        let mut state = ServerState::default();
        let mut hit = make_hit(1, 2, 15 | (2 << 5));
        let created_tick = 0x7fff_fff0;
        let settings = crate::authority_damage::ArenaWeaponDamageSettings {
            bullet_damage_level: 200_000,
            bullet_damage_upgrade: 100_000,
            bomb_damage_level: 750_000,
            burst_damage_level: 700_000,
            inactive_shrapnel_damage: 3_000,
            shrapnel_damage_percent: 500,
            emp_bomb_damage_percent: 1000,
            bouncing_bomb_damage_percent: 900,
        };
        assert_eq!(unprotected_direct_hit_damage(&state, &hit, settings), None);
        state.record_projectile(ProjectileRecord {
            id: hit.projectile_id,
            shot_group_id: 1,
            shot_index: 0,
            presentation_source: None,
            owner_id: hit.owner_id,
            owner_life_seq: hit.owner_life_seq,
            combat_timeline: None,
            weapon_data: hit.weapon_data,
            x: hit.x,
            y: hit.y,
            // The parent may have been fired long before the explosion.
            // Damage must use this fragment's creation time instead.
            timestamp: crate::continuum::continuum_tick_sub(created_tick, 500),
            created_tick,
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: None,
            pos_x: f32::from(hit.x),
            source_server_tick: None,
            precise_velocity: None,
            pos_y: f32::from(hit.y),
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
        for (age, expected) in [(0, 6), (25, 6), (26, 200), (300, 200)] {
            hit.tick = crate::continuum::continuum_tick_add(created_tick, age);
            assert_eq!(
                unprotected_direct_hit_damage(&state, &hit, settings),
                Some(expected)
            );
        }
        hit.tick = crate::continuum::continuum_tick_sub(created_tick, 1);
        assert_eq!(unprotected_direct_hit_damage(&state, &hit, settings), None);
        hit.tick = created_tick;
        hit.owner_life_seq += 1;
        assert_eq!(unprotected_direct_hit_damage(&state, &hit, settings), None);
        hit.owner_life_seq -= 1;
        hit.weapon_data = 15 | (1 << 5);
        assert_eq!(unprotected_direct_hit_damage(&state, &hit, settings), None);
    }

    #[test]
    fn inexact_bullets_roll_but_bombs_and_exact_arenas_do_not() {
        let bullet = make_hit(1, 2, 1);
        // An `ExactDamage` arena keeps the deterministic (maximum) value.
        assert_eq!(bullet_exactness_damage(&bullet, 210, true), 210);
        // Bombs never roll, even when `ExactDamage` is clear.
        assert_eq!(bullet_exactness_damage(&make_hit(1, 2, 3), 750, false), 750);
        assert_eq!(
            bullet_exactness_damage(&make_hit(1, 2, 8), 5600, false),
            5600
        );
        // An inexact arena rolls into [0, damage], and the value is stable for
        // the same hit so a capacity retry cannot change the outcome.
        let rolled = bullet_exactness_damage(&bullet, 210, false);
        assert!(rolled <= 210, "roll {rolled} exceeded the maximum");
        assert_eq!(rolled, bullet_exactness_damage(&bullet, 210, false));
        // Each splash victim owns its own roll.
        let splash = bullet_exactness_damage(&make_hit(1, 3, 1), 210, false);
        assert!(splash <= 210);
        assert_ne!(rolled, splash, "splash victims must not share a roll");
        // Shrapnel (wire type 15) rolls on the same terms as a bullet.
        assert!(bullet_exactness_damage(&make_hit(1, 2, 15), 210, false) <= 210);
    }

    #[test]
    fn thor_damage_does_not_inherit_the_ships_emp_bomb_multiplier() {
        // The reference game's SetWeaponSprite marks only Bomb/ProximityBomb as EMP;
        // a Weasel's Thor remains an ordinary Thor even in a reduced-EMP arena.
        assert!(arena_ship_emp_bomb(5));
        let mut settings = arena_weapon_damage_settings();
        settings.emp_bomb_damage_percent = 500;
        let full = (settings.bomb_damage_level / 1000) as u16;
        for raw_level in 0..=3 {
            let thor = make_hit(1, 2, 8 | (raw_level << 5));
            let expected = full * (1 + raw_level * raw_level);
            assert_eq!(
                bomb_damage_with_falloff_radius(&thor, (100.0, 100.0), settings, Some(5), 320.0,),
                expected,
            );
        }
        let bomb = make_hit(1, 2, 3);
        assert_eq!(
            bomb_damage_with_falloff_radius(&bomb, (100.0, 100.0), settings, Some(5), 80.0),
            full / 2,
            "ordinary EMP bombs must retain the configured multiplier"
        );
    }

    #[test]
    fn thor_falloff_uses_the_inflated_full_damage_and_blast_level() {
        let settings = arena_weapon_damage_settings();
        let plain = settings.bomb_damage_level / 1000;
        // A Thor with raw level 2 scales full damage by `1 + 2^2 = 5`.
        let thor = make_hit(1, 2, 8 | (2 << 5));
        assert_eq!(
            bomb_damage_with_falloff_radius(&thor, (100.0, 100.0), settings, Some(0), 90.0),
            (plain * 5) as u16,
            "a Thor must hit for BombDamageLevel * (1 + raw_level^2)"
        );
        // The same-level plain bomb keeps the configured damage.
        let bomb = make_hit(1, 2, 3 | (2 << 5));
        assert_eq!(
            bomb_damage_with_falloff_radius(&bomb, (100.0, 100.0), settings, Some(0), 90.0),
            plain as u16,
        );
        // Explosion level: bombs use 1..=4; a Thor uses `3 + raw_level`.
        assert_eq!(continuum_weapon_explosion_level(3 | (2 << 5)), 3);
        assert_eq!(continuum_weapon_explosion_level(8), 4);
        assert_eq!(continuum_weapon_explosion_level(8 | (3 << 5)), 7);
    }

    #[test]
    fn committed_damage_loss_uses_server_rng_and_versions_private_state_once() {
        let damage_factor = 30;
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let victim = register_connection(&state, tx);
        join_arena(&state, victim, 0);
        // A maximal focused-test damage makes the chance denominator one;
        // eligibility/lethality are passed separately just as in the staged
        // production transaction.
        let damage = u16::MAX;
        let (seed, expected_id) = (1..=100_000)
            .find_map(|seed| {
                let mut replay = seed;
                crate::authority_prizes::damage_prize_loss_id(&mut replay, damage_factor, damage)
                    .map(|id| (seed, id))
            })
            .expect("a deterministic successful damage-loss seed");
        let mut state = state.lock().expect("state");
        state.damage_prize_rng_seed = seed;
        let client = state.clients.get_mut(&victim).expect("victim");
        client.server_bounty = 3;
        client.server_energy = client.server_max_energy.saturating_sub(1);
        let energy_before = client.server_energy;
        let revision_before = client.server_prize_state_revision;
        let bounty_before = client.server_bounty;

        assert_eq!(
            apply_damage_prize_loss_with_factor(
                &mut state,
                victim,
                u32::MAX - 2,
                77,
                damage,
                energy_before,
                false,
                damage_factor,
            ),
            Some(expected_id)
        );
        let client = &state.clients[&victim];
        assert_eq!(client.last_damage_prize_loss_seq, Some(77));
        assert_eq!(client.last_damage_prize_loss_id, Some(expected_id));
        assert_ne!(client.server_prize_state_revision, revision_before);
        assert_eq!(
            client.server_bounty,
            bounty_before + u16::from(expected_id == 0),
            "only the reference game's selected None prize adds one bounty"
        );
    }

    #[test]
    fn ineligible_damage_loss_does_not_consume_rng_or_publish_a_loss() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let victim = register_connection(&state, tx);
        join_arena(&state, victim, 0);
        let mut state = state.lock().expect("state");
        state.damage_prize_rng_seed = 12345;
        let client = state.clients.get_mut(&victim).expect("victim");
        client.server_bounty = 0;
        let energy_before = client.server_energy;
        assert_eq!(
            apply_damage_prize_loss_with_factor(
                &mut state,
                victim,
                10,
                1,
                500,
                energy_before,
                false,
                30,
            ),
            None
        );
        assert_eq!(state.damage_prize_rng_seed, 12345);
        assert_eq!(state.clients[&victim].last_damage_prize_loss_seq, None);
    }

    fn drain(rx: &Receiver<Vec<u8>>) {
        while rx.try_recv().is_ok() {}
    }

    fn drain_reliable_payload_types(rx: &Receiver<Vec<u8>>) -> Vec<u8> {
        rx.try_iter()
            .filter_map(|packet| crate::reliable::reliable_payload(&packet).map(|body| body[0]))
            .collect()
    }

    fn test_client_position_packet_with_velocity(
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> Vec<u8> {
        let mut packet = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
        packet[0] = crate::continuum_position::CLIENT_POSITION_TYPE;
        packet[1] = 7;
        packet[6..8].copy_from_slice(&velocity_y.to_le_bytes());
        packet[8..10].copy_from_slice(&y.to_le_bytes());
        packet[10..12].copy_from_slice(&velocity_x.to_le_bytes());
        packet[12..14].copy_from_slice(&x.to_le_bytes());
        packet[18..20].copy_from_slice(&900_u16.to_le_bytes());
        crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            7, x, y, velocity_x, velocity_y,
        )
        .encode(&mut packet);
        packet
    }

    fn test_client_position_packet(x: u16, y: u16) -> Vec<u8> {
        test_client_position_packet_with_velocity(x, y, 0, 0)
    }

    fn set_damage_fixture_client(
        state: &mut ServerState,
        player_id: u16,
        x: u16,
        y: u16,
        energy: u16,
    ) {
        let mut position =
            crate::arena::large_position_from_client(player_id, &test_client_position_packet(x, y))
                .expect("fixture position should decode");
        position[21..23].copy_from_slice(&energy.to_le_bytes());
        state.set_client_position(player_id, position);
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.alive = true;
        client.respawn_at_tick = None;
        client.server_max_energy = 1_700;
        client.server_energy = energy;
        client.server_energy_remainder = 0.0;
        client.last_reported_energy = energy;
    }

    fn set_damage_fixture_client_velocity(
        state: &mut ServerState,
        player_id: u16,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
        energy: u16,
    ) {
        let mut position = crate::arena::large_position_from_client(
            player_id,
            &test_client_position_packet_with_velocity(x, y, velocity_x, velocity_y),
        )
        .expect("fixture position should decode");
        position[21..23].copy_from_slice(&energy.to_le_bytes());
        state.set_client_position(player_id, position);
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.alive = true;
        client.respawn_at_tick = None;
        client.server_max_energy = 1_700;
        client.server_energy = energy;
        client.server_energy_remainder = 0.0;
        client.last_reported_energy = energy;
    }

    fn bomb_splash_fixture(
        candidate_current_x: u16,
        hit_tick: u32,
    ) -> (Arc<Mutex<ServerState>>, u16, u16, ProjectileHit, f32) {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let (candidate_tx, _candidate_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let candidate = register_connection(&state, candidate_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, candidate, 0);
        let origin = 2_000;
        let weapon_data = 3 | (2 << 5);
        let radius = bomb_explosion_radius_pixels(continuum_weapon_explosion_level(weapon_data));
        let hit = {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, origin, origin, 1_700);
            set_damage_fixture_client(&mut guard, candidate, candidate_current_x, origin, 1_700);
            guard.clients.get_mut(&shooter).unwrap().player.frequency = 1;
            guard
                .clients
                .get_mut(&shooter)
                .unwrap()
                .lag_compensation_ticks = 5;
            guard.clients.get_mut(&candidate).unwrap().player.frequency = 2;
            ProjectileHit {
                projectile_id: 700,
                owner_id: shooter,
                owner_life_seq: guard.clients[&shooter].life_seq,
                victim_id: shooter,
                victim_life_seq: guard.clients[&shooter].life_seq,
                weapon_data,
                x: origin,
                y: origin,
                tick: hit_tick,
            }
        };
        (state, shooter, candidate, hit, radius)
    }

    fn exact_damage_hull(
        state: &ServerState,
        player_id: u16,
        requested_tick: u32,
        current: (f32, f32),
        exact: (f32, f32),
    ) -> PlayerHullSnapshot {
        let client = state.clients.get(&player_id).expect("fixture client");
        let segment = crate::combat_timeline::MotionSegmentIdentity {
            actor: state
                .active_player_runtime_identity(player_id)
                .expect("active fixture runtime"),
            arena_stream_id: 77,
            discontinuity_generation: 1,
        };
        PlayerHullSnapshot {
            player_id,
            life_seq: client.life_seq,
            x: current.0,
            y: current.1,
            frequency: client.player.frequency,
            radius_px: crate::arena_settings::arena_ship_radius_pixels(client.player.ship),
            motion_segment: Some(segment),
            motion_samples: vec![crate::combat_timeline::ActorMotionSampleV1 {
                segment,
                server_tick: requested_tick,
                sample_time: crate::combat_timeline::PresentationSampleTime::from_tick(
                    requested_tick,
                ),
                x_q8: (exact.0 * 256.0).round() as i32,
                y_q8: (exact.1 * 256.0).round() as i32,
                velocity_x_q8: 0,
                velocity_y_q8: 0,
                orientation_u16: 0,
                protocol_direction: 0,
                hull_radius_q8: (crate::arena_settings::arena_ship_radius_pixels(
                    client.player.ship,
                ) * 256.0)
                    .round() as u16,
                safe_or_attached: 0,
                source_kind: crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
            }],
            motion_source_kind: Some(
                crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
            ),
            lag_compensation_ticks: 0,
            life_started_tick: None,
        }
    }

    fn exact_collision_witness(
        hit: ProjectileHit,
        primary: &PlayerHullSnapshot,
        requested_tick: u32,
        anchor: (f32, f32),
        explosion_radius: Option<f32>,
    ) -> crate::authority_projectiles::ProjectileCollisionWitness {
        crate::authority_projectiles::ProjectileCollisionWitness {
            projectile_id: hit.projectile_id,
            shot_group_id: u64::from(hit.projectile_id),
            owner: hit.owner_identity(),
            target: hit.victim_identity(),
            projectile_created_tick: hit.tick.saturating_sub(10),
            collision_tick: hit.tick,
            requested_target_tick: requested_tick,
            selected_target_tick: Some(requested_tick),
            target_motion_segment: primary.motion_segment,
            rewind_ticks: crate::continuum::continuum_tick_elapsed(hit.tick, requested_tick),
            projectile_x: f32::from(hit.x),
            projectile_y: f32::from(hit.y),
            target_current_x: primary.x,
            target_current_y: primary.y,
            target_anchor_x: anchor.0,
            target_anchor_y: anchor.1,
            center_distance: ((anchor.0 - f32::from(hit.x)).powi(2)
                + (anchor.1 - f32::from(hit.y)).powi(2))
            .sqrt(),
            target_radius: primary.radius_px,
            proximity_trigger_radius: None,
            explosion_radius,
            history_source: crate::authority_projectiles::MotionHistorySource::UnifiedExactSegment,
            contact_reason: crate::authority_projectiles::ProjectileContactReason::Direct,
            presentation_geometry:
                crate::combat_timeline::ProjectilePresentationWitnessV1::server_native(),
        }
    }

    fn fill_realtime_queue(state: &mut ServerState, player_id: u16, event_id: u32) {
        let recipient = state
            .active_player_runtime_identity(player_id)
            .expect("active recipient");
        let envelope = crate::snapshot::PendingRealtimeEventEnvelope {
            flight_transition: false,
            recipient,
            event_id,
            server_tick: event_id,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                crate::realtime_event::RealtimeWeaponEvent {
                    source_player_id: player_id,
                    source_session_id: recipient.session.session_id.get(),
                    source_life_seq: recipient.life.life_seq,
                    shot_group_id: 1,
                    x: 0.0,
                    y: 0.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    orientation: 0.0,
                    frequency: 0,
                    ship: 0,
                    weapon_data: 1,
                },
            ),
        };
        state
            .clients
            .get_mut(&player_id)
            .expect("recipient")
            .pending_realtime_events
            .queue
            .extend(std::iter::repeat_n(
                envelope,
                crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
            ));
    }

    #[test]
    fn full_dedicated_event_queue_defers_damage_atomically_and_retry_applies_once() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let shooter = register_connection(&state, tx_a);
        let victim = register_connection(&state, tx_b);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        let (shooter_life, victim_life, energy_before, damage_seq_before) = {
            let mut guard = state.lock().unwrap();
            set_damage_fixture_client(&mut guard, shooter, 100, 100, 1_700);
            set_damage_fixture_client(&mut guard, victim, 100, 100, 1_700);
            let recipient = guard.active_player_runtime_identity(shooter).unwrap();
            let envelope = crate::snapshot::PendingRealtimeEventEnvelope {
                flight_transition: false,
                recipient,
                event_id: 41,
                server_tick: 41,
                payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                    crate::realtime_event::RealtimeWeaponEvent {
                        source_player_id: shooter,
                        source_session_id: recipient.session.session_id.get(),
                        source_life_seq: recipient.life.life_seq,
                        shot_group_id: 1,
                        x: 100.0,
                        y: 100.0,
                        velocity_x: 0.0,
                        velocity_y: 0.0,
                        orientation: 0.0,
                        frequency: 0,
                        ship: 0,
                        weapon_data: 1,
                    },
                ),
            };
            guard
                .clients
                .get_mut(&shooter)
                .unwrap()
                .pending_realtime_events
                .queue
                .extend(std::iter::repeat_n(
                    envelope,
                    crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                ));
            (
                guard.clients[&shooter].life_seq,
                guard.clients[&victim].life_seq,
                guard.clients[&victim].server_energy,
                guard.next_damage_seq,
            )
        };
        let mut hit = make_hit(shooter, victim, 1);
        hit.owner_life_seq = shooter_life;
        hit.victim_life_seq = victim_life;
        assert!(
            apply_server_detected_hit_with_broadcast(&state, &hit).is_none(),
            "queue pressure rejects before authoritative damage"
        );
        {
            let mut guard = state.lock().unwrap();
            assert_eq!(guard.clients[&victim].server_energy, energy_before);
            assert!(guard.clients[&victim].recent_damagers.is_empty());
            assert_eq!(guard.next_damage_seq, damage_seq_before);
            assert_eq!(guard.realtime_event_queue_overflows, 1);
            assert_eq!(guard.realtime_event_actions_rejected, 0);
            assert_eq!(guard.realtime_event_actions_deferred, 1);
            assert_eq!(
                guard.clients[&shooter]
                    .pending_realtime_events
                    .queue
                    .front()
                    .unwrap()
                    .event_id,
                41,
                "pressure never evicts the oldest committed fact"
            );
            guard
                .clients
                .get_mut(&shooter)
                .unwrap()
                .pending_realtime_events
                .queue
                .clear();
        }
        let applied = apply_server_detected_hit_with_broadcast(&state, &hit)
            .expect("same detected hit applies after capacity drains");
        assert_eq!(
            state.lock().unwrap().clients[&victim].server_energy,
            applied.energy_after
        );
    }

    #[test]
    fn shield_fraction_and_damage_rng_are_frozen_across_capacity_retry_and_wrap() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let shooter = register_connection(&state, tx_a);
        let victim = register_connection(&state, tx_b);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        let hit_tick = crate::continuum::CONTINUUM_TICK_MASK - 10;
        let lifetime = arena_ship_shield_lifetime_ticks(0);
        let remaining = lifetime / 2;
        let mut hit = make_hit(shooter, victim, 1);
        hit.tick = hit_tick;
        let expected_damage = {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, 100, 100, 1_700);
            set_damage_fixture_client(&mut guard, victim, 100, 100, 1_000);
            {
                let victim_client = guard.clients.get_mut(&victim).expect("victim");
                victim_client.server_bounty = 1;
                victim_client.server_shield_until_tick =
                    Some(crate::continuum::continuum_tick_add(hit_tick, remaining));
            }
            guard.damage_prize_rng_seed = 3;
            fill_realtime_queue(&mut guard, shooter, 1_001);
            let (_, unprotected) = preview_server_detected_hit(&guard, &hit).expect("damage");
            let shield =
                authoritative_shield_damage(&guard.clients[&victim], hit_tick, unprotected);
            assert_eq!(shield.remaining_ticks, remaining);
            assert!(shield.damage > 0 && shield.damage < 400);
            shield.damage
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::DeferredCapacity
        );
        let frozen_seed_after = {
            let mut guard = state.lock().expect("state");
            let transaction = guard
                .pending_damage_transactions
                .get(&hit.projectile_id)
                .expect("frozen transaction");
            assert_eq!(transaction.hits[0].damage, expected_damage);
            assert_eq!(transaction.hits[0].shield_remaining_ticks, remaining);
            assert_eq!(guard.damage_prize_rng_seed, 3, "defer cannot commit RNG");
            let seed_after = stage_prepared_damage_transaction(&guard, transaction)
                .expect("frozen transaction stages")
                .2;
            guard
                .clients
                .get_mut(&victim)
                .unwrap()
                .server_shield_until_tick = None;
            guard
                .clients
                .get_mut(&shooter)
                .unwrap()
                .pending_realtime_events
                .queue
                .clear();
            seed_after
        };
        let event = apply_server_detected_hit_with_broadcast(&state, &hit).expect("retry applies");
        assert_eq!(event.damage, expected_damage);
        assert_eq!(event.unprotected_damage, 200);
        assert_eq!(event.shield_remaining_ticks, remaining);
        assert_eq!(
            state.lock().unwrap().damage_prize_rng_seed,
            frozen_seed_after
        );

        let guard = state.lock().expect("state");
        let victim_client = &guard.clients[&victim];
        assert_eq!(
            authoritative_shield_damage(victim_client, hit_tick, 200).damage,
            200,
            "the live timer changed, but the committed transaction did not"
        );
    }

    #[test]
    fn shield_fraction_has_exact_full_partial_and_expiry_boundaries() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let victim = register_connection(&state, tx);
        join_arena(&state, victim, 0);
        let tick = crate::continuum::CONTINUUM_TICK_MASK - 2;
        let lifetime = arena_ship_shield_lifetime_ticks(0);
        let mut guard = state.lock().expect("state");
        let client = guard.clients.get_mut(&victim).expect("victim");

        client.server_shield_until_tick =
            Some(crate::continuum::continuum_tick_add(tick, lifetime));
        assert_eq!(authoritative_shield_damage(client, tick, 201).damage, 0);

        client.server_shield_until_tick =
            Some(crate::continuum::continuum_tick_add(tick, lifetime / 2));
        let partial = authoritative_shield_damage(client, tick, 201);
        assert_eq!(partial.remaining_ticks, lifetime / 2);
        assert_eq!(
            partial.damage, 101,
            "protected portion truncates before subtract"
        );

        let expiry = client.server_shield_until_tick.expect("deadline");
        let expired = authoritative_shield_damage(client, expiry, 201);
        assert_eq!(expired.damage, 201);
        assert_eq!(expired.until_tick, None);
    }

    #[test]
    fn flagger_damage_after_carry_mirrors_the_client_multiplier() {
        // Extreme ships FlaggerDamagePercent=800, and the client truncates.
        assert_eq!(flagger_damage_after_carry(200, 0.8, true), 160);
        assert_eq!(
            flagger_damage_after_carry(11, 0.8, true),
            8,
            "truncates toward zero like the client"
        );
        assert_eq!(flagger_damage_after_carry(200, 1.0, true), 200);
        assert_eq!(flagger_damage_after_carry(200, 0.0, true), 0);
        assert_eq!(
            flagger_damage_after_carry(200, -1.0, true),
            0,
            "a negative setting cannot heal"
        );
        assert_eq!(
            flagger_damage_after_carry(u16::MAX, 8.0, true),
            u16::MAX,
            "the scaled damage saturates instead of wrapping"
        );
        assert_eq!(
            flagger_damage_after_carry(200, 0.8, false),
            200,
            "a player who is not carrying is unaffected"
        );
    }

    #[test]
    fn flag_carrier_damage_uses_the_settings_flagger_multiplier() {
        // The bundled Chaos profile ships a neutral FlaggerDamagePercent of
        // 1000 and the active settings profile is a process global, so the
        // thread-local override is what makes Extreme's 800 observable here
        // without perturbing sibling damage tests.
        let _flagger = crate::arena_settings::FlaggerDamagePercentOverride::set(800);
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let (tx_carrier, _rx_carrier) = mpsc::channel();
        let (tx_control, _rx_control) = mpsc::channel();
        let shooter = register_connection(&state, tx_shooter);
        let carrier = register_connection(&state, tx_carrier);
        let control = register_connection(&state, tx_control);
        for id in [shooter, carrier, control] {
            join_arena(&state, id, 0);
        }
        let tick = crate::wire::current_tick();
        {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, 100, 100, 1_700);
            set_damage_fixture_client(&mut guard, carrier, 100, 100, 1_000);
            set_damage_fixture_client(&mut guard, control, 100, 100, 1_000);
            let carrier_runtime = guard
                .active_player_runtime_identity(carrier)
                .expect("carrier runtime");
            guard
                .objectives
                .reset(crate::objectives::synthetic_profile(1, 0))
                .expect("synthetic flag profile");
            guard
                .objectives
                .create_flag(crate::objectives::FlagState::Carried {
                    carrier: carrier_runtime,
                    x: 704,
                    y: 832,
                    owner: None,
                })
                .expect("carried flag");
        }

        let mut carrier_hit = make_hit(shooter, carrier, 1);
        carrier_hit.tick = tick;
        let mut control_hit = make_hit(shooter, control, 1);
        control_hit.tick = tick;

        let carrier_event = apply_server_detected_hit_with_broadcast(&state, &carrier_hit)
            .expect("carrier applies");
        let control_event = apply_server_detected_hit_with_broadcast(&state, &control_hit)
            .expect("control applies");

        assert_eq!(
            carrier_event.unprotected_damage, control_event.unprotected_damage,
            "both victims take the same raw weapon damage"
        );
        assert_eq!(
            control_event.damage, control_event.unprotected_damage,
            "the unshielded non-carrier takes the raw damage"
        );
        assert_eq!(
            carrier_event.damage,
            (f32::from(control_event.damage) * 0.8).trunc() as u16,
            "the flag carrier takes the configured flagger multiplier"
        );
        assert_ne!(carrier_event.damage, control_event.damage);
    }

    #[test]
    fn damage_equality_is_nonlethal_and_none_fullcharge_follows_damage_without_overwrite() {
        let seed = (1..250_000)
            .find(|candidate| {
                let mut replay = *candidate;
                prepare_damage_prize_loss_selection(&mut replay, 30, 200)
                    == Some(PreparedDamagePrizeLoss {
                        prize_id: 0,
                        replacement_prize_id: Some(13),
                    })
            })
            .expect("fixture seed selecting None -> FullCharge");
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let shooter = register_connection(&state, tx_a);
        let victim = register_connection(&state, tx_b);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, 100, 100, 1_700);
            set_damage_fixture_client(&mut guard, victim, 100, 100, 200);
            let victim_client = guard.clients.get_mut(&victim).expect("victim");
            victim_client.server_bounty = 1;
            victim_client.server_max_energy = 1_700;
            guard.damage_prize_rng_seed = seed;
        }
        let event = apply_server_detected_hit_with_broadcast(&state, &make_hit(shooter, victim, 1))
            .expect("damage");
        assert_eq!(event.damage, 200);
        assert!(
            !event.lethal,
            "energy == damage is nonlethal in the reference game"
        );
        assert_eq!(
            event.energy_after, 1_500,
            "the selected FullCharge replacement applies first, then the hit subtracts damage"
        );
        let guard = state.lock().expect("state");
        assert_eq!(guard.clients[&victim].last_damage_prize_loss_id, Some(0));
        assert!(guard.clients[&victim].alive);
    }

    #[test]
    fn far_away_pressured_client_cannot_suppress_unobserved_combat() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let (tx_far, _rx_far) = mpsc::channel();
        let shooter = register_connection(&state, tx_a);
        let victim = register_connection(&state, tx_b);
        let far = register_connection(&state, tx_far);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        join_arena(&state, far, 0);
        let (shooter_life, victim_life, far_queue_len) = {
            let mut guard = state.lock().unwrap();
            set_damage_fixture_client(&mut guard, shooter, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, victim, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, far, 15_000, 15_000, 1_700);
            guard.clients.get_mut(&victim).unwrap().player.frequency = 2;
            guard.clients.get_mut(&far).unwrap().player.frequency = 3;
            let far_runtime = guard.active_player_runtime_identity(far).unwrap();
            let envelope = crate::snapshot::PendingRealtimeEventEnvelope {
                flight_transition: false,
                recipient: far_runtime,
                event_id: 55,
                server_tick: 55,
                payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                    crate::realtime_event::RealtimeWeaponEvent {
                        source_player_id: shooter,
                        source_session_id: guard.clients[&shooter].session_id.get(),
                        source_life_seq: guard.clients[&shooter].life_seq,
                        shot_group_id: 1,
                        x: 1_000.0,
                        y: 1_000.0,
                        velocity_x: 0.0,
                        velocity_y: 0.0,
                        orientation: 0.0,
                        frequency: guard.clients[&shooter].player.frequency,
                        ship: guard.clients[&shooter].player.ship,
                        weapon_data: 1,
                    },
                ),
            };
            guard
                .clients
                .get_mut(&far)
                .unwrap()
                .pending_realtime_events
                .queue
                .extend(std::iter::repeat_n(
                    envelope,
                    crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                ));
            (
                guard.clients[&shooter].life_seq,
                guard.clients[&victim].life_seq,
                guard.clients[&far].pending_realtime_events.queue.len(),
            )
        };
        let mut hit = make_hit(shooter, victim, 1);
        hit.owner_life_seq = shooter_life;
        hit.victim_life_seq = victim_life;
        let applied = apply_server_detected_hit_with_broadcast(&state, &hit)
            .expect("a non-recipient's pressure must not block exact AOI combat");
        let guard = state.lock().unwrap();
        assert_eq!(guard.clients[&victim].server_energy, applied.energy_after);
        assert_eq!(
            guard.clients[&far].pending_realtime_events.queue.len(),
            far_queue_len,
            "unrelated observer was neither reserved nor mutated",
        );
        assert_eq!(guard.realtime_event_queue_overflows, 0);
    }

    #[test]
    fn lethal_damage_uses_one_frozen_recipient_set_with_or_without_pressure() {
        for pressured in [false, true] {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (tx_shooter, _rx_shooter) = mpsc::channel();
            let (tx_victim, _rx_victim) = mpsc::channel();
            let (tx_far, _rx_far) = mpsc::channel();
            let shooter = register_connection(&state, tx_shooter);
            let victim = register_connection(&state, tx_victim);
            let far = register_connection(&state, tx_far);
            join_arena(&state, shooter, 0);
            join_arena(&state, victim, 0);
            join_arena(&state, far, 0);
            let (shooter_life, victim_life) = {
                let mut guard = state.lock().expect("state");
                set_damage_fixture_client(&mut guard, shooter, 1_000, 1_000, 1_700);
                set_damage_fixture_client(&mut guard, victim, 1_000, 1_000, 1);
                set_damage_fixture_client(&mut guard, far, 15_000, 15_000, 1_700);
                guard.clients.get_mut(&victim).unwrap().player.frequency = 2;
                guard.clients.get_mut(&far).unwrap().player.frequency = 3;
                if pressured {
                    fill_realtime_queue(&mut guard, shooter, 900);
                }
                (
                    guard.clients[&shooter].life_seq,
                    guard.clients[&victim].life_seq,
                )
            };
            let mut hit = make_hit(shooter, victim, 1);
            hit.owner_life_seq = shooter_life;
            hit.victim_life_seq = victim_life;
            if pressured {
                assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
                let mut guard = state.lock().expect("state");
                assert!(guard.clients[&victim].alive);
                assert_eq!(guard.clients[&victim].server_energy, 1);
                guard
                    .clients
                    .get_mut(&shooter)
                    .unwrap()
                    .pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
            }
            let event = apply_server_detected_hit_with_broadcast(&state, &hit)
                .expect("frozen lethal transaction commits without recipient parity panic");
            assert!(event.lethal);
            let guard = state.lock().expect("state");
            assert!(!guard.clients[&victim].alive);
            assert_eq!(guard.clients[&far].pending_realtime_events.queue.len(), 0);
            assert_eq!(
                guard.clients[&shooter].pending_realtime_events.queue.len(),
                1
            );
            assert_eq!(
                guard.clients[&victim].pending_realtime_events.queue.len(),
                1
            );
        }
    }

    #[test]
    fn deferred_bomb_keeps_collision_time_targets_damage_and_aoi_after_movement() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut receivers = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..5 {
            let (tx, rx) = mpsc::channel();
            ids.push(register_connection(&state, tx));
            receivers.push(rx);
        }
        let [shooter, primary, splash, far, late] = ids.as_slice() else {
            unreachable!()
        };
        for &player_id in &ids {
            join_arena(&state, player_id, 0);
        }
        let hit = {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, *shooter, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, *primary, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, *splash, 1_010, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, *far, 15_000, 15_000, 1_700);
            set_damage_fixture_client(&mut guard, *late, 15_000, 15_000, 1_700);
            for (frequency, player_id) in [*primary, *splash, *far, *late].into_iter().enumerate() {
                guard.clients.get_mut(&player_id).unwrap().player.frequency = frequency as u16 + 2;
            }
            fill_realtime_queue(&mut guard, *shooter, 901);
            ProjectileHit {
                projectile_id: 404,
                owner_id: *shooter,
                owner_life_seq: guard.clients[shooter].life_seq,
                victim_id: *primary,
                victim_life_seq: guard.clients[primary].life_seq,
                weapon_data: 3 | (2 << 5),
                x: 1_000,
                y: 1_000,
                tick: 500,
            }
        };
        assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
        let (frozen, expected_energy) = {
            let mut guard = state.lock().expect("state");
            let frozen = guard.pending_damage_transactions[&hit.projectile_id].clone();
            assert!(
                frozen.hits.len() >= 2,
                "primary and captured splash target must both be frozen"
            );
            let expected_energy = stage_prepared_damage_transaction(&guard, &frozen)
                .expect("frozen targets stage from the collision-time predecessor")
                .0
                .into_iter()
                .map(|staged| (staged.prepared.hit.victim_id, staged.energy_after))
                .collect::<std::collections::BTreeMap<_, _>>();
            set_damage_fixture_client(&mut guard, *primary, 15_000, 15_000, 1_700);
            set_damage_fixture_client(&mut guard, *splash, 15_000, 15_000, 1_700);
            set_damage_fixture_client(&mut guard, *late, 1_000, 1_000, 1_700);
            guard
                .clients
                .get_mut(shooter)
                .unwrap()
                .pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
            (frozen, expected_energy)
        };
        let applied = apply_server_detected_hit_with_broadcast(&state, &hit)
            .expect("same immutable bomb transaction applies after drain");
        assert_eq!(applied.victim_id, *primary);
        let guard = state.lock().expect("state");
        for prepared in &frozen.hits {
            assert_eq!(
                guard.clients[&prepared.hit.victim_id].server_energy,
                expected_energy[&prepared.hit.victim_id],
                "movement/history rollover cannot alter frozen damage"
            );
        }
        assert_eq!(guard.clients[late].server_energy, 1_700);
        assert_eq!(guard.clients[far].pending_realtime_events.queue.len(), 0);
        assert!(
            !guard
                .pending_damage_transactions
                .contains_key(&hit.projectile_id)
        );
        drop(receivers);
    }

    #[test]
    fn production_bomb_damage_uses_only_frozen_exact_collision_geometry() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut ids = Vec::new();
        for _ in 0..5 {
            let (tx, _rx) = mpsc::channel();
            let id = register_connection(&state, tx);
            join_arena(&state, id, 0);
            ids.push(id);
        }
        let [shooter, primary, splash, current_only, unavailable] = ids.as_slice() else {
            unreachable!()
        };
        let tick = 4_000;
        let requested_tick = 3_995;
        let origin = (2_000.0, 2_000.0);
        let hit = {
            let mut guard = state.lock().expect("state");
            for (&id, (x, y, frequency)) in ids.iter().zip([
                (8_000, 8_000, 1),
                (12_000, 12_000, 2),
                (13_000, 13_000, 3),
                (2_001, 2_000, 4),
                (2_002, 2_000, 5),
            ]) {
                set_damage_fixture_client(&mut guard, id, x, y, 1_700);
                guard.clients.get_mut(&id).unwrap().player.frequency = frequency;
            }
            ProjectileHit {
                projectile_id: 9_001,
                owner_id: *shooter,
                owner_life_seq: guard.clients[shooter].life_seq,
                victim_id: *primary,
                victim_life_seq: guard.clients[primary].life_seq,
                weapon_data: 3 | (2 << 5),
                x: origin.0 as u16,
                y: origin.1 as u16,
                tick,
            }
        };

        let guard = state.lock().expect("state");
        let shooter_hull = exact_damage_hull(
            &guard,
            *shooter,
            requested_tick,
            (8_000.0, 8_000.0),
            (8_000.0, 8_000.0),
        );
        let primary_hull = exact_damage_hull(
            &guard,
            *primary,
            requested_tick,
            (12_000.0, 12_000.0),
            origin,
        );
        let authored_radius =
            bomb_explosion_radius_pixels(continuum_weapon_explosion_level(hit.weapon_data));
        let frozen_radius = authored_radius + 80.0;
        let splash_anchor = (origin.0 + authored_radius + 20.0, origin.1);
        let splash_hull = exact_damage_hull(
            &guard,
            *splash,
            requested_tick,
            (13_000.0, 13_000.0),
            splash_anchor,
        );
        // This actor's mutable roster position is inside the explosion, but it
        // is intentionally absent from the authoritative per-tick hull set
        // (the production safe/attached omission shape). It must not be hit.
        assert_eq!(guard.clients[current_only].player.frequency, 4);
        let unavailable_hull = exact_damage_hull(
            &guard,
            *unavailable,
            requested_tick.wrapping_add(1),
            (2_002.0, 2_000.0),
            (2_002.0, 2_000.0),
        );
        // Keep its requested tick before the only exact sample. Exact-history
        // failure must close the splash path instead of substituting current
        // data.
        let hulls = vec![
            shooter_hull,
            primary_hull.clone(),
            splash_hull.clone(),
            unavailable_hull,
        ];
        let collision = exact_collision_witness(
            hit,
            &primary_hull,
            requested_tick,
            origin,
            Some(frozen_radius),
        );
        let mut metrics = crate::authority_projectiles::PresentationGeometryMetrics::default();
        let previews = preview_detected_damage_hits_from_collision(
            &guard,
            &hit,
            collision,
            &hulls,
            &mut metrics,
        )
        .expect("exact production geometry");
        assert_eq!(
            metrics.lookups_total, 0,
            "server-native splash needs no observer lookup"
        );
        assert_eq!(
            previews
                .iter()
                .map(|preview| preview.hit.victim_id)
                .collect::<std::collections::BTreeSet<_>>(),
            [*primary, *splash].into_iter().collect(),
            "only exact per-tick hull geometry may select damage victims"
        );
        let transaction = prepare_server_detected_damage_transaction_from_previews(
            &guard,
            &hit,
            Some(collision),
            previews,
        )
        .expect("frozen production transaction");
        let witnesses = transaction.collision_damage_witnesses();
        let primary_witness = witnesses
            .iter()
            .find(|witness| witness.target.player_id == *primary)
            .expect("primary witness");
        assert!(primary_witness.primary);
        assert_eq!(
            primary_witness.target_motion_segment,
            primary_hull.motion_segment
        );
        assert_eq!(primary_witness.selected_target_tick, Some(requested_tick));
        assert_eq!(
            primary_witness.history_source,
            crate::authority_projectiles::MotionHistorySource::UnifiedExactSegment
        );
        assert_eq!(
            primary_witness.damage_anchor_x.to_bits(),
            origin.0.to_bits()
        );
        let splash_witness = witnesses
            .iter()
            .find(|witness| witness.target.player_id == *splash)
            .expect("splash witness");
        assert!(!splash_witness.primary);
        assert_eq!(
            splash_witness.target_motion_segment,
            splash_hull.motion_segment
        );
        assert_eq!(splash_witness.selected_target_tick, Some(requested_tick));
        assert_eq!(
            splash_witness.damage_anchor_x.to_bits(),
            splash_anchor.0.to_bits()
        );
        assert_eq!(
            bomb_damage_with_falloff(
                &hit,
                splash_anchor,
                arena_weapon_damage_settings(),
                continuum_weapon_level(hit.weapon_data),
                Some(guard.clients[shooter].player.ship),
            ),
            0,
            "a later authored-radius reconstruction would incorrectly erase this frozen hit"
        );
        let expected_splash = bomb_damage_with_falloff_radius(
            &hit,
            splash_anchor,
            arena_weapon_damage_settings(),
            Some(guard.clients[shooter].player.ship),
            frozen_radius,
        );
        assert!(expected_splash > 0);
        assert_eq!(splash_witness.unprotected_damage, expected_splash);
    }

    #[test]
    fn splash_projection_attempts_are_measured_even_when_history_is_unavailable() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut ids = Vec::new();
        for _ in 0..2 {
            let (tx, _rx) = mpsc::channel();
            let id = register_connection(&state, tx);
            join_arena(&state, id, 0);
            ids.push(id);
        }
        let (shooter, bystander) = (ids[0], ids[1]);
        let mut guard = state.lock().expect("state");
        for (id, x, frequency) in [(shooter, 2_000, 1), (bystander, 2_020, 2)] {
            set_damage_fixture_client(&mut guard, id, x, 2_000, 1_700);
            guard.clients.get_mut(&id).unwrap().player.frequency = frequency;
        }
        let tick = 4_000;
        let binding = crate::combat_timeline::ProjectileCombatTimelineV1 {
            owner: guard.active_player_runtime_identity(shooter).unwrap(),
            owner_input_sequence: 1,
            accepted_timeline_generation: 1,
            accepted_game_applied_snapshot_id: 1,
            accepted_server_tick: tick,
            maximum_rewind_ticks: 7,
        };
        let hit = ProjectileHit {
            projectile_id: 9_003,
            owner_id: shooter,
            owner_life_seq: binding.owner.life.life_seq,
            victim_id: shooter,
            victim_life_seq: binding.owner.life.life_seq,
            weapon_data: 3,
            x: 2_000,
            y: 2_000,
            tick,
        };
        guard.active_projectiles.record(
            ProjectileRecord {
                id: hit.projectile_id,
                shot_group_id: u64::from(hit.projectile_id),
                shot_index: 0,
                presentation_source: None,
                owner_id: shooter,
                owner_life_seq: hit.owner_life_seq,
                combat_timeline: Some(binding),
                weapon_data: hit.weapon_data,
                x: hit.x,
                y: hit.y,
                timestamp: tick,
                created_tick: tick,
                velocity_x: 0,
                velocity_y: 0,
                expiry_tick: None,
                pos_x: 2_000.0,
                source_server_tick: None,
                precise_velocity: None,
                pos_y: 2_000.0,
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            },
            128,
            1_024,
        );
        let primary = exact_damage_hull(
            &guard,
            shooter,
            tick,
            (2_000.0, 2_000.0),
            (2_000.0, 2_000.0),
        );
        let secondary = exact_damage_hull(
            &guard,
            bystander,
            tick,
            (2_020.0, 2_000.0),
            (2_020.0, 2_000.0),
        );
        let mut collision =
            exact_collision_witness(hit, &primary, tick, (2_000.0, 2_000.0), Some(160.0));
        collision.history_source = crate::authority_projectiles::MotionHistorySource::LocalSelf;
        collision.presentation_geometry =
            crate::combat_timeline::ProjectilePresentationWitnessV1::local_self(
                binding,
                tick,
                2_000 * 256,
                2_000 * 256,
            );
        let mut metrics = crate::authority_projectiles::PresentationGeometryMetrics::default();
        let previews = preview_detected_damage_hits_from_collision(
            &guard,
            &hit,
            collision,
            &[primary, secondary],
            &mut metrics,
        )
        .unwrap();
        assert_eq!(
            previews.len(),
            1,
            "missing observer history must not damage the bystander"
        );
        assert_eq!(
            metrics.lookups_total, 1,
            "secondary projection was attempted"
        );
        assert_eq!(metrics.unavailable_total, 1);
        assert_eq!(metrics.resolved_total, 0);
        assert_eq!(metrics.eval_nanos_max, metrics.eval_nanos_total);
        guard
            .active_projectiles
            .record_presentation_geometry_metrics(metrics);
        assert_eq!(
            guard.active_projectiles.presentation_geometry_metrics(),
            metrics
        );
    }

    #[test]
    fn production_damage_rejects_non_exact_primary_collision_witness() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let (primary_tx, _primary_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let primary = register_connection(&state, primary_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, primary, 0);
        let hit = {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, primary, 2_000, 2_000, 1_700);
            guard.clients.get_mut(&shooter).unwrap().player.frequency = 1;
            guard.clients.get_mut(&primary).unwrap().player.frequency = 2;
            ProjectileHit {
                projectile_id: 9_002,
                owner_id: shooter,
                owner_life_seq: guard.clients[&shooter].life_seq,
                victim_id: primary,
                victim_life_seq: guard.clients[&primary].life_seq,
                weapon_data: 1,
                x: 2_000,
                y: 2_000,
                tick: 5_000,
            }
        };
        let guard = state.lock().expect("state");
        let hull = exact_damage_hull(
            &guard,
            primary,
            hit.tick,
            (2_000.0, 2_000.0),
            (2_000.0, 2_000.0),
        );
        let mut collision = exact_collision_witness(hit, &hull, hit.tick, (2_000.0, 2_000.0), None);
        collision.history_source =
            crate::authority_projectiles::MotionHistorySource::UnifiedHistoryUnavailable;
        assert!(
            preview_detected_damage_hits_from_collision(
                &guard,
                &hit,
                collision,
                &[hull],
                &mut Default::default()
            )
            .is_none(),
            "production damage must fail closed instead of accepting a current-position fallback"
        );
    }

    #[test]
    fn splash_uses_rewound_inside_anchor_when_current_is_outside_and_retry_never_recomputes() {
        let hit_tick = 1_000;
        let origin = 2_000.0;
        let radius = bomb_explosion_radius_pixels(continuum_weapon_explosion_level(3 | (2 << 5)));
        let current_x = (origin + radius + 50.0) as u16;
        let (state, shooter, candidate, hit, fixture_radius) =
            bomb_splash_fixture(current_x, hit_tick);
        assert_eq!(fixture_radius, radius);
        let target_tick = crate::continuum::continuum_tick_sub(hit_tick, 5);
        let rewound_x = origin + radius * 0.5;
        {
            let mut guard = state.lock().expect("state");
            let client = guard.clients.get_mut(&candidate).expect("candidate");
            client.position_history.clear();
            client
                .position_history
                .push_back(crate::state::RecentPositionSample {
                    tick: target_tick,
                    x: rewound_x,
                    y: origin,
                });
            fill_realtime_queue(&mut guard, shooter, 1_001);
        }

        assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
        let (frozen_damage, expected_energy) = {
            let mut guard = state.lock().expect("state");
            let prepared_damage = {
                let frozen = &guard.pending_damage_transactions[&hit.projectile_id];
                let prepared = frozen
                    .hits
                    .iter()
                    .find(|prepared| prepared.hit.victim_id == candidate)
                    .expect("rewound-inside candidate must be selected");
                assert_eq!(
                    prepared.geometry.target_anchor_x.to_bits(),
                    rewound_x.to_bits()
                );
                assert_eq!(
                    prepared.geometry.target_anchor_y.to_bits(),
                    origin.to_bits()
                );
                prepared.damage
            };
            let settings = arena_weapon_damage_settings();
            let exact_rewound_damage = bomb_damage_with_falloff(
                &hit,
                (rewound_x, origin),
                settings,
                continuum_weapon_level(hit.weapon_data),
                Some(guard.clients[&shooter].player.ship),
            );
            let wrong_current_damage = bomb_damage_with_falloff(
                &hit,
                (f32::from(current_x), origin),
                settings,
                continuum_weapon_level(hit.weapon_data),
                Some(guard.clients[&shooter].player.ship),
            );
            assert_eq!(prepared_damage, exact_rewound_damage);
            assert_ne!(prepared_damage, wrong_current_damage);
            let expected_energy = 1_700_u16.saturating_sub(prepared_damage);

            set_damage_fixture_client(&mut guard, candidate, 15_000, 15_000, 1_700);
            let client = guard.clients.get_mut(&candidate).expect("candidate");
            client.position_history.clear();
            client
                .position_history
                .push_back(crate::state::RecentPositionSample {
                    tick: target_tick,
                    x: 15_000.0,
                    y: 15_000.0,
                });
            guard
                .clients
                .get_mut(&shooter)
                .expect("shooter")
                .pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
            (prepared_damage, expected_energy)
        };

        apply_server_detected_hit_with_broadcast(&state, &hit)
            .expect("pressure retry uses frozen rewound transaction");
        let guard = state.lock().expect("state");
        assert!(frozen_damage > 0);
        assert_eq!(guard.clients[&candidate].server_energy, expected_energy);
    }

    #[test]
    fn splash_rejects_rewound_outside_and_exact_boundary_despite_current_inside() {
        let hit_tick = 2_000;
        let origin = 2_000.0;
        let radius = bomb_explosion_radius_pixels(continuum_weapon_explosion_level(3 | (2 << 5)));
        let (state, _shooter, candidate, hit, _) =
            bomb_splash_fixture((origin + 1.0) as u16, hit_tick);
        let target_tick = crate::continuum::continuum_tick_sub(hit_tick, 5);
        let assert_candidate = |anchor_x: f32, expected: bool| {
            let mut guard = state.lock().expect("state");
            let client = guard.clients.get_mut(&candidate).expect("candidate");
            client.position_history.clear();
            client
                .position_history
                .push_back(crate::state::RecentPositionSample {
                    tick: target_tick,
                    x: anchor_x,
                    y: origin,
                });
            let transaction = prepare_server_detected_damage_transaction(&guard, &hit)
                .expect("primary self-bomb remains valid");
            assert_eq!(
                transaction
                    .hits
                    .iter()
                    .any(|prepared| prepared.hit.victim_id == candidate),
                expected
            );
        };
        assert_candidate(origin + radius + 1.0, false);
        assert_candidate(origin + radius, false);
        assert_candidate(origin + radius - 1.0, true);
    }

    #[test]
    fn splash_anchor_history_selection_is_wrap_safe_and_missing_history_falls_back_to_current() {
        let hit_tick = 2;
        let origin = 2_000.0;
        let radius = bomb_explosion_radius_pixels(continuum_weapon_explosion_level(3 | (2 << 5)));
        let current_x = (origin + radius + 20.0) as u16;
        let (state, _shooter, candidate, hit, _) = bomb_splash_fixture(current_x, hit_tick);
        let target_tick = crate::continuum::continuum_tick_sub(hit_tick, 5);
        let wrapped_inside_x = origin + radius * 0.25;
        {
            let mut guard = state.lock().expect("state");
            let client = guard.clients.get_mut(&candidate).expect("candidate");
            client.position_history.clear();
            client
                .position_history
                .push_back(crate::state::RecentPositionSample {
                    tick: crate::continuum::continuum_tick_add(target_tick, 1),
                    x: origin + radius + 10.0,
                    y: origin,
                });
            client
                .position_history
                .push_back(crate::state::RecentPositionSample {
                    tick: crate::continuum::continuum_tick_sub(target_tick, 1),
                    x: wrapped_inside_x,
                    y: origin,
                });
            let transaction = prepare_server_detected_damage_transaction(&guard, &hit)
                .expect("wrapped history transaction");
            let prepared = transaction
                .hits
                .iter()
                .find(|prepared| prepared.hit.victim_id == candidate)
                .expect("latest non-future wrapped sample is inside");
            assert_eq!(
                prepared.geometry.target_anchor_x.to_bits(),
                wrapped_inside_x.to_bits()
            );

            set_damage_fixture_client(
                &mut guard,
                candidate,
                (origin + radius * 0.5) as u16,
                origin as u16,
                1_700,
            );
            guard
                .clients
                .get_mut(&candidate)
                .expect("candidate")
                .position_history
                .clear();
            let transaction = prepare_server_detected_damage_transaction(&guard, &hit)
                .expect("missing history falls back to current position");
            let prepared = transaction
                .hits
                .iter()
                .find(|prepared| prepared.hit.victim_id == candidate)
                .expect("current-inside fallback is eligible");
            assert_eq!(
                prepared.geometry.target_anchor_x.to_bits(),
                f32::from((origin + radius * 0.5) as u16).to_bits()
            );
        }
    }

    #[test]
    fn deferred_damage_cancels_atomically_when_a_target_lifecycle_changes() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let (tx_victim, _rx_victim) = mpsc::channel();
        let shooter = register_connection(&state, tx_shooter);
        let victim = register_connection(&state, tx_victim);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        let hit = {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, victim, 1_000, 1_000, 1_700);
            guard.clients.get_mut(&victim).unwrap().player.frequency = 2;
            fill_realtime_queue(&mut guard, shooter, 902);
            ProjectileHit {
                projectile_id: 405,
                owner_id: shooter,
                owner_life_seq: guard.clients[&shooter].life_seq,
                victim_id: victim,
                victim_life_seq: guard.clients[&victim].life_seq,
                weapon_data: 1,
                x: 1_000,
                y: 1_000,
                tick: 501,
            }
        };
        assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
        {
            let mut guard = state.lock().expect("state");
            guard.advance_client_life(victim).expect("replacement life");
            guard
                .clients
                .get_mut(&shooter)
                .unwrap()
                .pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
        }
        assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
        let guard = state.lock().expect("state");
        assert_eq!(guard.clients[&victim].server_energy, 1_700);
        assert!(
            !guard
                .pending_damage_transactions
                .contains_key(&hit.projectile_id)
        );
    }

    #[test]
    fn deferred_damage_cancels_before_reused_player_id_session_can_be_mutated() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let (tx_victim, _rx_victim) = mpsc::channel();
        let shooter = register_connection(&state, tx_shooter);
        let victim = register_connection(&state, tx_victim);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        let (hit, victim_runtime) = {
            let mut guard = state.lock().expect("state");
            set_damage_fixture_client(&mut guard, shooter, 1_000, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, victim, 1_000, 1_000, 1_700);
            guard.clients.get_mut(&victim).unwrap().player.frequency = 2;
            fill_realtime_queue(&mut guard, shooter, 903);
            let runtime = guard
                .active_player_runtime_identity(victim)
                .expect("victim runtime");
            (
                ProjectileHit {
                    projectile_id: 406,
                    owner_id: shooter,
                    owner_life_seq: guard.clients[&shooter].life_seq,
                    victim_id: victim,
                    victim_life_seq: guard.clients[&victim].life_seq,
                    weapon_data: 1,
                    x: 1_000,
                    y: 1_000,
                    tick: 502,
                },
                runtime,
            )
        };
        assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
        assert!(
            state
                .lock()
                .expect("state")
                .pending_damage_transactions
                .contains_key(&hit.projectile_id)
        );

        crate::arena::disconnect_player(&state, victim);
        {
            let mut guard = state.lock().expect("state");
            guard.next_player_id = victim;
        }
        let (tx_replacement, _rx_replacement) = mpsc::channel();
        let replacement = register_connection(&state, tx_replacement);
        assert_eq!(replacement, victim);
        join_arena(&state, replacement, 0);
        let replacement_runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(replacement)
            .expect("replacement runtime");
        assert_ne!(replacement_runtime.session, victim_runtime.session);

        assert!(apply_server_detected_hit_with_broadcast(&state, &hit).is_none());
        let guard = state.lock().expect("state");
        assert_eq!(guard.clients[&replacement].server_energy, 1_700);
        assert!(guard.pending_damage_transactions.is_empty());
        assert!(
            !guard
                .active_projectiles
                .contains_pending_hit(hit.projectile_id)
        );
    }

    #[test]
    fn death_green_requires_nonzero_canonical_victim_velocity() {
        fn run(
            velocity_x: i16,
            velocity_y: i16,
        ) -> (Vec<u8>, Vec<crate::state::ServerHiddenPrize>) {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (victim_tx, _victim_rx) = mpsc::channel();
            let (killer_tx, _killer_rx) = mpsc::channel();
            let victim = register_connection(&state, victim_tx);
            let killer = register_connection(&state, killer_tx);
            join_arena(&state, victim, 0);
            join_arena(&state, killer, 0);
            let mut state = state.lock().expect("server state");
            set_damage_fixture_client_velocity(
                &mut state, victim, 8_200, 8_200, velocity_x, velocity_y, 1,
            );
            let killer_runtime = state
                .active_player_runtime_identity(killer)
                .expect("killer runtime");
            let artifacts = build_kill_pipeline(
                &mut state,
                &ProjectileHit {
                    projectile_id: 501,
                    owner_id: killer,
                    owner_life_seq: 1,
                    victim_id: victim,
                    victim_life_seq: 1,
                    weapon_data: 1,
                    x: 8_200,
                    y: 8_200,
                    tick: 9_000,
                },
                killer_runtime,
                None,
            )
            .expect("kill artifacts");
            (artifacts.death_packet, state.active_hidden_prizes.clone())
        }

        let (stationary_packet, stationary_registry) = run(0, 0);
        assert_eq!(stationary_packet.len(), 22);
        assert!(stationary_packet[1] as i8 <= 0);
        assert_eq!(
            u32::from_le_bytes(stationary_packet[10..14].try_into().unwrap()),
            0
        );
        assert!(stationary_registry.is_empty());

        let (moving_packet, moving_registry) = run(25, -10);
        assert!(moving_packet[1] as i8 > 0);
        let moving_id = u32::from_le_bytes(moving_packet[10..14].try_into().unwrap());
        assert_ne!(moving_id, 0);
        assert_eq!(moving_registry.len(), 1);
        assert_eq!(moving_registry[0].collectible_id, moving_id);
        assert_eq!((moving_registry[0].x, moving_registry[0].y), (8_200, 8_200));
    }

    #[test]
    fn server_damage_structured_log_line_is_json() {
        let event = ServerComputedDamage {
            damage_seq: 42,
            projectile_id: 99,
            server_tick: 1200,
            victim_id: 7,
            shooter_id: 3,
            victim_life_seq: 2,
            shooter_life_seq: 9,
            weapon_data: 0x0042,
            damage: 400,
            unprotected_damage: 500,
            shield_until_tick: Some(1_300),
            shield_remaining_ticks: 100,
            shield_lifetime_ticks: 200,
            emp_until_tick: None,
            energy_before: 1700,
            energy_after: 1300,
            lethal: false,
        };
        let line = server_damage_event_json_line(&event, "primary");
        let json = line
            .strip_prefix("zubspace_event ")
            .expect("structured event prefix should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(json).expect("structured damage event should be valid JSON");

        assert_eq!(parsed["type"], "server_damage_applied");
        assert_eq!(parsed["damage_seq"], 42);
        assert_eq!(parsed["projectile_id"], 99);
        assert_eq!(parsed["server_tick"], 1200);
        assert_eq!(parsed["victim_id"], 7);
        assert_eq!(parsed["shooter_id"], 3);
        assert_eq!(parsed["victim_life_seq"], 2);
        assert_eq!(parsed["shooter_life_seq"], 9);
        assert_eq!(parsed["weapon"], 0x0042);
        assert_eq!(parsed["damage"], 400);
        assert_eq!(parsed["energy_before"], 1700);
        assert_eq!(parsed["energy_after"], 1300);
        assert_eq!(parsed["lethal"], false);
        assert_eq!(parsed["kind"], "primary");
    }

    #[test]
    fn flag_off_returns_none_and_leaves_energy_unchanged() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        let energy_before = state
            .clients
            .get(&player)
            .expect("client present")
            .server_energy;
        let hit = make_hit(shooter, player, 1);
        let result = apply_server_detected_hit_inner(&mut state, &hit, false);
        assert!(result.is_none(), "flag-off path is a no-op");
        assert_eq!(
            state.clients.get(&player).unwrap().server_energy,
            energy_before,
            "energy untouched when flag is off"
        );
    }

    #[test]
    fn applies_damage_and_records_event_when_authority_active() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        let energy_before = state
            .clients
            .get(&player)
            .expect("client present")
            .server_energy;
        let hit = make_hit(shooter, player, 1); // bullet
        let event = apply_server_detected_hit_inner(&mut state, &hit, true)
            .expect("authority-on path produces an event");
        assert_eq!(event.projectile_id, hit.projectile_id);
        assert_eq!(event.victim_id, player);
        assert_eq!(event.shooter_id, shooter);
        assert_eq!(event.weapon_data, 1);
        assert!(event.damage > 0, "bullet has non-zero damage");
        assert_eq!(event.energy_before, energy_before);
        assert!(
            event.energy_after < energy_before,
            "energy reduced by the damage"
        );
        assert_eq!(
            state.clients.get(&player).unwrap().server_energy,
            event.energy_after,
            "client's server_energy reflects the deduction"
        );
    }

    #[test]
    fn damage_event_identity_preserves_damage_projectile_and_life_sequences() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, _rx_victim) = mpsc::channel();
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        let victim_life_seq = state.clients[&victim].life_seq;
        let shooter_life_seq = state.clients[&shooter].life_seq;

        let first = apply_server_detected_hit_inner(
            &mut state,
            &ProjectileHit {
                projectile_id: 500,
                owner_id: shooter,
                owner_life_seq: shooter_life_seq,
                victim_id: victim,
                victim_life_seq,
                weapon_data: 66,
                x: 100,
                y: 100,
                tick: 10,
            },
            true,
        )
        .expect("first hit applies");
        let second = apply_server_detected_hit_inner(
            &mut state,
            &ProjectileHit {
                projectile_id: 501,
                owner_id: shooter,
                owner_life_seq: shooter_life_seq,
                victim_id: victim,
                victim_life_seq,
                weapon_data: 66,
                x: 100,
                y: 100,
                tick: 11,
            },
            true,
        )
        .expect("second hit applies");

        assert_ne!(first.damage_seq, second.damage_seq);
        assert_eq!(first.projectile_id, 500);
        assert_eq!(second.projectile_id, 501);
        assert_eq!(first.victim_life_seq, victim_life_seq);
        assert_eq!(second.victim_life_seq, victim_life_seq);
        assert_eq!(first.shooter_life_seq, shooter_life_seq);
        assert_eq!(second.shooter_life_seq, shooter_life_seq);
    }

    #[test]
    fn delayed_damage_from_an_earlier_shooter_life_is_rejected() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, _rx_victim) = mpsc::channel();
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);

        let mut state = state.lock().expect("server state");
        let hit = make_hit(shooter, victim, 1);
        let energy_before = state.clients[&victim].server_energy;
        let next_shooter_life = state
            .advance_client_life(shooter)
            .expect("shooter should have an active client");

        assert_ne!(hit.owner_life_seq, next_shooter_life.life_seq);
        assert!(apply_server_detected_hit_inner(&mut state, &hit, true).is_none());
        assert_eq!(state.clients[&victim].server_energy, energy_before);
    }

    #[test]
    fn delayed_damage_against_an_earlier_victim_life_is_rejected() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, _rx_victim) = mpsc::channel();
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);

        let mut state = state.lock().expect("server state");
        let hit = make_hit(shooter, victim, 1);
        let next_victim_life = state
            .advance_client_life(victim)
            .expect("victim should have an active client");
        let energy_before = state.clients[&victim].server_energy;

        assert_ne!(hit.victim_life_seq, next_victim_life.life_seq);
        assert!(apply_server_detected_hit_inner(&mut state, &hit, true).is_none());
        assert_eq!(state.clients[&victim].server_energy, energy_before);
    }

    #[test]
    fn delayed_damage_from_a_recycled_player_id_is_rejected() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, _rx_victim) = mpsc::channel();
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        let stale_hit = make_hit(shooter, victim, 1);

        crate::arena::disconnect_player(&state, shooter);
        state.lock().expect("server state").next_player_id = shooter;
        let (tx_replacement, _rx_replacement) = mpsc::channel();
        let replacement = register_connection(&state, tx_replacement);
        assert_eq!(
            replacement, shooter,
            "fixture should recycle the numeric id"
        );
        join_arena(&state, replacement, 0);

        let mut state = state.lock().expect("server state");
        let replacement_life = state.clients[&replacement].life_seq;
        let energy_before = state.clients[&victim].server_energy;
        assert!(replacement_life > stale_hit.owner_life_seq);
        assert!(apply_server_detected_hit_inner(&mut state, &stale_hit, true).is_none());
        assert_eq!(state.clients[&victim].server_energy, energy_before);
    }

    #[test]
    fn gun_self_hit_is_rejected_even_if_detector_reports_it() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        let energy_before = state
            .clients
            .get(&player)
            .expect("client present")
            .server_energy;
        for weapon_data in [1, 2, 66, 7, 15 | (2 << 5)] {
            let hit = ProjectileHit {
                projectile_id: 102,
                owner_id: player,
                owner_life_seq: 1,
                victim_id: player,
                victim_life_seq: 1,
                weapon_data,
                x: 100,
                y: 100,
                tick: 0x1234,
            };

            let event = apply_server_detected_hit_inner(&mut state, &hit, true);

            assert!(
                event.is_none(),
                "gun/bullet weapon {weapon_data:#06x} must never apply authoritative self-damage"
            );
            assert_eq!(
                state.clients.get(&player).unwrap().server_energy,
                energy_before,
                "rejected gun self-hit leaves authoritative energy unchanged"
            );
        }
    }

    #[test]
    fn damage_updates_cached_position_energy_for_snapshots() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        {
            let client = state.clients.get_mut(&player).expect("client present");
            let mut last_position = vec![0; 23];
            last_position[0] = 0x05;
            last_position[21..23].copy_from_slice(&client.server_energy.to_le_bytes());
            client.last_position = Some(last_position);
            client.server_sim_state = Some(crate::state::ServerSimMovement {
                position_x: f32::NAN,
                position_y: f32::NAN,
                orientation: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                energy: client.server_energy as f32,
                max_energy: client.server_max_energy as f32,
            });
        }

        let hit = make_hit(shooter, player, 1);
        let event = apply_server_detected_hit_inner(&mut state, &hit, true)
            .expect("authority-on path produces an event");
        let client = state.clients.get(&player).expect("client present");
        let cached = client
            .last_position
            .as_ref()
            .expect("cached position should remain");
        assert_eq!(
            u16::from_le_bytes(cached[21..23].try_into().unwrap()),
            event.energy_after,
            "snapshot energy source should reflect authoritative damage"
        );
        assert_eq!(
            client
                .server_sim_state
                .as_ref()
                .map(|sim| sim.energy as u16),
            Some(event.energy_after),
            "movement sim energy should not exceed damaged energy"
        );
    }

    #[test]
    fn delayed_damage_commit_does_not_rewind_an_already_advanced_recharge_clock() {
        for prepared_path in [true, false] {
            for hit_tick in [1_000, 0x7fff_fffc] {
                for recharge_per_second in [115.0, 420.0] {
                    let state = Arc::new(Mutex::new(ServerState::default()));
                    let (tx, _rx) = mpsc::channel();
                    let player = register_connection(&state, tx);
                    join_arena(&state, player, 0);
                    let (tx, _rx_shooter) = mpsc::channel();
                    let shooter = register_connection(&state, tx);
                    join_arena(&state, shooter, 0);
                    let mut guard = state.lock().unwrap();
                    let now = crate::continuum::continuum_tick_add(hit_tick, 28);
                    let client = guard.clients.get_mut(&player).unwrap();
                    client.server_energy = client.server_max_energy;
                    client.server_energy_remainder = 0.0;
                    client.server_recharge_per_second = recharge_per_second;
                    client.last_energy_recharge_tick = Some(hit_tick);
                    let hit = ProjectileHit {
                        projectile_id: 103,
                        owner_id: shooter,
                        owner_life_seq: 1,
                        victim_id: player,
                        victim_life_seq: 1,
                        weapon_data: 66,
                        x: 100,
                        y: 100,
                        tick: hit_tick,
                    };
                    let transaction = prepare_server_detected_damage_transaction(&guard, &hit)
                        .expect("freeze the collision before its presentation wait");
                    crate::arena::advance_server_recharge(
                        guard.clients.get_mut(&player).unwrap(),
                        now,
                    );
                    let event = if prepared_path {
                        let (staged, _, _) =
                            stage_prepared_damage_transaction(&guard, &transaction).unwrap();
                        assert_eq!(staged.len(), 1);
                        apply_prepared_damage_hit(&mut guard, &staged[0])
                            .unwrap()
                            .unwrap()
                    } else {
                        apply_server_detected_hit_inner(&mut guard, &hit, true).unwrap()
                    };
                    assert_eq!(
                        event.server_tick, hit_tick,
                        "collision identity stays historical"
                    );
                    let client = guard.clients.get_mut(&player).unwrap();
                    assert_eq!(
                        client.last_energy_recharge_tick,
                        Some(now),
                        "a delayed damage commit must not refund its presentation wait: prepared={prepared_path}"
                    );
                    let elapsed = crate::arena::advance_server_recharge(
                        client,
                        crate::continuum::continuum_tick_add(now, 1),
                    );
                    assert_eq!(elapsed, 1, "only new time can recharge after damage");
                    let recovered = client.server_recharge_per_second / 100.0;
                    assert_eq!(
                        client.server_energy,
                        event.energy_after + recovered.floor() as u16
                    );
                    assert!((client.server_energy_remainder - recovered.fract()).abs() < 0.0001);
                }
            }
        }
    }

    #[test]
    fn damage_reanchors_recharge_to_damage_tick() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        {
            let client = state.clients.get_mut(&player).expect("client present");
            client.server_energy = client.server_max_energy;
            client.server_energy_remainder = 0.0;
            client.last_energy_recharge_tick = Some(900);
        }

        let hit = ProjectileHit {
            projectile_id: 103,
            owner_id: shooter,
            owner_life_seq: 1,
            victim_id: player,
            victim_life_seq: 1,
            weapon_data: 66,
            x: 100,
            y: 100,
            tick: 1000,
        };
        let event = apply_server_detected_hit_inner(&mut state, &hit, true)
            .expect("authority-on path produces an event");
        assert_eq!(event.damage, 400);
        assert_eq!(event.energy_before, 1700);
        assert_eq!(event.energy_after, 1300);

        let client = state.clients.get_mut(&player).expect("client present");
        assert_eq!(
            client.last_energy_recharge_tick,
            Some(hit.tick),
            "future recharge must start at the damage tick, not the last pre-hit movement tick"
        );
        let elapsed = crate::arena::advance_server_recharge(client, hit.tick + 1);
        assert_eq!(elapsed, 1);
        let recovered = client.server_recharge_per_second * 0.01;
        assert_eq!(client.server_energy, 1300 + recovered.floor() as u16);
        assert!((client.server_energy_remainder - recovered.fract()).abs() < 0.0001);
    }

    #[test]
    fn emp_partial_damage_duration_uses_full_bomb_damage_independent_of_radius() {
        let hit = make_hit(1, 2, 4 | (1 << 5));
        for radius in [80.0, 160.0, 240.0] {
            for damage in [1, 180, 375, 749, 750] {
                assert_eq!(
                    emp_shutdown_ticks_for_damage_radius(&hit, Some(5), damage, Some(radius)),
                    400 * u32::from(damage) / 750,
                    "the reference game scales EBombShutdownTime by post-falloff damage / full bomb damage"
                );
            }
        }
    }

    #[test]
    fn emp_deadline_is_anchored_to_hit_tick_across_31_bit_wrap() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (victim_tx, _victim_rx) = mpsc::channel();
        let victim = register_connection(&state, victim_tx);
        join_arena(&state, victim, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 5);
        assert!(arena_ship_emp_bomb(5));

        let hit_tick = 0x7fff_ff00;
        let shutdown_ticks = arena_emp_bomb_shutdown_ticks().max(0) as u32;
        assert!(shutdown_ticks > 0);
        let expected_deadline = crate::continuum::continuum_tick_add(hit_tick, shutdown_ticks);
        let mut hit = make_hit(shooter, victim, 3);
        hit.tick = hit_tick;

        let mut state = state.lock().expect("server state");
        apply_server_detected_hit_inner(&mut state, &hit, true)
            .expect("EMP bomb should apply authoritative damage");
        let client = state.clients.get(&victim).expect("victim should exist");
        assert_eq!(client.server_emp_until_tick, Some(expected_deadline));
        assert!(
            client.server_emp_active(crate::continuum::continuum_tick_sub(expected_deadline, 1,))
        );
        assert!(!client.server_emp_active(expected_deadline));
    }

    #[test]
    fn emp_excludes_self_and_each_later_hit_replaces_the_deadline_across_wrap() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (victim_tx, _victim_rx) = mpsc::channel();
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let victim = register_connection(&state, victim_tx);
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 5);
        let tick = crate::continuum::CONTINUUM_TICK_MASK - 20;
        let radius = bomb_explosion_radius_pixels(continuum_weapon_explosion_level(3 | (2 << 5)));
        {
            let mut state = state.lock().expect("state");
            set_damage_fixture_client(&mut state, shooter, 2_000, 2_000, 1_700);
            set_damage_fixture_client(&mut state, victim, 2_000, 2_000, 1_700);
            let mut self_hit = make_hit(shooter, shooter, 3 | (2 << 5));
            self_hit.x = 2_000;
            self_hit.y = 2_000;
            self_hit.tick = tick;
            apply_server_detected_hit_inner(&mut state, &self_hit, true)
                .expect("self bomb still has ordinary self-damage");
            assert_eq!(state.clients[&shooter].server_emp_until_tick, None);

            let mut full = make_hit(shooter, victim, 3 | (2 << 5));
            full.x = 2_000;
            full.y = 2_000;
            full.tick = tick;
            let first =
                apply_server_detected_hit_inner(&mut state, &full, true).expect("full EMP hit");
            let first_deadline = first.emp_until_tick.expect("EMP deadline");

            set_damage_fixture_client(
                &mut state,
                victim,
                (2_000.0 + radius * 0.75) as u16,
                2_000,
                1_700,
            );
            let mut shorter = full;
            shorter.projectile_id = shorter.projectile_id.wrapping_add(1);
            shorter.tick = crate::continuum::continuum_tick_add(tick, 1);
            let second = apply_server_detected_hit_inner(&mut state, &shorter, true)
                .expect("shorter splash EMP hit");
            let second_deadline = second.emp_until_tick.expect("replacement deadline");
            assert_ne!(second_deadline, first_deadline);
            assert_eq!(
                state.clients[&victim].server_emp_until_tick,
                Some(second_deadline)
            );
            assert!(
                crate::continuum::continuum_tick_diff(first_deadline, second_deadline) > 0,
                "a later shorter EMP replaces rather than max-stacks"
            );

            set_damage_fixture_client(&mut state, victim, 2_000, 2_000, 1_700);
            let mut longer = full;
            longer.projectile_id = longer.projectile_id.wrapping_add(2);
            longer.tick = crate::continuum::continuum_tick_add(tick, 2);
            let third = apply_server_detected_hit_inner(&mut state, &longer, true)
                .expect("later full EMP hit");
            let third_deadline = third.emp_until_tick.expect("long replacement deadline");
            assert_eq!(
                state.clients[&victim].server_emp_until_tick,
                Some(third_deadline)
            );
            assert!(crate::continuum::continuum_tick_diff(third_deadline, second_deadline) > 0);

            state
                .clients
                .get_mut(&victim)
                .expect("victim")
                .clear_server_timed_protections();
            state.advance_client_life_at_tick(victim, third_deadline);
            assert_eq!(state.clients[&victim].server_emp_until_tick, None);
            assert_eq!(state.clients[&victim].server_emp_start_sequence, None);
        }
    }

    #[test]
    fn fully_shielded_emp_hit_has_no_damage_identity_watch_assist_or_loss_fact() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (victim_tx, _victim_rx) = mpsc::channel();
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let victim = register_connection(&state, victim_tx);
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 5);
        let tick = 12_000;
        let (hit, energy_before, revision_before) = {
            let mut state = state.lock().expect("state");
            set_damage_fixture_client(&mut state, shooter, 1_000, 2_000, 1_700);
            set_damage_fixture_client(&mut state, victim, 2_000, 2_000, 1_200);
            let lifetime = arena_ship_shield_lifetime_ticks(0);
            let client = state.clients.get_mut(&victim).expect("victim");
            client.server_bounty = 1;
            client.server_shield_until_tick =
                Some(crate::continuum::continuum_tick_add(tick, lifetime));
            let hit = ProjectileHit {
                projectile_id: 0x55aa,
                owner_id: shooter,
                owner_life_seq: state.clients[&shooter].life_seq,
                victim_id: victim,
                victim_life_seq: state.clients[&victim].life_seq,
                weapon_data: 3 | (2 << 5),
                x: 2_000,
                y: 2_000,
                tick,
            };
            (
                hit,
                state.clients[&victim].server_energy,
                state.clients[&victim].server_prize_state_revision,
            )
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let state = state.lock().expect("state");
        let client = &state.clients[&victim];
        assert!(client.server_emp_until_tick.is_some());
        assert_eq!(client.server_energy, energy_before);
        assert!(client.recent_damagers.is_empty());
        assert_eq!(client.last_damage_prize_loss_seq, None);
        assert_eq!(client.server_prize_state_revision, revision_before);
        assert_eq!(state.next_damage_seq, 0);
        assert!(
            state.clients[&victim]
                .pending_realtime_events
                .queue
                .is_empty()
        );
        assert!(
            state.clients[&shooter]
                .pending_realtime_events
                .queue
                .is_empty()
        );
    }

    #[test]
    fn authoritative_damage_queues_snapshot_watch_damage_without_legacy_packet() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, rx_victim) = mpsc::channel();
        let (tx_shooter, rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        drain(&rx_victim);
        drain(&rx_shooter);

        let hit = ProjectileHit {
            projectile_id: 104,
            owner_id: shooter,
            owner_life_seq: 1,
            victim_id: victim,
            victim_life_seq: 1,
            weapon_data: 1,
            x: 100,
            y: 100,
            tick: 0x1122_3344,
        };
        let event = apply_server_detected_hit_with_broadcast(&state, &hit)
            .expect("authoritative damage should apply");

        assert!(matches!(
            rx_victim.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert!(matches!(
            rx_shooter.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        let state_guard = state.lock().expect("server state should not be poisoned");
        for player_id in [victim, shooter] {
            let queue = &state_guard.clients[&player_id]
                .pending_snapshot_events
                .watch_damage;
            assert_eq!(queue.len(), 1);
            let queued = queue[0];
            assert_eq!(queued.victim_id, event.victim_id);
            assert_eq!(queued.shooter_id, event.shooter_id);
            assert_eq!(queued.weapon_data, event.weapon_data);
            assert_eq!(queued.damage, event.damage);
            assert_eq!(queued.energy_after, event.energy_after);
            assert_eq!(queued.lethal, event.lethal);
            let realtime = &state_guard.clients[&player_id]
                .pending_realtime_events
                .queue;
            assert_eq!(realtime.len(), 1);
            let crate::realtime_event::RealtimeEventPayload::WatchDamage(realtime) =
                realtime[0].payload
            else {
                panic!("damage must queue the dedicated authenticated event kind");
            };
            assert_eq!(realtime.victim_player_id, victim);
            assert_eq!(realtime.shooter_player_id, shooter);
            assert_eq!(realtime.victim_life_seq, event.victim_life_seq);
            assert_eq!(realtime.shooter_life_seq, event.shooter_life_seq);
            assert_eq!(realtime.damage_seq, event.damage_seq);
            assert_eq!(realtime.projectile_id, event.projectile_id);
            assert_ne!(realtime.victim_session_id, 0);
            assert_ne!(realtime.shooter_session_id, 0);
        }
        drop(state_guard);

        // A later soft cutover of the observing shooter cannot erase this
        // already-accepted public fact. This is the production ordering seen
        // when a browser damages a bot and is killed later in the same tick.
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .advance_client_life(shooter)
            .expect("observer life should advance");
        assert_eq!(
            state.clients[&shooter]
                .pending_snapshot_events
                .watch_damage
                .len(),
            1
        );
        assert_eq!(
            state.clients[&shooter].pending_realtime_events.queue.len(),
            1
        );
    }

    #[test]
    fn delayed_watch_damage_targets_only_captured_compatible_lives() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, _rx_victim) = mpsc::channel();
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        let hit = make_hit(shooter, victim, 1);

        let applied = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let worker = {
            let state = Arc::clone(&state);
            let applied = Arc::clone(&applied);
            let resume = Arc::clone(&resume);
            thread::spawn(move || {
                apply_server_detected_hit_with_delivery_hook(&state, &hit, || {
                    applied.wait();
                    resume.wait();
                })
            })
        };
        applied.wait();

        let next_victim_life = state.lock().unwrap().advance_client_life(victim).unwrap();
        assert_ne!(next_victim_life.life_seq, hit.victim_life_seq);
        resume.wait();
        let event = worker.join().unwrap().expect("damage should apply");

        let state = state.lock().unwrap();
        assert!(
            state.clients[&victim]
                .pending_snapshot_events
                .watch_damage
                .is_empty(),
            "the newer victim life must not receive the delayed old-life event"
        );
        let shooter_queue = &state.clients[&shooter].pending_snapshot_events.watch_damage;
        assert_eq!(shooter_queue.len(), 1);
        assert_eq!(shooter_queue[0].recipient.life.life_seq, hit.owner_life_seq);
        assert_eq!(shooter_queue[0].event.damage_seq, event.damage_seq);
    }

    #[test]
    fn delayed_lethal_artifacts_do_not_retarget_reused_victim_session() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, _rx_victim) = mpsc::channel();
        let (tx_shooter, rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        drain(&rx_shooter);
        state
            .lock()
            .unwrap()
            .clients
            .get_mut(&victim)
            .unwrap()
            .server_energy = 1;
        let hit = make_hit(shooter, victim, 1);
        let old_victim_session = state
            .lock()
            .unwrap()
            .active_player_session_identity(victim)
            .unwrap();

        let applied = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let worker = {
            let state = Arc::clone(&state);
            let applied = Arc::clone(&applied);
            let resume = Arc::clone(&resume);
            thread::spawn(move || {
                apply_server_detected_hit_with_delivery_hook(&state, &hit, || {
                    applied.wait();
                    resume.wait();
                })
            })
        };
        applied.wait();

        crate::arena::disconnect_player(&state, victim);
        state.lock().unwrap().next_player_id = victim;
        let (replacement_tx, replacement_rx) = mpsc::channel();
        let replacement = register_connection(&state, replacement_tx);
        assert_eq!(replacement, victim);
        join_arena(&state, replacement, 0);
        let replacement_session = state
            .lock()
            .unwrap()
            .active_player_session_identity(replacement)
            .unwrap();
        assert_ne!(
            replacement_session.session_id,
            old_victim_session.session_id
        );
        drain(&replacement_rx);
        drain(&rx_shooter);

        resume.wait();
        let lethal = worker.join().unwrap().expect("lethal damage should apply");
        assert!(lethal.lethal);

        let state_guard = state.lock().unwrap();
        assert!(
            state_guard.clients[&replacement]
                .pending_snapshot_events
                .watch_damage
                .is_empty(),
            "replacement must not inherit old-session WATCH_DAMAGE"
        );
        let shooter_queue = &state_guard.clients[&shooter]
            .pending_snapshot_events
            .watch_damage;
        assert_eq!(
            shooter_queue.len(),
            1,
            "compatible shooter still receives WATCH_DAMAGE"
        );
        drop(state_guard);

        let replacement_types = drain_reliable_payload_types(&replacement_rx);
        assert!(!replacement_types.contains(&PLAYER_DEATH_PACKET));
        assert!(!replacement_types.contains(&crate::roster::SCORE_UPDATE));
        let shooter_types = drain_reliable_payload_types(&rx_shooter);
        assert!(
            !shooter_types.contains(&PLAYER_DEATH_PACKET),
            "the old victim runtime no longer exists, so no observer may receive its death"
        );
        assert!(shooter_types.contains(&crate::roster::SCORE_UPDATE));
    }

    #[test]
    fn lethal_death_delivers_for_the_kill_pipeline_post_death_runtime() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, rx_victim) = mpsc::channel();
        let (tx_shooter, rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        drain(&rx_victim);
        drain(&rx_shooter);
        state
            .lock()
            .unwrap()
            .clients
            .get_mut(&victim)
            .unwrap()
            .server_energy = 1;

        let hit = make_hit(shooter, victim, 1);
        assert!(
            apply_server_detected_hit_with_broadcast(&state, &hit)
                .unwrap()
                .lethal
        );
        assert_eq!(
            state.lock().unwrap().clients[&victim].life_seq,
            hit.victim_life_seq + 1,
            "the kill pipeline advances to the dead/respawn runtime before delivery"
        );
        for rx in [&rx_victim, &rx_shooter] {
            let reliable_types = drain_reliable_payload_types(rx);
            assert!(reliable_types.contains(&PLAYER_DEATH_PACKET));
            assert!(reliable_types.contains(&crate::roster::SCORE_UPDATE));
        }
    }

    #[test]
    fn delayed_death_rejects_later_life_but_durable_scores_survive() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_victim, rx_victim) = mpsc::channel();
        let (tx_shooter, rx_shooter) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        drain(&rx_victim);
        drain(&rx_shooter);
        state
            .lock()
            .unwrap()
            .clients
            .get_mut(&victim)
            .unwrap()
            .server_energy = 1;
        let hit = make_hit(shooter, victim, 1);

        let applied = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let worker = {
            let state = Arc::clone(&state);
            let applied = Arc::clone(&applied);
            let resume = Arc::clone(&resume);
            thread::spawn(move || {
                apply_server_detected_hit_with_delivery_hook(&state, &hit, || {
                    applied.wait();
                    resume.wait();
                })
            })
        };
        applied.wait();

        let captured_dead_life = state.lock().unwrap().clients[&victim].life_seq;
        let advanced_life = state.lock().unwrap().advance_client_life(victim).unwrap();
        assert_ne!(advanced_life.life_seq, captured_dead_life);
        resume.wait();
        assert!(worker.join().unwrap().unwrap().lethal);

        assert!(
            state.lock().unwrap().clients[&victim]
                .pending_snapshot_events
                .watch_damage
                .is_empty(),
            "life-scoped WATCH_DAMAGE must be filtered after the transition"
        );
        for rx in [&rx_victim, &rx_shooter] {
            let reliable_types = drain_reliable_payload_types(rx);
            assert!(
                !reliable_types.contains(&PLAYER_DEATH_PACKET),
                "PLAYER_DEATH must not target the victim after another life transition"
            );
            assert!(reliable_types.contains(&crate::roster::SCORE_UPDATE));
        }
    }

    #[test]
    fn lethal_damage_sets_lethal_flag_and_zeroes_energy() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        // Set victim energy to 1 so the bullet (>1 damage) is lethal.
        state
            .clients
            .get_mut(&player)
            .expect("client")
            .server_energy = 1;
        let hit = make_hit(shooter, player, 1);
        let event =
            apply_server_detected_hit_inner(&mut state, &hit, true).expect("damage applied");
        assert!(event.lethal, "lethal flag set");
        assert_eq!(event.energy_after, 0, "energy floored at zero");
    }

    #[test]
    fn lethal_self_bomb_damage_survives_at_one_energy() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let mut state = state.lock().expect("server state");
        set_damage_fixture_client(&mut state, player, 8_200, 8_200, 123);

        let hit = ProjectileHit {
            projectile_id: 108,
            owner_id: player,
            owner_life_seq: 1,
            victim_id: player,
            victim_life_seq: 1,
            weapon_data: 3 | (2 << 5),
            x: 8_200,
            y: 8_200,
            tick: 12_000,
        };
        let event =
            apply_server_detected_hit_inner(&mut state, &hit, true).expect("damage applied");

        assert_eq!(event.damage, 750);
        assert_eq!(event.energy_before, 123);
        assert_eq!(event.energy_after, 1);
        assert!(!event.lethal, "self-bomb damage must not kill the owner");
        assert_eq!(state.clients[&player].server_energy, 1);
        assert!(state.clients[&player].alive);
    }

    #[test]
    fn exact_self_bomb_damage_is_nonlethal_but_reaches_zero_energy() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let mut state = state.lock().expect("server state");
        set_damage_fixture_client(&mut state, player, 8_200, 8_200, 750);

        let hit = ProjectileHit {
            projectile_id: 109,
            owner_id: player,
            owner_life_seq: 1,
            victim_id: player,
            victim_life_seq: 1,
            weapon_data: 3 | (2 << 5),
            x: 8_200,
            y: 8_200,
            tick: 12_001,
        };
        let event =
            apply_server_detected_hit_inner(&mut state, &hit, true).expect("damage applied");

        assert_eq!(event.damage, 750);
        assert_eq!(event.energy_before, 750);
        assert_eq!(event.energy_after, 0);
        assert!(
            !event.lethal,
            "the reference game uses strict energy < damage lethality"
        );
        assert_eq!(state.clients[&player].server_energy, 0);
        assert!(state.clients[&player].alive);
    }

    #[test]
    fn chaos_yellow_bomb_direct_and_proximity_edge_damage_match_settings() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let victim = register_connection(&state, tx);
        join_arena(&state, victim, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        let yellow_proximity_bomb = 4 | (1 << 5);

        set_damage_fixture_client(&mut state, victim, 8_200, 8_200, 1_700);
        let direct = apply_server_detected_hit_inner(
            &mut state,
            &ProjectileHit {
                projectile_id: 110,
                owner_id: shooter,
                owner_life_seq: 1,
                victim_id: victim,
                victim_life_seq: 1,
                weapon_data: yellow_proximity_bomb,
                x: 8_200,
                y: 8_200,
                tick: 12_002,
            },
            true,
        )
        .expect("direct yellow bomb damage applies");
        assert_eq!(direct.damage, 750);

        set_damage_fixture_client(&mut state, victim, 8_200, 8_200, 1_700);
        let proximity_edge = apply_server_detected_hit_inner(
            &mut state,
            &ProjectileHit {
                projectile_id: 111,
                owner_id: shooter,
                owner_life_seq: 1,
                victim_id: victim,
                victim_life_seq: 1,
                weapon_data: yellow_proximity_bomb,
                x: 8_272,
                y: 8_200,
                tick: 12_003,
            },
            true,
        )
        .expect("proximity-edge yellow bomb damage applies");
        assert_eq!(proximity_edge.damage, 412);
    }

    #[test]
    fn dead_or_spectator_victim_is_skipped() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        {
            let mut state = state.lock().expect("server state");
            state.clients.get_mut(&player).expect("client").alive = false;
            let hit = make_hit(shooter, player, 1);
            assert!(
                apply_server_detected_hit_inner(&mut state, &hit, true).is_none(),
                "dead victim ⇒ no damage applied"
            );
        }
        // Spectator (ship >= 8) — same skip.
        {
            let mut state = state.lock().expect("server state");
            let client = state.clients.get_mut(&player).expect("client");
            client.alive = true;
            client.player.ship = 8;
            let hit = make_hit(shooter, player, 1);
            assert!(
                apply_server_detected_hit_inner(&mut state, &hit, true).is_none(),
                "spectator victim ⇒ no damage applied"
            );
        }
    }

    #[test]
    fn unknown_weapon_returns_none() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        // Weapon type 0 (none) → no damage formula → return None.
        let hit = make_hit(shooter, player, 0);
        assert!(
            apply_server_detected_hit_inner(&mut state, &hit, true).is_none(),
            "weapon_data == 0 produces no damage"
        );
    }

    #[test]
    fn repeated_kill_respawn_bullet_damage_cycles_reset_energy_and_life_identity() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_v, rx_v) = mpsc::channel();
        let (tx_s, rx_s) = mpsc::channel();
        let victim = register_connection(&state, tx_v);
        let shooter = register_connection(&state, tx_s);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        drain(&rx_v);
        drain(&rx_s);

        let mut expected_life_seq = state
            .lock()
            .expect("server state")
            .clients
            .get(&victim)
            .expect("victim exists")
            .life_seq;
        for cycle in 1..=2 {
            let mut last = None;
            for _ in 0..5 {
                last = apply_server_detected_hit_with_broadcast(
                    &state,
                    &ProjectileHit {
                        projectile_id: 1_000 + cycle,
                        owner_id: shooter,
                        owner_life_seq: 1,
                        victim_id: victim,
                        victim_life_seq: expected_life_seq,
                        weapon_data: 66,
                        x: 100,
                        y: 100,
                        tick: 1000 + cycle,
                    },
                );
            }
            let lethal = last.expect("fifth full-energy bullet should kill");
            assert_eq!(lethal.victim_life_seq, expected_life_seq);
            assert_eq!(lethal.damage, 400);
            assert_eq!(lethal.energy_before, 100);
            assert_eq!(lethal.energy_after, 0);
            assert!(lethal.lethal);

            {
                let state = state.lock().expect("server state");
                let victim_client = state.clients.get(&victim).expect("victim exists");
                assert!(!victim_client.alive);
                assert_eq!(victim_client.server_energy, 0);
                assert_eq!(victim_client.life_seq, expected_life_seq + 1);
                assert!(victim_client.respawn_at_tick.is_some());
            }

            expected_life_seq += 1;
            {
                let mut state = state.lock().expect("server state");
                let victim_client = state.clients.get_mut(&victim).expect("victim exists");
                victim_client.alive = true;
                victim_client.respawn_at_tick = None;
                victim_client.reset_server_runtime_for_ship();
            }

            let post_respawn = apply_server_detected_hit_with_broadcast(
                &state,
                &ProjectileHit {
                    projectile_id: 2_000 + cycle,
                    owner_id: shooter,
                    owner_life_seq: 1,
                    victim_id: victim,
                    victim_life_seq: expected_life_seq,
                    weapon_data: 66,
                    x: 100,
                    y: 100,
                    tick: 2000 + cycle,
                },
            )
            .expect("fresh post-respawn bullet should damage");
            assert_eq!(post_respawn.victim_life_seq, expected_life_seq);
            assert_eq!(post_respawn.damage, 400);
            assert_eq!(post_respawn.energy_before, 1700);
            assert_eq!(post_respawn.energy_after, 1300);
            assert!(!post_respawn.lethal);

            {
                let mut state = state.lock().expect("server state");
                for recipient in [victim, shooter] {
                    let queued = state.clients[&recipient]
                        .pending_snapshot_events
                        .watch_damage
                        .back()
                        .copied()
                        .expect("post-respawn damage should queue snapshot watch damage");
                    assert_eq!(queued.victim_id, victim);
                    assert_eq!(queued.shooter_id, shooter);
                    assert_eq!(queued.damage_seq, post_respawn.damage_seq);
                    assert_eq!(queued.server_tick, post_respawn.server_tick);
                    assert_eq!(queued.victim_life_seq, post_respawn.victim_life_seq);
                    assert_eq!(queued.energy_before, 1700);
                    assert_eq!(queued.energy_after, 1300);
                    assert!(!queued.lethal);
                }
                let victim_client = state.clients.get_mut(&victim).expect("victim exists");
                victim_client.server_energy = victim_client.server_max_energy;
                victim_client.server_energy_remainder = 0.0;
                victim_client.last_reported_energy = victim_client.server_energy;
            }
        }
    }

    #[test]
    fn death_and_respawn_reset_weapon_inventory_cooldowns_and_pending_weapon() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);

        {
            let mut state = state.lock().expect("server state");
            let client = state.clients.get_mut(&player).expect("client present");
            client.server_weapons = WeaponInventory {
                gun_level: 0,
                bomb_level: 0,
                shrapnel: u8::MAX,
                bouncing_bullets: false,
                multifire_capable: true,
                proximity_bombs: true,
                repels: 0,
                bursts: 0,
                decoys: 0,
                thors: 0,
                bricks: 0,
                rockets: 0,
                portals: 0,
            };
            client.last_weapon_fire_tick = Some(10);
            client.last_gun_fire_tick = Some(11);
            client.last_bomb_fire_tick = Some(12);
            client.last_accepted_weapon_data = Some(0x0666);
            client.last_accepted_weapon_tick = Some(10);
            client.last_special_weapon_request = Some(7);
            client.last_brick_drop_tick = Some(10);
            client.pending_snapshot_weapon = 0x0666;
            client.server_rocket_until_tick = Some(1_000);
            client.server_rocket_input_asserted = true;

            client.server_thrust_acceleration = 3.0;
            client.server_top_speed = 4.0;
            client.server_rotation_per_second = 5.0;
            client.server_recharge_per_second = 6.0;
            client
                .position_history
                .push_back(crate::state::RecentPositionSample {
                    tick: 123,
                    x: 1000.0,
                    y: 1000.0,
                });
            client.server_sim_state = Some(crate::state::ServerSimMovement {
                position_x: f32::NAN,
                position_y: f32::NAN,
                orientation: 0.25,
                velocity_x: 12.0,
                velocity_y: -8.0,
                energy: 999.0,
                max_energy: 1700.0,
            });
            client.server_sim_anchor_tick = Some(123);

            client.alive = false;
            client.clear_server_runtime();
            assert_eq!(
                client.server_weapons,
                WeaponInventory::cleared(),
                "death should clear the old weapon loadout"
            );
            assert_weapon_latches_clear(client);
            assert_eq!(client.server_rocket_until_tick, None);
            assert!(!client.server_rocket_input_asserted);
            assert_eq!(
                client.pending_snapshot_weapon, 0,
                "death must not leave a one-shot weapon queued for a later snapshot"
            );

            client.server_weapons = WeaponInventory {
                gun_level: 0,
                bomb_level: 0,
                shrapnel: u8::MAX,
                bouncing_bullets: false,
                multifire_capable: true,
                proximity_bombs: true,
                repels: 0,
                bursts: 0,
                decoys: 0,
                thors: 0,
                bricks: 0,
                rockets: 0,
                portals: 0,
            };
            client.last_weapon_fire_tick = Some(20);
            client.last_gun_fire_tick = Some(21);
            client.last_bomb_fire_tick = Some(22);
            client.last_accepted_weapon_data = Some(0x0666);
            client.last_accepted_weapon_tick = Some(20);
            client.last_special_weapon_request = Some(5);
            client.last_brick_drop_tick = Some(20);
            client.pending_snapshot_weapon = 0x0666;
            client.server_rocket_until_tick = Some(2_000);
            client.server_rocket_input_asserted = true;

            client.alive = true;
            client.respawn_at_tick = None;
            client.reset_server_loadout_for_ship();

            let expected = WeaponInventory::initialized(crate::state::weapon_inventory_tuning(
                client.player.ship,
            ));
            assert_eq!(
                client.server_weapons, expected,
                "respawn should restore the arena's initial weapon loadout"
            );
            assert_weapon_latches_clear(client);
            assert_eq!(client.server_rocket_until_tick, None);
            assert!(!client.server_rocket_input_asserted);
            assert_eq!(
                client.pending_snapshot_weapon, 0,
                "respawn must not publish stale pre-respawn weapon state"
            );
            assert_eq!(
                client.server_thrust_acceleration,
                crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(0)
            );
            assert_eq!(
                client.server_top_speed,
                crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(0)
            );
            assert_eq!(
                client.server_rotation_per_second,
                crate::arena_settings::arena_ship_initial_rotation_per_second(0)
            );
            assert_eq!(
                client.server_recharge_per_second,
                crate::arena_settings::arena_ship_initial_recharge_per_second(0)
            );
            assert!(client.position_history.is_empty());
            assert!(client.server_sim_state.is_none());
            assert!(client.server_sim_anchor_tick.is_none());
        }
    }

    fn assert_weapon_latches_clear(client: &crate::state::ClientHandle) {
        assert_eq!(client.last_weapon_fire_tick, None);
        assert_eq!(client.last_gun_fire_tick, None);
        assert_eq!(client.last_bomb_fire_tick, None);
        assert_eq!(client.last_accepted_weapon_data, None);
        assert_eq!(client.last_accepted_weapon_tick, None);
        assert_eq!(client.last_special_weapon_request, None);
        assert_eq!(client.last_brick_drop_tick, None);
    }

    fn hit_from(shooter: u16, victim: u16, weapon_data: u16) -> ProjectileHit {
        ProjectileHit {
            projectile_id: 107,
            owner_id: shooter,
            owner_life_seq: 1,
            victim_id: victim,
            victim_life_seq: 1,
            weapon_data,
            x: 100,
            y: 100,
            tick: 0,
        }
    }

    #[test]
    fn damage_hit_pushes_shooter_onto_victim_recent_damagers() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_v, _rx_v) = mpsc::channel();
        let (tx_s, _rx_s) = mpsc::channel();
        let victim = register_connection(&state, tx_v);
        let shooter = register_connection(&state, tx_s);
        join_arena(&state, victim, 0);
        join_arena(&state, shooter, 0);
        let mut state = state.lock().expect("server state");
        let hit = hit_from(shooter, victim, 1); // bullet
        apply_server_detected_hit_inner(&mut state, &hit, true).expect("hit applied");
        let ring = &state.clients[&victim].recent_damagers;
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].0.player_id, shooter);
        assert_eq!(ring[0].0.life_seq, 1);
    }

    #[test]
    fn self_damage_is_not_recorded_for_assist_credit() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        let mut state = state.lock().expect("server state");
        // owner_id == victim_id (self-bomb / suicide-style damage).
        let hit = hit_from(player, player, 1);
        apply_server_detected_hit_inner(&mut state, &hit, true);
        assert!(
            state.clients[&player].recent_damagers.is_empty(),
            "self-damage should not be tracked as an assister"
        );
    }

    #[test]
    fn build_kill_pipeline_awards_assists_to_recent_non_killer_damagers() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_v, _rx_v) = mpsc::channel();
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_k, _rx_k) = mpsc::channel();
        let victim = register_connection(&state, tx_v);
        let assister = register_connection(&state, tx_a);
        let killer = register_connection(&state, tx_k);
        join_arena(&state, victim, 0);
        join_arena(&state, assister, 0);
        join_arena(&state, killer, 0);
        let mut state = state.lock().expect("server state");
        // assister lands a non-lethal hit first.
        apply_server_detected_hit_inner(&mut state, &hit_from(assister, victim, 1), true)
            .expect("assist hit applied");
        // Verify the ring contains the assister.
        assert_eq!(state.clients[&victim].recent_damagers.len(), 1);

        // Now run the kill pipeline as if killer landed a lethal blow.
        let kill_hit = hit_from(killer, victim, 1);
        // Ensure the kill_pipeline sees the victim as still alive.
        state.clients.get_mut(&victim).unwrap().alive = true;
        let killer_runtime = state
            .active_player_runtime_identity(killer)
            .expect("killer runtime");
        let artifacts = build_kill_pipeline(&mut state, &kill_hit, killer_runtime, None)
            .expect("kill pipeline emits artifacts");
        // Assist scoring side effect.
        assert_eq!(
            state.clients[&assister].player.assists, 1,
            "non-killer recent damager should get +1 assist"
        );
        assert_eq!(state.clients[&killer].player.assists, 0);
        assert_eq!(state.clients[&victim].player.assists, 0);
        // The score_packets bundle should include at minimum: the
        // victim's score_update, the killer's, and the assister's
        // (3 entries). Pin the count loosely — just confirm the
        // assister's contribution is in there.
        assert!(
            artifacts.score_packets.len() >= 3,
            "expected ≥3 score_packets (victim, killer, assister); got {}",
            artifacts.score_packets.len()
        );
    }

    #[test]
    fn kill_pipeline_skips_assist_credit_for_killer_themselves() {
        // If the killer was also the most recent damager (because
        // they hit the victim several times en route to the lethal
        // hit), the kill pipeline should award them only the kill —
        // no assist.
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_v, _rx_v) = mpsc::channel();
        let (tx_k, _rx_k) = mpsc::channel();
        let victim = register_connection(&state, tx_v);
        let killer = register_connection(&state, tx_k);
        join_arena(&state, victim, 0);
        join_arena(&state, killer, 0);
        let mut state = state.lock().expect("server state");
        // Killer lands several non-lethal hits.
        for _ in 0..3 {
            apply_server_detected_hit_inner(&mut state, &hit_from(killer, victim, 1), true);
        }
        // Then the lethal one via the kill pipeline.
        state.clients.get_mut(&victim).unwrap().alive = true;
        let killer_runtime = state
            .active_player_runtime_identity(killer)
            .expect("killer runtime");
        build_kill_pipeline(
            &mut state,
            &hit_from(killer, victim, 1),
            killer_runtime,
            None,
        )
        .expect("kill pipeline emits artifacts");
        assert_eq!(
            state.clients[&killer].player.assists, 0,
            "killer should not receive assist credit on their own kill"
        );
    }

    #[test]
    fn human_projectile_kills_server_bot_and_awards_authoritative_score() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let shooter = register_connection(&state, tx);
        join_arena(&state, shooter, 0);
        let (bot_id, bot_life, x, y, shooter_life, wins_before) = {
            let mut guard = state.lock().expect("state");
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.energy = 1;
            (
                bot.player.id,
                bot.life_seq,
                bot.x as u16,
                bot.y as u16,
                guard.clients[&shooter].life_seq,
                guard.clients[&shooter].player.wins,
            )
        };
        let hit = ProjectileHit {
            projectile_id: 700,
            owner_id: shooter,
            owner_life_seq: shooter_life,
            victim_id: bot_id,
            victim_life_seq: bot_life,
            weapon_data: 1,
            x,
            y,
            tick: 50,
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        let bot = guard.bot(bot_id).expect("bot remains rostered");
        assert!(!bot.alive());
        assert_eq!(bot.life_seq, bot_life + 1);
        assert_eq!(bot.player.losses, 1);
        assert_eq!(guard.clients[&shooter].player.wins, wins_before + 1);
        assert_eq!(guard.objectives.profile().flags.count(), 0);
        assert_eq!(guard.objectives.profile().powerballs.count(), 0);
        let realtime = &guard.clients[&shooter].pending_realtime_events.queue;
        let watch = realtime
            .iter()
            .find_map(|delivery| {
                let crate::realtime_event::RealtimeEventPayload::WatchDamage(watch) =
                    delivery.payload
                else {
                    return None;
                };
                (watch.victim_player_id == bot_id).then_some(watch)
            })
            .expect("human shooter must receive bot-victim WATCH_DAMAGE presentation");
        assert_eq!(watch.victim_session_id, bot.session_id.get());
        assert_eq!(watch.victim_life_seq, bot_life);
        assert_eq!(watch.shooter_player_id, shooter);
        assert_eq!(watch.shooter_life_seq, shooter_life);
        assert_eq!(watch.energy_after, 0);
        assert!(watch.lethal);
    }

    #[test]
    fn human_bomb_kills_server_bot_through_production_damage_dispatch() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let shooter = register_connection(&state, tx);
        join_arena(&state, shooter, 0);
        let (bot_id, bot_life, x, y, shooter_life) = {
            let mut guard = state.lock().expect("state");
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.energy = 1;
            (
                bot.player.id,
                bot.life_seq,
                bot.x as u16,
                bot.y as u16,
                guard.clients[&shooter].life_seq,
            )
        };
        let hit = ProjectileHit {
            projectile_id: 703,
            owner_id: shooter,
            owner_life_seq: shooter_life,
            victim_id: bot_id,
            victim_life_seq: bot_life,
            weapon_data: 3,
            x,
            y,
            tick: 52,
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        let bot = guard.bot(bot_id).expect("bot remains rostered");
        assert!(!bot.alive());
        assert_eq!(bot.life_seq, bot_life + 1);
        assert_eq!(guard.clients[&shooter].player.wins, 1);
    }

    #[test]
    fn active_burst_deals_authored_damage_to_server_bot_and_can_kill() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let shooter = register_connection(&state, tx);
        join_arena(&state, shooter, 0);
        let (bot_id, first_life, x, y, shooter_life) = {
            let mut guard = state.lock().expect("state");
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.energy = 701;
            bot.max_energy = 1_700;
            (
                bot.player.id,
                bot.life_seq,
                bot.x as u16,
                bot.y as u16,
                guard.clients[&shooter].life_seq,
            )
        };
        let first = ProjectileHit {
            projectile_id: 704,
            owner_id: shooter,
            owner_life_seq: shooter_life,
            victim_id: bot_id,
            victim_life_seq: first_life,
            weapon_data: 7,
            x,
            y,
            tick: 53,
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &first),
            DetectedHitApplication::Applied
        );
        {
            let guard = state.lock().expect("state");
            let bot = guard.bot(bot_id).expect("bot remains rostered");
            assert_eq!(bot.life_seq, first_life);
            assert_eq!(
                bot.energy, 1,
                "Chaos BurstDamageLevel=700000 must deal exactly 700 energy"
            );
        }

        let second = ProjectileHit {
            projectile_id: 705,
            tick: 54,
            ..first
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &second),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        let bot = guard.bot(bot_id).expect("bot remains rostered");
        assert!(!bot.alive());
        assert_eq!(bot.life_seq, first_life + 1);
        assert_eq!(guard.clients[&shooter].player.wins, 1);
    }

    #[test]
    fn human_primary_proximity_explosion_includes_nearby_server_bot_splash() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let (primary_tx, _primary_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let primary = register_connection(&state, primary_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, primary, 0);

        let (bot_id, bot_life, shooter_life, primary_life) = {
            let mut guard = state.lock().expect("state");
            let shooter_frequency = guard.clients[&shooter].player.frequency;
            guard.clients.get_mut(&primary).unwrap().player.frequency =
                shooter_frequency.wrapping_add(1);
            set_damage_fixture_client(&mut guard, shooter, 8_200, 8_200, 1_700);
            set_damage_fixture_client(&mut guard, primary, 8_380, 8_200, 1_700);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.player.frequency = shooter_frequency.wrapping_add(2);
            bot.x = 8_330.0;
            bot.y = 8_200.0;
            bot.energy = 1_700;
            bot.max_energy = 1_700;
            (
                bot.player.id,
                bot.life_seq,
                guard.clients[&shooter].life_seq,
                guard.clients[&primary].life_seq,
            )
        };
        let proximity_bomb = 4 | (2 << 5);
        let hit = ProjectileHit {
            projectile_id: 704,
            owner_id: shooter,
            owner_life_seq: shooter_life,
            victim_id: primary,
            victim_life_seq: primary_life,
            weapon_data: proximity_bomb,
            x: 8_290,
            y: 8_200,
            tick: 53,
        };

        let splash_witnesses = {
            let guard = state.lock().expect("state");
            let mut transaction = prepare_server_detected_damage_transaction(&guard, &hit)
                .expect("complete mixed damage transaction");
            transaction.collision_witness = Some(
                crate::authority_projectiles::ProjectileCollisionWitness {
                    projectile_id: hit.projectile_id,
                    shot_group_id: 1,
                    owner: hit.owner_identity(),
                    target: hit.victim_identity(),
                    projectile_created_tick: 40,
                    collision_tick: hit.tick,
                    requested_target_tick: hit.tick,
                    selected_target_tick: Some(hit.tick),
                    target_motion_segment: None,
                    rewind_ticks: 0,
                    projectile_x: f32::from(hit.x),
                    projectile_y: f32::from(hit.y),
                    target_current_x: 8_380.0,
                    target_current_y: 8_200.0,
                    target_anchor_x: 8_380.0,
                    target_anchor_y: 8_200.0,
                    center_distance: 90.0,
                    target_radius: 14.0,
                    proximity_trigger_radius: Some(72.0),
                    explosion_radius: Some(160.0),
                    history_source:
                        crate::authority_projectiles::MotionHistorySource::UnifiedHistoryUnavailable,
                    contact_reason:
                        crate::authority_projectiles::ProjectileContactReason::ProximityClosestApproach,
                    presentation_geometry:
                        crate::combat_timeline::ProjectilePresentationWitnessV1::not_evaluated(),
                },
            );
            transaction.collision_damage_witnesses()
        };
        let bot_splash_witness = splash_witnesses
            .iter()
            .find(|witness| witness.target.player_id == bot_id)
            .expect("bot splash owns a frozen per-victim geometry witness");
        assert!(!bot_splash_witness.primary);
        assert_eq!(bot_splash_witness.target.life_seq, bot_life);
        assert_eq!(bot_splash_witness.damage_center_distance, 40.0);
        assert_eq!(bot_splash_witness.unprotected_damage, 625);
        assert_eq!(bot_splash_witness.adjusted_damage, 625);

        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        let bot = guard.bot(bot_id).expect("bot remains rostered");
        assert_eq!(bot.life_seq, bot_life);
        assert_eq!(bot.energy, 1_075, "40px radial splash must deal 625");
        let watch = guard.clients[&shooter]
            .pending_realtime_events
            .queue
            .iter()
            .filter_map(|delivery| {
                let crate::realtime_event::RealtimeEventPayload::WatchDamage(watch) =
                    delivery.payload
                else {
                    return None;
                };
                (watch.victim_player_id == bot_id).then_some(watch)
            })
            .next()
            .expect("bot splash must be visible to the human shooter");
        assert_eq!(watch.damage, 625);
        assert_eq!(watch.energy_before, 1_700);
        assert_eq!(watch.energy_after, 1_075);
    }

    #[test]
    fn bot_primary_explosion_is_one_mixed_human_and_bot_splash_transaction() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 2,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let (bystander_tx, _bystander_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let bystander = register_connection(&state, bystander_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, bystander, 0);

        let (primary_bot, primary_life, splash_bot, shooter_life) = {
            let mut guard = state.lock().expect("state");
            let shooter_frequency = guard.clients[&shooter].player.frequency;
            set_damage_fixture_client(&mut guard, shooter, 700, 1_000, 1_700);
            set_damage_fixture_client(&mut guard, bystander, 1_080, 1_000, 1_700);
            guard.clients.get_mut(&bystander).unwrap().player.frequency =
                shooter_frequency.wrapping_add(1);
            let primary_bot = guard.bots[0].player.id;
            let splash_bot = guard.bots[1].player.id;
            guard.bots[0].player.frequency = shooter_frequency.wrapping_add(2);
            guard.bots[0].x = 1_120.0;
            guard.bots[0].y = 1_000.0;
            guard.bots[0].energy = 1_700;
            guard.bots[0].max_energy = 1_700;
            guard.bots[1].player.frequency = shooter_frequency.wrapping_add(3);
            guard.bots[1].x = 1_040.0;
            guard.bots[1].y = 1_000.0;
            guard.bots[1].energy = 1_700;
            guard.bots[1].max_energy = 1_700;
            (
                primary_bot,
                guard.bots[0].life_seq,
                splash_bot,
                guard.clients[&shooter].life_seq,
            )
        };
        let hit = ProjectileHit {
            projectile_id: 705,
            owner_id: shooter,
            owner_life_seq: shooter_life,
            victim_id: primary_bot,
            victim_life_seq: primary_life,
            weapon_data: 4 | (1 << 5),
            x: 1_000,
            y: 1_000,
            tick: 54,
        };

        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        assert_eq!(guard.bot(primary_bot).unwrap().energy, 1_513);
        assert_eq!(guard.clients[&bystander].server_energy, 1_325);
        assert_eq!(guard.bot(splash_bot).unwrap().energy, 1_138);
        let watch_events = guard.clients[&shooter]
            .pending_realtime_events
            .queue
            .iter()
            .filter_map(|delivery| {
                let crate::realtime_event::RealtimeEventPayload::WatchDamage(watch) =
                    delivery.payload
                else {
                    return None;
                };
                (watch.projectile_id == 705).then_some(watch)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            watch_events.first().map(|watch| watch.victim_player_id),
            Some(primary_bot),
            "the immutable primary collision must publish before splash victims"
        );
        assert!(
            watch_events
                .windows(2)
                .all(|pair| { pair[1].damage_seq == pair[0].damage_seq.wrapping_add(1) })
        );
        let victims = watch_events
            .iter()
            .map(|watch| watch.victim_player_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            victims,
            [primary_bot, bystander, splash_bot].into_iter().collect()
        );
    }

    #[test]
    fn mixed_human_bot_explosion_defers_and_retries_without_partial_damage() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let (primary_tx, _primary_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let primary = register_connection(&state, primary_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, primary, 0);
        let (bot_id, hit) = {
            let mut guard = state.lock().expect("state");
            let shooter_frequency = guard.clients[&shooter].player.frequency;
            guard.clients.get_mut(&primary).unwrap().player.frequency =
                shooter_frequency.wrapping_add(1);
            set_damage_fixture_client(&mut guard, shooter, 8_200, 8_200, 1_700);
            set_damage_fixture_client(&mut guard, primary, 8_380, 8_200, 1_700);
            let bot = guard.bots.first_mut().unwrap();
            bot.player.frequency = shooter_frequency.wrapping_add(2);
            bot.x = 8_330.0;
            bot.y = 8_200.0;
            bot.energy = 1_700;
            bot.max_energy = 1_700;
            let bot_id = bot.player.id;
            fill_realtime_queue(&mut guard, shooter, 7_104);
            (
                bot_id,
                ProjectileHit {
                    projectile_id: 706,
                    owner_id: shooter,
                    owner_life_seq: guard.clients[&shooter].life_seq,
                    victim_id: primary,
                    victim_life_seq: guard.clients[&primary].life_seq,
                    weapon_data: 4 | (2 << 5),
                    x: 8_290,
                    y: 8_200,
                    tick: 55,
                },
            )
        };

        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::DeferredCapacity
        );
        {
            let mut guard = state.lock().expect("state");
            assert_eq!(guard.clients[&primary].server_energy, 1_700);
            assert_eq!(guard.bot(bot_id).unwrap().energy, 1_700);
            assert!(guard.pending_damage_transactions.contains_key(&706));
            guard
                .clients
                .get_mut(&shooter)
                .unwrap()
                .pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
            guard.bot_mut(bot_id).unwrap().x = 15_000.0;
            guard.bot_mut(bot_id).unwrap().y = 15_000.0;
        }
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        assert_eq!(guard.clients[&primary].server_energy, 1_232);
        assert_eq!(
            guard.bot(bot_id).unwrap().energy,
            1_075,
            "retry consumes the frozen collision-time bot anchor"
        );
        assert!(!guard.pending_damage_transactions.contains_key(&706));
    }

    #[test]
    fn one_explosion_can_kill_two_exact_life_bots_and_award_both_scores() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 2,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let shooter = register_connection(&state, tx);
        join_arena(&state, shooter, 0);
        let (first_id, first_life, second_id, second_life, hit) = {
            let mut guard = state.lock().expect("state");
            let shooter_frequency = guard.clients[&shooter].player.frequency;
            set_damage_fixture_client(&mut guard, shooter, 700, 1_000, 1_700);
            for (index, x) in [1_040.0, 1_080.0].into_iter().enumerate() {
                let bot = &mut guard.bots[index];
                bot.player.frequency = shooter_frequency.wrapping_add(index as u16 + 1);
                bot.x = x;
                bot.y = 1_000.0;
                bot.energy = 1;
                bot.max_energy = 1_700;
            }
            (
                guard.bots[0].player.id,
                guard.bots[0].life_seq,
                guard.bots[1].player.id,
                guard.bots[1].life_seq,
                ProjectileHit {
                    projectile_id: 707,
                    owner_id: shooter,
                    owner_life_seq: guard.clients[&shooter].life_seq,
                    victim_id: guard.bots[0].player.id,
                    victim_life_seq: guard.bots[0].life_seq,
                    weapon_data: 4 | (1 << 5),
                    x: 1_000,
                    y: 1_000,
                    tick: 56,
                },
            )
        };

        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        assert!(!guard.bot(first_id).unwrap().alive());
        assert!(!guard.bot(second_id).unwrap().alive());
        assert_eq!(guard.bot(first_id).unwrap().life_seq, first_life + 1);
        assert_eq!(guard.bot(second_id).unwrap().life_seq, second_life + 1);
        assert_eq!(guard.clients[&shooter].player.wins, 2);
        assert!(guard.pending_damage_transactions.is_empty());
    }

    #[test]
    fn projectile_terminal_reserves_spectator_capacity_before_bot_damage_and_survives_soft_cutover()
    {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 2,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().unwrap().initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let observer = register_connection(&state, tx);
        join_arena(&state, observer, 8);
        let hit = {
            let mut guard = state.lock().unwrap();
            let shooter = guard.bots[0].runtime_identity();
            let victim = &guard.bots[1];
            let hit = ProjectileHit {
                projectile_id: 901,
                owner_id: shooter.player_id(),
                owner_life_seq: shooter.life.life_seq,
                victim_id: victim.player.id,
                victim_life_seq: victim.life_seq,
                weapon_data: 1,
                x: victim.x.round() as u16,
                y: victim.y.round() as u16,
                tick: 51,
            };
            let source = crate::realtime_event::RealtimeWeaponEvent {
                source_player_id: hit.owner_id,
                source_session_id: shooter.session.session_id.get(),
                source_life_seq: hit.owner_life_seq,
                shot_group_id: 777,
                x: 8_192.0,
                y: 8_192.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: 0.0,
                frequency: guard.bots[0].player.frequency,
                ship: 0,
                weapon_data: 1,
            };
            guard.active_projectiles.record(
                ProjectileRecord {
                    id: hit.projectile_id,
                    shot_group_id: 777,
                    shot_index: 0,
                    owner_id: hit.owner_id,
                    owner_life_seq: hit.owner_life_seq,
                    presentation_source: Some(source),
                    combat_timeline: None,
                    weapon_data: 1,
                    x: hit.x,
                    y: hit.y,
                    timestamp: 1,
                    created_tick: 1,
                    velocity_x: 0,
                    velocity_y: 0,
                    expiry_tick: Some(551),
                    pos_x: f32::from(hit.x),
                    source_server_tick: None,
                    precise_velocity: None,
                    pos_y: f32::from(hit.y),
                    bounces_remaining: 0,
                    behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                    proximity_target: None,
                    proximity_sensor_end_tick: None,
                    proximity_highest_offset: 0.0,
                },
                128,
                1024,
            );
            // This spectator learned only a different child of the linked
            // group through late AOI state. It must still receive the terminal.
            let known = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
                shooter, 777, 902,
            )
            .unwrap();
            assert!(
                guard
                    .clients
                    .get_mut(&observer)
                    .unwrap()
                    .threat_presentation
                    .observe_projectile_queued(known, 40, 1)
            );
            fill_realtime_queue(&mut guard, observer, 41);
            hit
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::DeferredCapacity
        );
        {
            let mut guard = state.lock().unwrap();
            assert_eq!(guard.bots[1].energy, 1700);
            guard
                .clients
                .get_mut(&observer)
                .unwrap()
                .pending_realtime_events
                .queue
                .clear();
        }
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let mut guard = state.lock().unwrap();
        assert_eq!(guard.bots[1].energy, 1500);
        let mut next = guard.active_player_runtime_identity(observer).unwrap();
        next.life.life_seq += 1;
        let queue = &mut guard
            .clients
            .get_mut(&observer)
            .unwrap()
            .pending_realtime_events;
        queue.retain_for_recipient(next);
        let terminal = queue
            .queue
            .iter()
            .filter_map(|envelope| match envelope.payload {
                crate::realtime_event::RealtimeEventPayload::ProjectileState(state)
                    if state.terminal =>
                {
                    Some(state)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].projectile_id, 901);
        assert_eq!(terminal[0].source.shot_group_id, 777);
        assert_eq!(terminal[0].expiry_tick, Some(hit.tick));
    }

    #[test]
    fn bot_emp_damage_dispatch_preserves_engines_and_pauses_only_recharge() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 2,
                bot_ship_types: vec![5, 0],
                ..Default::default()
            },
            ..Default::default()
        }));
        let hit = {
            let mut guard = state.lock().unwrap();
            guard.initialize_configured_bots();
            guard.bots[0].x = 8400.0;
            guard.bots[0].y = 8200.0;
            guard.bots[1].x = 8200.0;
            guard.bots[1].y = 8200.0;
            guard.bots[1].xradar = false;
            ProjectileHit {
                projectile_id: 9_200,
                owner_id: guard.bots[0].player.id,
                owner_life_seq: guard.bots[0].life_seq,
                victim_id: guard.bots[1].player.id,
                victim_life_seq: guard.bots[1].life_seq,
                weapon_data: 4 | (1 << 5),
                x: 8200,
                y: 8200,
                tick: 100,
            }
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let mut guard = state.lock().unwrap();
        let bot = &mut guard.bots[1];
        assert_eq!(bot.energy, 950);
        assert_eq!(bot.emp_until_tick, Some(500));
        assert_eq!(bot.engine_shutdown_until_tick, None);
        assert!(!bot.shutdown_active(200));
        bot.advance_energy_at_tick(100, 200);
        assert_eq!(bot.energy, 950);
        bot.advance_energy_at_tick(301, 501);
        assert_eq!(bot.energy, 951);
    }

    #[test]
    fn fifteen_mixed_ffa_bots_damage_each_other_and_award_bot_kills() {
        // Exercise damage admission/accounting, independently of the browser's
        // projectile presentation and the registry's swept collision tests.
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 15,
                bot_ship_types: vec![0, 3, 4, 7],
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().unwrap().initialize_configured_bots();
        let frequencies = state
            .lock()
            .unwrap()
            .bots
            .iter()
            .map(|bot| bot.player.frequency)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(frequencies.len(), 15);
        for index in 0..15 {
            let hit = {
                let guard = state.lock().unwrap();
                let shooter = &guard.bots[index];
                let victim = &guard.bots[(index + 1) % 15];
                assert_ne!(shooter.player.frequency, victim.player.frequency);
                ProjectileHit {
                    projectile_id: 9_000 + index as u32,
                    owner_id: shooter.player.id,
                    owner_life_seq: shooter.life_seq,
                    victim_id: victim.player.id,
                    victim_life_seq: victim.life_seq,
                    weapon_data: 2 | (2 << 5),
                    x: victim.x.round() as u16,
                    y: victim.y.round() as u16,
                    tick: 51 + index as u32,
                }
            };
            assert_eq!(
                try_apply_server_detected_hit_with_broadcast(&state, &hit),
                DetectedHitApplication::Applied
            );
        }
        let lethal = {
            let mut guard = state.lock().unwrap();
            for bot in &guard.bots {
                assert_eq!(
                    bot.energy, 1_300,
                    "each bot must receive one level-three hit"
                );
                assert!(bot.alive());
            }
            guard.bots[0].energy = 1;
            ProjectileHit {
                projectile_id: 9_100,
                owner_id: guard.bots[1].player.id,
                owner_life_seq: guard.bots[1].life_seq,
                victim_id: guard.bots[0].player.id,
                victim_life_seq: guard.bots[0].life_seq,
                weapon_data: 2 | (2 << 5),
                x: guard.bots[0].x.round() as u16,
                y: guard.bots[0].y.round() as u16,
                tick: 100,
            }
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &lethal),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().unwrap();
        assert!(!guard.bots[0].alive());
        assert_eq!(guard.bots[0].player.losses, 1);
        assert_eq!(guard.bots[1].player.wins, 1);
    }

    #[test]
    fn server_bot_projectile_uses_same_damage_ledger_for_human_victim() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let victim = register_connection(&state, tx);
        join_arena(&state, victim, 0);
        let (bot_id, bot_life, victim_life, energy_before) = {
            let guard = state.lock().expect("state");
            (
                guard.bots[0].player.id,
                guard.bots[0].life_seq,
                guard.clients[&victim].life_seq,
                guard.clients[&victim].server_energy,
            )
        };
        let hit = ProjectileHit {
            projectile_id: 701,
            owner_id: bot_id,
            owner_life_seq: bot_life,
            victim_id: victim,
            victim_life_seq: victim_life,
            weapon_data: 1,
            x: 100,
            y: 100,
            tick: 51,
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        let guard = state.lock().expect("state");
        assert!(guard.clients[&victim].server_energy < energy_before);
        assert_eq!(guard.clients[&victim].recent_damagers.len(), 1);
        assert_eq!(
            guard.clients[&victim].recent_damagers[0].0.player_id,
            bot_id
        );
    }

    #[test]
    fn server_bot_lethal_projectile_awards_bot_win_and_human_loss() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let victim = register_connection(&state, tx);
        join_arena(&state, victim, 0);
        let (bot_id, bot_life, victim_life, bot_wins, victim_losses) = {
            let mut guard = state.lock().expect("state");
            guard
                .clients
                .get_mut(&victim)
                .expect("victim")
                .server_energy = 1;
            (
                guard.bots[0].player.id,
                guard.bots[0].life_seq,
                guard.clients[&victim].life_seq,
                guard.bots[0].player.wins,
                guard.clients[&victim].player.losses,
            )
        };
        let decoy = {
            let mut guard = state.lock().expect("state");
            let decoy = crate::state::ServerDecoy {
                presentation_id: 0,
                presentation_source: None,
                source_input_sequence: None,
                owner: guard.active_player_runtime_identity(victim).unwrap(),
                source_weapon_data: 6,
                frequency: guard.clients[&victim].player.frequency,
                ship: guard.clients[&victim].player.ship,
                position_x: 100.0,
                position_y: 100.0,
                velocity_x: 10.0,
                velocity_y: 0.0,
                orientation: 0.25,
                created_tick: 50,
                last_step_tick: 50,
                expires_at_tick: 500,
            };
            guard.active_decoys.push(decoy);
            decoy
        };
        let hit = ProjectileHit {
            projectile_id: 702,
            owner_id: bot_id,
            owner_life_seq: bot_life,
            victim_id: victim,
            victim_life_seq: victim_life,
            weapon_data: 1,
            x: 100,
            y: 100,
            tick: 52,
        };

        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );

        let guard = state.lock().expect("state");
        assert!(!guard.clients[&victim].alive);
        assert_eq!(guard.clients[&victim].player.losses, victim_losses + 1);
        assert_eq!(guard.bots[0].player.wins, bot_wins + 1);
        assert_eq!(
            guard.active_decoys,
            vec![decoy],
            "real death commit preserves decoy flight"
        );
        assert_ne!(guard.clients[&victim].life_seq, decoy.owner.life.life_seq);
    }

    /// The reference removes a projectile by its own alive time or its own explosion - it computes
    /// `end_tick = local_timestamp + GetWeaponTotalAliveTime(type, alternate)` and nothing in that
    /// removal path looks at the shooter - so a destroyed player's live shot keeps flying and keeps
    /// its shooter's authority for as long as it does. This is the behaviour the previous design
    /// inverted by clearing the dead actor's projectiles on death.
    #[test]
    fn destroyed_shooters_live_projectile_survives_death_and_stays_attributable() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let human = register_connection(&state, tx);
        join_arena(&state, human, 0);
        let collision_tick = 9_000;
        let mut guard = state.lock().expect("state");
        let human_life = guard.clients[&human].life_seq;
        let runtime = guard
            .active_actor_runtime_identity(human)
            .expect("a joined human has a runtime identity");
        let projectile =
            |id, group, owner_id, owner_life_seq, weapon_data, x, y| ProjectileRecord {
                id,
                shot_group_id: group,
                shot_index: 0,
                owner_id,
                owner_life_seq,
                combat_timeline: None,
                presentation_source: None,
                weapon_data,
                x,
                y,
                timestamp: collision_tick,
                created_tick: collision_tick,
                velocity_x: 0,
                velocity_y: 0,
                expiry_tick: None,
                pos_x: f32::from(x),
                source_server_tick: None,
                precise_velocity: None,
                pos_y: f32::from(y),
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            };

        let mut shot = projectile(901, 901, human, human_life, 1, 1_200, 1_000);
        // The weapon's own alive time, not its shooter's life, is what ends the flight.
        shot.expiry_tick = Some(collision_tick + 30);
        guard.active_projectiles.record(shot, 128, 1_024);

        guard.retire_projectile_owner_after_damage_death(runtime, collision_tick);

        assert!(
            guard
                .active_projectiles
                .iter()
                .any(|record| record.id == 901),
            "a destroyed player's live shot must keep flying until its own alive time ends"
        );

        // Advance the life so the retired branch is the only thing that can attribute the shot.
        guard.clients.get_mut(&human).expect("client").life_seq = human_life + 1;
        assert!(
            guard.actor_runtime_matches_damage_collision(runtime, collision_tick + 7_000),
            "a retired shooter stays authoritative for its own shots long after the death tick"
        );

        // The memory exists only to serve flight that the server still authored: once the weapon's
        // own lifetime ends, nothing may keep the retired life privileged.
        // A collision it lands well after the death is still attributed to the retired life, because
        // the authority behind the hit is the record the server authored rather than a roster entry.
        let later_hit = crate::authority_projectiles::ProjectileHit {
            projectile_id: 901,
            owner_id: human,
            owner_life_seq: human_life,
            victim_id: human,
            victim_life_seq: human_life + 1,
            weapon_data: 1,
            x: 1_200,
            y: 1_000,
            tick: collision_tick + 20,
        };
        assert_eq!(
            damage_shooter_runtime(&guard, &later_hit),
            Some(runtime),
            "a shot that lands after its shooter died keeps its own authority"
        );
        // The window is pinned to a live record on purpose: a hit that merely names a retired life,
        // or a projectile the server no longer holds, cannot ride it.
        let forged = crate::authority_projectiles::ProjectileHit {
            projectile_id: 999_999,
            ..later_hit
        };
        assert_eq!(
            damage_shooter_runtime(&guard, &forged),
            None,
            "a hit for a projectile the server does not hold must not be attributed"
        );

        // Let the weapon's own alive time end the flight, exactly as the production tick does.
        guard
            .active_projectiles
            .step(collision_tick + 31, |_x, _y| false, &[]);
        assert!(
            !guard
                .active_projectiles
                .iter()
                .any(|record| record.id == 901),
            "the alive time must end the flight on its own schedule"
        );
        guard.prune_retired_projectile_owners();
        assert!(
            guard.retired_projectile_owners.is_empty(),
            "a retired life with no remaining flight must stop being remembered"
        );
        assert!(
            !guard.actor_runtime_matches_damage_collision(runtime, collision_tick + 7_000),
            "a retired life whose shots are gone cannot attribute anything"
        );
    }

    #[test]
    fn late_kill_credit_is_session_scoped_for_every_human_bot_pair() {
        for bot_source in [false, true] {
            for bot_victim in [false, true] {
                for replace_session in [false, true] {
                    let shared = Arc::new(Mutex::new(ServerState {
                        config: crate::config::ServerConfig {
                            bot_count: 2,
                            ..Default::default()
                        },
                        ..ServerState::default()
                    }));
                    shared.lock().unwrap().initialize_configured_bots();
                    let (tx, _rx) = mpsc::channel();
                    let a = register_connection(&shared, tx.clone());
                    let b = register_connection(&shared, tx);
                    join_arena(&shared, a, 0);
                    join_arena(&shared, b, 0);
                    let mut state = shared.lock().unwrap();
                    set_damage_fixture_client(&mut state, a, 1000, 1000, 1700);
                    set_damage_fixture_client(&mut state, b, 1100, 1000, 1);
                    let source = if bot_source {
                        state.bots[0].player.id
                    } else {
                        a
                    };
                    let victim = if bot_victim {
                        state.bots[1].player.id
                    } else {
                        b
                    };
                    let runtime = state.active_actor_runtime_identity(source).unwrap();
                    let victim_runtime = state.active_actor_runtime_identity(victim).unwrap();
                    let replacement = state.allocate_player_session_id();
                    if bot_source {
                        let bot = state.bot_mut(source).unwrap();
                        bot.life_seq += 1;
                        bot.bounty = 9;
                        if replace_session {
                            bot.session_id = replacement;
                        }
                    } else {
                        state.advance_client_life_at_tick(source, 110).unwrap();
                        let client = state.clients.get_mut(&source).unwrap();
                        client.server_bounty = 9;
                        if replace_session {
                            client.session_id = replacement;
                        }
                    }
                    if bot_victim {
                        state.bot_mut(victim).unwrap().bounty = 47;
                    } else {
                        state.clients.get_mut(&victim).unwrap().server_bounty = 47;
                    }
                    let hit = ProjectileHit {
                        projectile_id: 123,
                        owner_id: source,
                        owner_life_seq: runtime.life.life_seq,
                        victim_id: victim,
                        victim_life_seq: victim_runtime.life.life_seq,
                        weapon_data: 1,
                        x: 1100,
                        y: 1000,
                        tick: 150,
                    };
                    let event = ServerComputedDamage {
                        damage_seq: 1,
                        projectile_id: 123,
                        server_tick: 150,
                        victim_id: victim,
                        shooter_id: source,
                        victim_life_seq: hit.victim_life_seq,
                        shooter_life_seq: hit.owner_life_seq,
                        weapon_data: 1,
                        damage: 400,
                        unprotected_damage: 400,
                        shield_until_tick: None,
                        shield_remaining_ticks: 0,
                        shield_lifetime_ticks: 0,
                        emp_until_tick: None,
                        energy_before: 1,
                        energy_after: 0,
                        lethal: true,
                    };
                    // Component boundary: damage admission has supplied this exact
                    // source runtime. The production-tick regression separately
                    // proves that earlier-life flight can reach this boundary.
                    if bot_victim {
                        assert!(build_bot_kill_pipeline(&mut state, &event, runtime).is_some());
                        assert!(build_bot_kill_pipeline(&mut state, &event, runtime).is_none());
                    } else {
                        assert!(
                            build_kill_pipeline(&mut state, &hit, runtime, Some(&event)).is_some()
                        );
                        assert!(
                            build_kill_pipeline(&mut state, &hit, runtime, Some(&event)).is_none()
                        );
                    }
                    let (score, bounty) = if bot_source {
                        let bot = state.bot(source).unwrap();
                        (&bot.player, bot.bounty)
                    } else {
                        let client = &state.clients[&source];
                        (&client.player, client.server_bounty)
                    };
                    let expected = if replace_session { (0, 0) } else { (1, 47) };
                    assert_eq!(
                        (score.wins, score.kill_points),
                        expected,
                        "bot_source={bot_source} bot_victim={bot_victim} replaced={replace_session}"
                    );
                    assert_eq!(
                        bounty, 9,
                        "cumulative credit must not change successor-life bounty"
                    );
                }
            }
        }
    }

    #[test]
    fn same_tick_mutual_human_bot_collisions_survive_death_order_and_capacity() {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let human = register_connection(&state, tx);
        join_arena(&state, human, 0);

        let collision_tick = 5_544;
        let (bot_id, human_life, bot_life, hits) = {
            let mut guard = state.lock().expect("state");
            let bot_id = guard.bots[0].player.id;
            let bot_life = guard.bots[0].life_seq;
            let human_life = guard.clients[&human].life_seq;
            set_damage_fixture_client(&mut guard, human, 1_000, 1_000, 1);
            guard.clients.get_mut(&human).unwrap().player.frequency = 1;
            {
                let bot = guard.bot_mut(bot_id).expect("bot");
                bot.x = 1_100.0;
                bot.y = 1_000.0;
                bot.energy = 1;
                bot.bounty = 100;
                bot.player.frequency = 2;
            }

            let projectile =
                |id, group, owner_id, owner_life_seq, weapon_data, x, y| ProjectileRecord {
                    id,
                    shot_group_id: group,
                    shot_index: 0,
                    owner_id,
                    owner_life_seq,
                    combat_timeline: None,
                    presentation_source: None,
                    weapon_data,
                    x,
                    y,
                    timestamp: collision_tick,
                    created_tick: collision_tick,
                    velocity_x: 0,
                    velocity_y: 0,
                    expiry_tick: None,
                    pos_x: f32::from(x),
                    source_server_tick: None,
                    precise_velocity: None,
                    pos_y: f32::from(y),
                    bounces_remaining: 0,
                    behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                    proximity_target: None,
                    proximity_sensor_end_tick: None,
                    proximity_highest_offset: 0.0,
                };
            // Preserve the production failure order: bot→human is applied
            // first, followed by the already-collided human→bot projectile.
            guard.active_projectiles.record(
                projectile(801, 801, bot_id, bot_life, 1, 1_000, 1_000),
                128,
                1_024,
            );
            guard.active_projectiles.record(
                projectile(802, 802, human, human_life, 3, 1_100, 1_000),
                128,
                1_024,
            );
            // A non-collided sibling in the bomb group is retained only while
            // the exact collision is frozen, then discarded with the old
            // life rather than resuming after capacity drains.
            guard.active_projectiles.record(
                projectile(803, 802, human, human_life, 3, 1_400, 1_400),
                128,
                1_024,
            );
            let exact_hull = |player_id, life_seq, x, frequency, source_kind| {
                let actor = guard
                    .active_actor_runtime_identity(player_id)
                    .expect("active collision fixture actor");
                let segment = crate::combat_timeline::MotionSegmentIdentity {
                    actor,
                    arena_stream_id: 1,
                    discontinuity_generation: 1,
                };
                PlayerHullSnapshot {
                    player_id,
                    life_seq,
                    x,
                    y: 1_000.0,
                    frequency,
                    radius_px: 14.0,
                    motion_segment: Some(segment),
                    motion_samples: vec![crate::combat_timeline::ActorMotionSampleV1 {
                        segment,
                        server_tick: collision_tick,
                        sample_time: crate::combat_timeline::PresentationSampleTime::from_tick(
                            collision_tick,
                        ),
                        x_q8: (x * 256.0) as i32,
                        y_q8: 1_000 * 256,
                        velocity_x_q8: 0,
                        velocity_y_q8: 0,
                        orientation_u16: 0,
                        protocol_direction: 0,
                        hull_radius_q8: 14 * 256,
                        safe_or_attached: 0,
                        source_kind,
                    }],
                    motion_source_kind: Some(source_kind),
                    lag_compensation_ticks: 0,
                    life_started_tick: None,
                }
            };
            let hulls = vec![
                exact_hull(
                    human,
                    human_life,
                    1_000.0,
                    1,
                    crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
                ),
                exact_hull(
                    bot_id,
                    bot_life,
                    1_100.0,
                    2,
                    crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation,
                ),
            ];
            assert!(
                guard
                    .active_projectiles
                    .step_with_proximity_distance_transactional(
                        collision_tick - 1,
                        |_, _| false,
                        &hulls,
                        0.0,
                        0,
                    )
                    .is_empty()
            );
            let hits = guard
                .active_projectiles
                .step_with_proximity_distance_transactional(
                    collision_tick,
                    |_, _| false,
                    &hulls,
                    0.0,
                    0,
                );
            for hit in &hits {
                if hit.victim_id == human {
                    let transaction = prepare_server_detected_damage_transaction(&guard, hit)
                        .expect("human-victim collision freezes before either death");
                    guard
                        .pending_damage_transactions
                        .insert(hit.projectile_id, transaction);
                }
            }
            (bot_id, human_life, bot_life, hits)
        };
        assert_eq!(hits.len(), 2);
        let bot_to_human = hits
            .iter()
            .find(|hit| hit.owner_id == bot_id && hit.victim_id == human)
            .copied()
            .expect("bot collision");
        let human_to_bot = hits
            .iter()
            .find(|hit| hit.owner_id == human && hit.victim_id == bot_id)
            .copied()
            .expect("human collision");

        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &bot_to_human),
            DetectedHitApplication::Applied
        );
        let replacement_bounty = {
            let mut guard = state.lock().expect("state");
            assert!(!guard.clients[&human].alive);
            assert_ne!(guard.clients[&human].life_seq, human_life);
            assert!(
                guard
                    .active_projectiles
                    .contains_pending_hit(human_to_bot.projectile_id)
            );
            assert!(
                guard
                    .retired_projectile_owners
                    .iter()
                    .any(|(runtime, tick)| {
                        runtime.life.player_id == human
                            && runtime.life.life_seq == human_life
                            && *tick == collision_tick
                    })
            );
            let retired = guard
                .retired_projectile_owners
                .iter()
                .find_map(|(runtime, tick)| {
                    (runtime.life.player_id == human
                        && runtime.life.life_seq == human_life
                        && *tick == collision_tick)
                        .then_some(*runtime)
                })
                .expect("retired shooter token");
            assert!(guard.actor_runtime_matches_damage_collision(retired, collision_tick));
            assert!(guard.clients[&human].joined);
            assert_eq!(guard.clients[&human].session_id, retired.session.session_id);
            fill_realtime_queue(&mut guard, human, 7_000);
            guard.clients[&human].server_bounty
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &human_to_bot),
            DetectedHitApplication::DeferredCapacity,
            "delivery pressure retains the frozen same-tick collision"
        );
        {
            let mut guard = state.lock().expect("state");
            assert!(guard.bot(bot_id).expect("bot").alive());
            guard
                .clients
                .get_mut(&human)
                .unwrap()
                .pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
        }
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &human_to_bot),
            DetectedHitApplication::Applied
        );
        {
            let mut guard = state.lock().expect("state");
            guard
                .active_projectiles
                .resolve_hit(human_to_bot.projectile_id);
            guard.prune_retired_projectile_owners();
            let bot = guard.bot(bot_id).expect("bot remains rostered");
            assert!(!bot.alive());
            assert_ne!(bot.life_seq, bot_life);
            assert_eq!(bot.player.losses, 1);
            assert_eq!(bot.player.wins, 1);
            assert_eq!(guard.clients[&human].player.losses, 1);
            assert_eq!(guard.clients[&human].player.wins, 1);
            assert_eq!(guard.clients[&human].server_bounty, replacement_bounty);
            assert_eq!(guard.active_projectiles.pending_hit_count(), 0);
            // The reference retires a projectile by its own alive time or its own explosion, never
            // because its shooter died, so a destroyed player's shots stay in flight here and the
            // retired life stays authoritative for them. What must still hold - and what the old
            // "cleared on death" assertion was really standing in for - is that the retired life is
            // remembered exactly while the server still holds one of its records, and that nothing
            // is left to accumulate once its own flight is over.
            let dead_life_still_flying = guard
                .active_projectiles
                .iter()
                .any(|projectile| projectile.owner_identity().life_seq == human_life);
            assert_eq!(
                guard.retired_projectile_owners.is_empty(),
                !dead_life_still_flying,
                "a retired shooter is remembered exactly while its own shots are still registered"
            );
            guard.prune_retired_projectile_owners();
            assert_eq!(
                guard.retired_projectile_owners.is_empty(),
                !dead_life_still_flying,
                "pruning must neither forget a flying retired shooter nor keep a finished one"
            );
            assert!(guard.pending_damage_transactions.is_empty());
        }
    }
}
