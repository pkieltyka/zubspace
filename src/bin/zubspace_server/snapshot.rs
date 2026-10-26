//! Per-recipient AOI snapshot frames.
//!
//! See `docs/networking.md` and `docs/protocol_compatibility.md`. This module owns:
//!
//! - The per-tier snapshot intervals, derived from the configured tick
//!   rate: near frames go out every tick at 60 Hz, mid frames at 30 Hz.
//! - The per-recipient AOI computation that classifies every other
//!   joined player as Near / Mid / Far relative to the recipient's
//!   `last_position`, and the per-recipient emission step called
//!   once per server tick.
//!
//! Near entities carry field-masked deltas for position, velocity,
//! direction, energy, status, bounty, ping, and flag timer. The weapon bit is
//! retained only for legacy zero-stream fixtures; authenticated production
//! one-shots use the independent `0x7d` event carrier.
//! Mid entities carry radar-position deltas. Snapshot events carry
//! realtime combat events such as WATCH_DAMAGE.
//!
//! Wire format is owned by `src/protocol/snapshot.rs`, which is included by
//! both this binary and the browser client.
//!
//! ```text
//! type:u8 = 0x70
//! flags:u8 | snapshot_id:u16 | base_id:u16 | tick:u32 |
//! arena_stream_id:u64 |
//! near_count:u8 | mid_count:u8 | event_count:u8
//! near_entities[near_count]
//! mid_entities[mid_count]
//! events[event_count]
//! ```

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::authority_identity::{
    LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
};
#[allow(unused_imports)]
pub(crate) use crate::snapshot_wire::{
    AOI_PROXY_LIFETIME_SNAPSHOTS, NEAR_SNAPSHOT_HZ, SNAPSHOT_FIELD_BOUNTY,
    SNAPSHOT_FIELD_DIRECTION, SNAPSHOT_FIELD_ENERGY, SNAPSHOT_FIELD_FLAG_TIMER,
    SNAPSHOT_FIELD_LIFE_SEQ, SNAPSHOT_FIELD_PING, SNAPSHOT_FIELD_STATUS, SNAPSHOT_FIELD_VELOCITY_X,
    SNAPSHOT_FIELD_VELOCITY_Y, SNAPSHOT_FIELD_WEAPON, SNAPSHOT_FIELD_X, SNAPSHOT_FIELD_Y,
    SNAPSHOT_FLAG_FULL, SNAPSHOT_HISTORY_CAPACITY, SNAPSHOT_HZ, SNAPSHOT_PACKET, SnapshotEntity,
    SnapshotEntityState as NearEntityState, SnapshotEntityStateMap as SnapshotEntityMap,
    SnapshotEvent, SnapshotFrame, SnapshotHeader, SnapshotInputApplied, SnapshotLifecycleState,
    SnapshotPrizeState, SnapshotRepelDelivery, SnapshotRocketState, SnapshotWatchDamage,
    delta_encode_mid_tier, delta_encode_near_tier, next_snapshot_id, snapshot_id_is_forward,
};
use crate::state::ServerState;
use crate::visibility::{AoiTier, MID_AOI_RADIUS_PX, NEAR_AOI_RADIUS_PX};

/// Debug-server-only fault seam for the real-browser delayed baseline replay.
/// It omits only the repeated recipient-private PrizeState field; snapshot
/// lifecycle, positions, and DataChannel delivery remain live.
#[cfg(debug_assertions)]
fn qa_prize_baseline_delay_snapshots() -> u16 {
    static DELAY: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *DELAY.get_or_init(|| {
        std::env::var("ZUBSPACE_SERVER_TEST_PRIZE_BASELINE_DELAY_SNAPSHOTS")
            .ok()
            .map(|value| {
                value.parse::<u16>().unwrap_or_else(|_| {
                    panic!("ZUBSPACE_SERVER_TEST_PRIZE_BASELINE_DELAY_SNAPSHOTS must be a u16")
                })
            })
            .unwrap_or(0)
    })
}

#[cfg(debug_assertions)]
fn qa_prize_baseline_delay_released(
    client: &mut crate::state::ClientHandle,
    snapshot_id: u16,
) -> bool {
    let delay = qa_prize_baseline_delay_snapshots();
    if delay == 0 {
        return true;
    }
    let files_complete = client.map_transfer.as_ref().is_some_and(|transfer| {
        transfer.next_index >= transfer.file_count
            && transfer.pending_index.is_none()
            && transfer.deferred_successor_index.is_none()
            && transfer.active.is_none()
    });
    if !client.alive || client.last_security_valid != Some(true) || !files_complete {
        return false;
    }
    let start = *client
        .qa_prize_baseline_delay_start_snapshot
        .get_or_insert(snapshot_id);
    snapshot_id.wrapping_sub(start) >= delay
}

#[cfg(not(debug_assertions))]
fn qa_prize_baseline_delay_released(
    _client: &mut crate::state::ClientHandle,
    _snapshot_id: u16,
) -> bool {
    true
}

#[cfg(test)]
pub(crate) const SNAPSHOT_HEADER_LEN: usize = SnapshotHeader::HEADER_LEN;
#[cfg(test)]
pub(crate) const SNAPSHOT_ENTITY_HEADER_LEN: usize = SnapshotEntity::HEADER_LEN;

/// Load-shedding: pressure ratio (EWMA tick wallclock ÷ tick budget)
/// above which `emit_snapshots_for_tick` drops the mid-tier entries
/// from each snapshot. Picked so a healthy server (steady state at
/// p99 ≈ 5.5 ms vs the 16.7 ms budget at 60 Hz ≈ 0.33 pressure) never
/// shed; sustained 0.60+ pressure means the loop is spending most
/// of its budget and peripheral awareness is the right tradeoff
/// to drop first.
pub(crate) const LOAD_SHED_MID_THRESHOLD: f64 = 0.60;
/// Even under sustained MID-tier load shedding, refresh authoritative actor
/// membership often enough that the browser cannot expire a living in-AOI
/// actor. Half the client lifetime tolerates one lost refresh without a
/// same-life presentation gap while still shedding five out of six MID
/// payloads at the normal 60 Hz near cadence.
pub(crate) const LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS: u64 =
    AOI_PROXY_LIFETIME_SNAPSHOTS as u64 / 2;
const _: () = assert!(LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS > 0);
const _: () =
    assert!(LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS * 2 <= AOI_PROXY_LIFETIME_SNAPSHOTS as u64);
/// Phase 4 load-shedding: pressure ratio above which the whole
/// snapshot frame for a recipient is skipped. The next tick's
/// full-state fallback (delta base will be missing from history)
/// catches them up. At 0.80 pressure the loop is essentially
/// saturated and we'd otherwise start running over budget.
pub(crate) const LOAD_SHED_FRAME_THRESHOLD: f64 = 0.80;

/// Phase 3e-5: WATCH_DAMAGE event type byte inside a snapshot's
/// `events[]` region. Mirror of
/// `protocol::SnapshotEvent::TYPE_WATCH_DAMAGE`.
#[cfg(test)]
pub(crate) const SNAPSHOT_EVENT_WATCH_DAMAGE: u8 =
    crate::snapshot_wire::SnapshotEvent::TYPE_WATCH_DAMAGE;
/// Length of a `WatchDamage` event body. Mirror of
/// `protocol::SnapshotEvent::WATCH_DAMAGE_BODY_LEN`.
#[cfg(test)]
pub(crate) const SNAPSHOT_EVENT_WATCH_DAMAGE_BODY_LEN: usize =
    crate::snapshot_wire::SnapshotEvent::WATCH_DAMAGE_BODY_LEN;
/// Per-event header on the wire: `event_type:u8 | body_len:u8`.
/// Mirror of `protocol::SnapshotEvent::HEADER_LEN`.
#[cfg(test)]
pub(crate) const SNAPSHOT_EVENT_HEADER_LEN: usize = crate::snapshot_wire::SnapshotEvent::HEADER_LEN;

/// Server-side WATCH_DAMAGE event payload. The snapshot copy preserves the
/// existing damage-sequence consumer while an equivalent arena/runtime-bound
/// `0x7d` copy provides independent cross-channel delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingWatchDamage {
    pub(crate) victim_id: u16,
    pub(crate) shooter_id: u16,
    pub(crate) victim_life_seq: u32,
    pub(crate) shooter_life_seq: u32,
    pub(crate) weapon_data: u16,
    pub(crate) damage: u16,
    pub(crate) energy_before: u16,
    pub(crate) energy_after: u16,
    pub(crate) server_tick: u32,
    pub(crate) damage_seq: u32,
    pub(crate) projectile_id: u32,
    pub(crate) lethal: bool,
}

/// Per-recipient delivery envelope kept entirely server-side. The recipient
/// runtime is checked before queueing. WATCH_DAMAGE itself is a public,
/// self-contained combat fact: its victim and shooter runtime identities live
/// in the payload, so an observing recipient's soft life cutover must not
/// erase it. Emission therefore rechecks the never-reused recipient session,
/// while an arena leave/rejoin clears the queue explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingWatchDamageEnvelope {
    pub(crate) recipient: PlayerRuntimeIdentity,
    pub(crate) event: PendingWatchDamage,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PendingRealtimeEventEnvelope {
    /// Held/resumed flight is public session-scoped presentation, so it must
    /// survive a recipient death/respawn while remaining on the same stream.
    pub(crate) flight_transition: bool,
    pub(crate) recipient: PlayerRuntimeIdentity,
    pub(crate) event_id: u32,
    pub(crate) server_tick: u32,
    pub(crate) payload: crate::realtime_event::RealtimeEventPayload,
}

/// Dedicated authenticated one-shot queue capacity per recipient.
///
/// The supported arena has 768 player ids. A dense authoritative tick can
/// produce one weapon fact and one WATCH_DAMAGE fact per player, or 1,536
/// recipient envelopes. Retaining two complete bursts covers one burst being
/// held by send pacing while the next commits: `768 * 2 * 2 = 3,072`. The
/// The compile-time checks below use the actual largest 0x7d packet, so wire
/// growth cannot silently exceed either the RTC writer's 4,096
/// non-replaceable-packet or 4 MiB byte envelopes.
pub(crate) const DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY: usize =
    crate::limits::MAX_PLAYER_CLIENTS * 2 * 2;
const _: () = assert!(
    DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY < crate::rtc::RTC_APPLICATION_RELIABLE_QUEUE_PACKETS
);
const _: () = assert!(
    DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY * crate::realtime_event::MAX_REALTIME_EVENT_PACKET_LEN
        < crate::rtc::RTC_APPLICATION_QUEUE_BYTES
);

#[derive(Clone, Debug, Default)]
pub(crate) struct PendingRealtimeEvents {
    pub(crate) queue: VecDeque<PendingRealtimeEventEnvelope>,
}

impl PendingRealtimeEvents {
    pub(crate) fn retain_for_recipient(&mut self, recipient: PlayerRuntimeIdentity) {
        self.queue.retain(|envelope| match envelope.payload {
            crate::realtime_event::RealtimeEventPayload::OwnerWeaponsCancelled(_)
            | crate::realtime_event::RealtimeEventPayload::WatchDamage(_) => {
                envelope.recipient.session == recipient.session
            }
            crate::realtime_event::RealtimeEventPayload::ProjectileState(state)
                if state.terminal || envelope.flight_transition =>
            {
                envelope.recipient.session == recipient.session
            }
            _ => envelope.recipient == recipient,
        });
    }

    pub(crate) fn retain_public_combat_for_session(&mut self, recipient: PlayerSessionIdentity) {
        self.queue.retain(|envelope| {
            envelope.recipient.session == recipient
                && (matches!(envelope.payload, crate::realtime_event::RealtimeEventPayload::OwnerWeaponsCancelled(_)
                    | crate::realtime_event::RealtimeEventPayload::WatchDamage(_))
                    || matches!(envelope.payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(state) if state.terminal || envelope.flight_transition))
        });
    }
}

static WITHHELD_PROJECTILE_STATES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static WITHHELD_PROJECTILE_STATE_LOG_SECOND: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);

/// Total projectile states withheld because the client would reject them.
pub(crate) fn withheld_projectile_states() -> u64 {
    WITHHELD_PROJECTILE_STATES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The client rejects a projectile state that breaks
/// `RealtimeProjectileStateEvent::invalid_field`, so it is never sent. Warn at
/// most once a server second, naming the rule, so a regression shows in the
/// log rather than as a red error in players' chat.
fn warn_withheld_projectile_state(
    recipient_id: u16,
    event_id: u32,
    tick: u32,
    field: &'static str,
    state: crate::realtime_event::RealtimeProjectileStateEvent,
) {
    use std::sync::atomic::Ordering::Relaxed;
    let withheld_total = WITHHELD_PROJECTILE_STATES.fetch_add(1, Relaxed) + 1;
    let second = tick / 100;
    if WITHHELD_PROJECTILE_STATE_LOG_SECOND.swap(second, Relaxed) == second {
        return;
    }
    tracing::warn!(
        recipient_id,
        event_id,
        field,
        withheld_total,
        projectile_id = state.projectile_id,
        weapon_type = state.weapon_data & 0x1f,
        source_weapon_type = state.source.weapon_data & 0x1f,
        owner_id = state.source.source_player_id,
        owner_life_seq = state.source.source_life_seq,
        terminal = state.terminal,
        motion_frozen = state.motion_frozen,
        has_expiry = state.expiry_tick.is_some(),
        proximity = state.proximity.is_some(),
        behavior = state.behavior,
        "withheld a projectile state the client would reject"
    );
}

fn projectile_source_runtime(
    source: crate::realtime_event::RealtimeWeaponEvent,
) -> PlayerRuntimeIdentity {
    PlayerRuntimeIdentity::new(
        PlayerSessionIdentity::new(
            source.source_player_id,
            PlayerSessionId::new(source.source_session_id),
        ),
        LifeIdentity::new(source.source_player_id, source.source_life_seq),
    )
}

fn projectile_presentation_identity(
    payload: crate::realtime_event::RealtimeEventPayload,
) -> Option<crate::threat_presentation::ProjectilePresentationIdentity> {
    match payload {
        crate::realtime_event::RealtimeEventPayload::Weapon(source) => {
            matches!(source.weapon_data as u8 & 0x1f, 1 | 2 | 3 | 4 | 7 | 8)
                .then(|| {
                    crate::threat_presentation::ProjectilePresentationIdentity::shot_group(
                        projectile_source_runtime(source),
                        source.shot_group_id,
                    )
                })
                .flatten()
        }
        crate::realtime_event::RealtimeEventPayload::ProjectileState(state) => {
            if state.terminal {
                return None;
            }
            crate::threat_presentation::ProjectilePresentationIdentity::projectile(
                projectile_source_runtime(state.source),
                state.source.shot_group_id,
                state.projectile_id,
            )
        }
        crate::realtime_event::RealtimeEventPayload::WatchDamage(_)
        | crate::realtime_event::RealtimeEventPayload::OwnerWeaponsCancelled(_)
        | crate::realtime_event::RealtimeEventPayload::RepelMutations(_) => None,
    }
}

fn projectile_recovery_center(
    state: &ServerState,
    recipient_id: u16,
    near: &[(u16, NearEntityState)],
) -> Option<(f32, f32)> {
    let client = state.clients.get(&recipient_id)?;
    if client.player.ship >= 8 {
        // Spectators have no authoritative self hull in `near`. Their
        // server-owned observer anchor is sufficient for presentation only.
        let observer = client.aoi_observer_position.as_deref()?;
        let observer = extract_near_entity_state(observer)?;
        Some((f32::from(observer.x), f32::from(observer.y)))
    } else {
        near.iter()
            .find(|(id, _)| *id == recipient_id)
            .map(|(_, hull)| (f32::from(hull.x), f32::from(hull.y)))
    }
}

#[cfg(test)]
fn active_projectile_state_for_recipient(
    state: &ServerState,
    recipient_id: u16,
    x: f32,
    y: f32,
) -> Vec<(
    crate::threat_presentation::ProjectilePresentationIdentity,
    crate::realtime_event::RealtimeProjectileStateEvent,
)> {
    active_projectile_state_for_recipient_with_frozen_ids(
        state,
        recipient_id,
        x,
        y,
        &state.active_projectiles.frozen_projectile_ids(),
    )
}

fn active_projectile_state_for_recipient_with_frozen_ids(
    state: &ServerState,
    recipient_id: u16,
    recipient_x: f32,
    recipient_y: f32,
    frozen_ids: &std::collections::HashSet<u32>,
) -> Vec<(
    crate::threat_presentation::ProjectilePresentationIdentity,
    crate::realtime_event::RealtimeProjectileStateEvent,
)> {
    let Some(client) = state.clients.get(&recipient_id) else {
        return Vec::new();
    };
    let spectator = client.player.ship >= 8;
    if !client.joined || (!client.alive && !spectator) {
        return Vec::new();
    }
    let radius = NEAR_AOI_RADIUS_PX + crate::visibility::AOI_PREDICTIVE_LEAD_MAX_PX;
    let radius_squared = radius * radius;
    let mut states = state
        .active_projectiles
        .iter()
        .filter_map(|projectile| {
            let source = projectile.presentation_source?;
            let own_recovery = !spectator
                && projectile.owner_id == recipient_id
                && source.source_session_id == client.session_id.get()
                && projectile.combat_timeline.is_some();
            if (!own_recovery
                && (projectile.owner_id == recipient_id
                    || (!spectator && source.frequency == client.player.frequency)))
                || source.source_player_id != projectile.owner_id
                || source.source_life_seq != projectile.owner_life_seq
                || source.shot_group_id != projectile.shot_group_id
            {
                return None;
            }
            let dx = projectile.pos_x - recipient_x;
            let dy = projectile.pos_y - recipient_y;
            let distance_squared = dx * dx + dy * dy;
            if distance_squared > radius_squared {
                return None;
            }
            let identity = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
                projectile_source_runtime(source),
                projectile.shot_group_id,
                projectile.id,
            )?;
            let motion_frozen = frozen_ids.contains(&projectile.id);
            if client.threat_presentation.exact_projectile_known(identity)
                && !client.projectile_refresh_pending.contains(&projectile.id)
                && motion_frozen
                    == client
                        .projectile_motion_frozen_published
                        .contains(&projectile.id)
            {
                return None;
            }
            let behavior = match projectile.behavior {
                crate::gameplay_rules::ProjectileBehavior::Standard
                | crate::gameplay_rules::ProjectileBehavior::BouncingBullet => {
                    crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD
                }
                crate::gameplay_rules::ProjectileBehavior::Burst { active: false } => {
                    crate::realtime_event::PROJECTILE_BEHAVIOR_BURST_INACTIVE
                }
                crate::gameplay_rules::ProjectileBehavior::Burst { active: true } => {
                    crate::realtime_event::PROJECTILE_BEHAVIOR_BURST_ACTIVE
                }
            };
            let proximity = match (
                projectile.proximity_target,
                projectile.proximity_sensor_end_tick,
            ) {
                (Some(target), Some(sensor_end_tick)) => {
                    Some(crate::realtime_event::RealtimeProjectileProximityState {
                        target_player_id: target.player_id,
                        target_life_seq: target.life_seq,
                        sensor_end_tick,
                        highest_axis_offset: projectile.proximity_highest_offset,
                    })
                }
                (None, None) => None,
                _ => {
                    debug_assert!(false, "proximity target and deadline must be atomic");
                    None
                }
            };
            Some((
                distance_squared,
                identity,
                crate::realtime_event::RealtimeProjectileStateEvent {
                    source_input_sequence: projectile
                        .combat_timeline
                        .map(|binding| binding.owner_input_sequence),
                    terminal: false,
                    motion_frozen,
                    source,
                    projectile_id: projectile.id,
                    source_server_tick: projectile
                        .source_server_tick
                        .unwrap_or(projectile.created_tick),
                    source_projectile_index: projectile.shot_index,
                    weapon_data: projectile.weapon_data,
                    position_x: projectile.pos_x,
                    position_y: projectile.pos_y,
                    velocity_x: projectile.motion_velocity().x,
                    velocity_y: projectile.motion_velocity().y,
                    expiry_tick: projectile.expiry_tick,
                    behavior,
                    bounces_remaining: projectile.bounces_remaining,
                    proximity,
                },
            ))
        })
        .collect::<Vec<_>>();
    for decoy in &state.active_decoys {
        let Some(source) = decoy.presentation_source else {
            continue;
        };
        let own = source.source_player_id == recipient_id
            && source.source_session_id == client.session_id.get();
        if !spectator
            && !own
            && (source.source_player_id == recipient_id
                || source.frequency == client.player.frequency)
        {
            continue;
        }
        let dx = decoy.position_x - recipient_x;
        let dy = decoy.position_y - recipient_y;
        let distance_squared = dx * dx + dy * dy;
        if distance_squared > radius_squared {
            continue;
        }
        let Some(identity) = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
            projectile_source_runtime(source),
            source.shot_group_id,
            decoy.presentation_id,
        ) else {
            continue;
        };
        if client.threat_presentation.exact_projectile_known(identity)
            && !client
                .projectile_refresh_pending
                .contains(&decoy.presentation_id)
        {
            continue;
        }
        states.push((
            distance_squared,
            identity,
            crate::realtime_event::RealtimeProjectileStateEvent {
                source_input_sequence: decoy.source_input_sequence,
                terminal: false,
                motion_frozen: false,
                source,
                projectile_id: decoy.presentation_id,
                source_server_tick: decoy.created_tick,
                source_projectile_index: 0,
                weapon_data: decoy.source_weapon_data,
                position_x: decoy.position_x,
                position_y: decoy.position_y,
                velocity_x: decoy.velocity_x,
                velocity_y: decoy.velocity_y,
                expiry_tick: Some(decoy.expires_at_tick),
                behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
                bounces_remaining: 0,
                proximity: None,
            },
        ));
    }
    states.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    states
        .into_iter()
        .map(|(_, identity, state)| (identity, state))
        .collect()
}

impl std::ops::Deref for PendingWatchDamageEnvelope {
    type Target = PendingWatchDamage;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}

/// Per-recipient queue of pending snapshot events. Drained into the
/// next snapshot frame's `events[]` region and cleared after emit.
///
/// This compatibility queue deliberately retains the snapshot wire's one-byte
/// event-count semantics. Authoritative one-shot delivery uses the separate
/// `PendingRealtimeEvents` queue and is never evicted by this cap.
#[derive(Clone, Debug, Default)]
pub(crate) struct PendingSnapshotEvents {
    pub(crate) watch_damage: VecDeque<PendingWatchDamageEnvelope>,
}

impl PendingSnapshotEvents {
    fn len(&self) -> usize {
        self.watch_damage.len()
    }

    pub(crate) fn retain_for_recipient(&mut self, recipient: PlayerRuntimeIdentity) {
        self.watch_damage
            .retain(|envelope| envelope.recipient.session == recipient.session);
    }

    pub(crate) fn retain_for_session(&mut self, recipient: PlayerSessionIdentity) {
        self.watch_damage
            .retain(|envelope| envelope.recipient.session == recipient);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RealtimeEventPressureDisposition {
    #[cfg_attr(not(test), allow(dead_code))]
    Rejected,
    Deferred,
}

/// Capacity reservation for one gameplay transaction. `ServerState` is held
/// exclusively from reservation through commit, so no other action can spend
/// these per-player slots in between. The reservation records player ids (not
/// reusable runtime identities) because a lethal damage transaction may
/// advance the victim life before its self-contained old-life fact is queued.
pub(crate) struct RealtimeEventReservation {
    slots_by_player: std::collections::BTreeMap<u16, usize>,
}

pub(crate) struct RealtimeEventDelivery {
    pub(crate) recipients: Vec<PlayerRuntimeIdentity>,
    pub(crate) server_tick: u32,
    pub(crate) payload: crate::realtime_event::RealtimeEventPayload,
}

/// Damage audience frozen before mutation. Session identity prevents a reused
/// player id from receiving the fact, while permitting a lethal transaction to
/// advance the same connection's life before the old-life payload is queued.
pub(crate) struct RealtimeEventSessionDelivery {
    pub(crate) recipients: Vec<PlayerSessionIdentity>,
    pub(crate) server_tick: u32,
    pub(crate) payload: crate::realtime_event::RealtimeEventPayload,
}

#[derive(Clone, Debug)]
pub(crate) struct SnapshotHistoryEntry {
    pub(crate) snapshot_id: u16,
    /// Exact Continuum tick encoded in this snapshot. The acknowledged base
    /// proves which authoritative clock sample reached the browser, allowing
    /// input-door validation to mirror the browser's bounded extrapolation
    /// without trusting a client-authored timestamp.
    pub(crate) server_tick: u32,
    /// Immutable once emitted. Admission clones the history queue for rollback,
    /// but must not deep-copy every retained delta base on every input.
    pub(crate) entities: std::sync::Arc<SnapshotEntityMap>,
    /// Phase 2 polish (RTT): when this snapshot left the server, so
    /// the matching 0x71 ack can be timed for per-shooter RTT.
    pub(crate) emitted_at: std::time::Instant,
    /// The full prize body a client that acknowledged this frame must hold,
    /// with the id of the snapshot that carried it: this frame's own body, or
    /// the one its acknowledged base guaranteed.
    pub(crate) prize_known: Option<(u16, SnapshotPrizeState)>,
}

/// A compact-private client gets the full prize body at least this often,
/// even when only its energy changed.
pub(crate) const PRIZE_BODY_REFRESH_SNAPSHOTS: u16 = 60;

/// Extract the full near-tier entity state from a stored
/// `last_position` packet (the server's outgoing LARGE_POSITION
/// shape). Returns `None` if the packet is malformed.
pub(crate) fn extract_near_entity_state(packet: &[u8]) -> Option<NearEntityState> {
    if packet.len() < 23 {
        return None;
    }
    let (x, y) = crate::arena::large_position_coordinates(packet)?;
    Some(NearEntityState {
        x,
        y,
        velocity_x: i16::from_le_bytes(packet[10..12].try_into().ok()?),
        velocity_y: i16::from_le_bytes(packet[6..8].try_into().ok()?),
        direction: packet[1],
        energy: u16::from_le_bytes(packet[21..23].try_into().ok()?),
        weapon: u16::from_le_bytes(packet[19..21].try_into().ok()?),
        status: packet[13],
        bounty: u16::from_le_bytes(packet[17..19].try_into().ok()?),
        ping: packet[14],
        // flag_timer is optional in the LARGE_POSITION extension
        // region. Fall back to 0 if the packet doesn't carry it.
        flag_timer: if packet.len() >= 27 {
            u16::from_le_bytes(packet[25..27].try_into().ok()?)
        } else {
            0
        },
        // Filled from the authenticated ClientHandle by AOI classification.
        life_seq: 0,
        motion_segment: 0,
        relocation_generation: 0,
        // Emission installs the exact accepted sample time before encoding.
        sample_time: crate::combat_timeline::PresentationSampleTime::default(),
    })
}

fn observer_presentation_samples(
    state: &ServerState,
    recipient_id: u16,
    near: &[(u16, NearEntityState)],
) -> Vec<crate::combat_timeline::ObserverPresentationSampleV1> {
    near.iter()
        .filter(|(actor_id, _)| *actor_id != recipient_id)
        .filter_map(|(actor_id, sample)| {
            let actor = state.active_actor_runtime_identity(*actor_id)?;
            if actor.life.life_seq != sample.life_seq {
                return None;
            }
            let motion_segment = state.active_actor_motion_segment(*actor_id)?;
            Some(observer_presentation_sample(actor, motion_segment, sample))
        })
        .collect()
}

/// What the recipient is shown of one actor: the published state as the
/// combat timeline judges hits against it.
fn observer_presentation_sample(
    actor: crate::authority_identity::PlayerRuntimeIdentity,
    motion_segment: crate::combat_timeline::MotionSegmentIdentity,
    sample: &NearEntityState,
) -> crate::combat_timeline::ObserverPresentationSampleV1 {
    crate::combat_timeline::ObserverPresentationSampleV1 {
        actor,
        motion_segment,
        sample_time: sample.sample_time,
        x_q8: i32::from(sample.x) * 256,
        y_q8: i32::from(sample.y) * 256,
        velocity_x_q8: (crate::continuum_position::protocol_velocity_pixels_per_second(
            sample.velocity_x,
        ) * 256.0)
            .round() as i32,
        velocity_y_q8: (crate::continuum_position::protocol_velocity_pixels_per_second(
            sample.velocity_y,
        ) * 256.0)
            .round() as i32,
        orientation_u16: (((u32::from(sample.direction % 40) * 65_536) + 20) / 40) as u16,
        snap_discontinuity: sample.status & crate::authority::POSITION_STATUS_FLASH != 0,
    }
}

/// Bytes a held entity still costs: its id and an empty field mask.
const HELD_ENTITY_WIRE_LEN: usize = SnapshotEntity::HEADER_LEN;
/// An actor is held at most this many snapshots in a row, so every listed
/// actor still updates at least every third frame (20 Hz at 60 Hz).
pub(crate) const MAX_CONSECUTIVE_SNAPSHOT_HOLDS: u8 = 2;

/// When a snapshot would drain the recipient's send budget, picks listed
/// actors to hold at their acknowledged-base state instead of skipping whole
/// frames. A held actor stays listed with no changed fields, so the
/// client keeps showing its last state, exactly as if that actor's update had
/// been lost, and the next delta from an acknowledged frame catches it up.
///
/// Only actors whose base has the same life, motion segment and relocation
/// generation can be held: births, deaths and warps always go out. Mid-range
/// actors are held first, then nearby actors farthest from `origin` first,
/// skipping any already held [`MAX_CONSECUTIVE_SNAPSHOT_HOLDS`] frames in a
/// row. Returns the fewest actors in that order that save `excess_bytes`, or
/// every candidate when that is not possible; a frame that still exceeds the
/// remaining budget is then skipped as before.
pub(crate) fn choose_snapshot_holds(
    near: &[(u16, NearEntityState)],
    near_wire: &[SnapshotEntity],
    mid: &[(u16, u16, u16, u32)],
    mid_wire: &[SnapshotEntity],
    base: &SnapshotEntityMap,
    recipient_id: u16,
    origin: Option<(u16, u16)>,
    consecutive_holds: &std::collections::HashMap<u16, u8>,
    compact_tick: Option<u32>,
    excess_bytes: usize,
) -> Vec<u16> {
    let holdable = |id: u16| {
        id != recipient_id
            && consecutive_holds.get(&id).copied().unwrap_or(0) < MAX_CONSECUTIVE_SNAPSHOT_HOLDS
    };
    let saving = |wire: &SnapshotEntity| {
        wire.wire_len_at(compact_tick)
            .saturating_sub(HELD_ENTITY_WIRE_LEN)
    };
    let mut candidates: Vec<(u8, u64, u16, usize)> = Vec::new();
    for ((id, x, y, life_seq), wire) in mid.iter().zip(mid_wire) {
        if base.get(id).is_some_and(|held| held.life_seq == *life_seq) && holdable(*id) {
            let distance = origin.map_or(0, |origin| squared_distance(origin, (*x, *y)));
            candidates.push((0, distance, *id, saving(wire)));
        }
    }
    for ((id, current), wire) in near.iter().zip(near_wire) {
        let same_motion = base.get(id).is_some_and(|held| {
            held.life_seq == current.life_seq
                && held.motion_segment == current.motion_segment
                && held.relocation_generation == current.relocation_generation
        });
        if same_motion && holdable(*id) {
            let distance =
                origin.map_or(0, |origin| squared_distance(origin, (current.x, current.y)));
            candidates.push((1, distance, *id, saving(wire)));
        }
    }
    // Mid before near; within a tier, farthest first.
    candidates.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut saved = 0;
    let mut held = Vec::new();
    for (_, _, id, saving) in candidates {
        if saved >= excess_bytes {
            break;
        }
        if saving > 0 {
            saved += saving;
            held.push(id);
        }
    }
    held
}

/// The most bytes a frame should take from the send budget. With room for it
/// and two more ticks of refill it goes out whole; below that, frames shrink
/// towards one tick's refill so the budget stops draining, and never above
/// what is left.
pub(crate) fn snapshot_hold_target(frame_len: usize, budget: usize, refill: usize) -> usize {
    if budget >= frame_len + 2 * refill {
        frame_len
    } else {
        budget.min(refill)
    }
}

fn squared_distance(a: (u16, u16), b: (u16, u16)) -> u64 {
    let dx = u64::from(a.0.abs_diff(b.0));
    let dy = u64::from(a.1.abs_diff(b.1));
    dx * dx + dy * dy
}

// Phase 4 legacy retirement: the `ZUBSPACE_SERVER_SNAPSHOTS` flag
// has been removed. The snapshot/delta path is the only production
// emission path; the bench numbers and Chrome QA validated that
// flipping the default was the correct call (commit 18d5101). The
// `emit_snapshots_for_tick_inner(state, enabled)` shape stays so
// the `server_does_not_emit_when_flag_off` regression test can
// still drive the no-emit case directly without a process-global
// env override.

/// Mid-tier emission interval in server ticks. Clamped to at least 1 so
/// a tick rate below `SNAPSHOT_HZ` falls back to one snapshot per tick
/// rather than dividing by zero.
pub(crate) fn snapshot_interval_ticks(tick_rate_hz: u16) -> u64 {
    (tick_rate_hz / SNAPSHOT_HZ.max(1)).max(1) as u64
}

/// Near-tier emission interval in server ticks (Phase A). Near snapshots
/// target `min(tick_rate, NEAR_SNAPSHOT_HZ)`, so at 60 Hz they emit every
/// tick (interval 1) and at 120 Hz every other tick (interval 2). The
/// near interval always divides the mid interval, so every mid-bearing
/// frame is also a near frame. Clamped to at least 1.
pub(crate) fn near_snapshot_interval_ticks(tick_rate_hz: u16) -> u64 {
    let near_hz = tick_rate_hz.min(NEAR_SNAPSHOT_HZ).max(1);
    (tick_rate_hz / near_hz).max(1) as u64
}

/// Build a snapshot frame with the given metadata + entity payload
/// + pending event payload.
pub(crate) fn build_snapshot(
    arena_stream_id: u64,
    snapshot_id: u16,
    base_id: u16,
    flags: u8,
    tick: u32,
    near_entities: &[SnapshotEntity],
    mid_entities: &[SnapshotEntity],
    events: &PendingSnapshotEvents,
    rocket_state: Option<SnapshotRocketState>,
    lifecycle_state: Option<SnapshotLifecycleState>,
    prize_state: Option<SnapshotPrizeState>,
    prize_reference: Option<crate::snapshot_wire::SnapshotPrizeReference>,
    observer_timeline: Option<crate::combat_timeline::ObserverTimelineDirectiveV1>,
    input_applied: Option<SnapshotInputApplied>,
) -> Vec<u8> {
    let mut snapshot_events: Vec<_> = events
        .watch_damage
        .iter()
        .map(|envelope| {
            let event = envelope.event;
            SnapshotEvent::WatchDamage(SnapshotWatchDamage {
                victim_id: event.victim_id,
                shooter_id: event.shooter_id,
                victim_life_seq: Some(event.victim_life_seq),
                shooter_life_seq: Some(event.shooter_life_seq),
                weapon_data: event.weapon_data,
                damage: event.damage,
                energy_before: Some(event.energy_before),
                energy_after: event.energy_after,
                server_tick: Some(event.server_tick),
                damage_seq: Some(event.damage_seq),
                projectile_id: Some(event.projectile_id),
                lethal: event.lethal,
            })
        })
        .collect();
    if let Some(rocket_state) = rocket_state {
        snapshot_events.push(SnapshotEvent::RocketState(rocket_state));
    }
    if let Some(lifecycle_state) = lifecycle_state {
        snapshot_events.push(SnapshotEvent::LifecycleState(lifecycle_state));
    }
    if let Some(prize_state) = prize_state {
        snapshot_events.push(SnapshotEvent::PrizeState(prize_state));
    }
    if let Some(prize_reference) = prize_reference {
        snapshot_events.push(SnapshotEvent::PrizeReference(prize_reference));
    }
    if let Some(observer_timeline) = observer_timeline {
        snapshot_events.push(SnapshotEvent::ObserverTimeline(observer_timeline));
    }
    if let Some(input_applied) = input_applied {
        snapshot_events.push(SnapshotEvent::InputApplied(input_applied));
    }
    SnapshotFrame {
        header: SnapshotHeader {
            arena_stream_id,
            flags,
            snapshot_id,
            base_id,
            tick,
            near_count: near_entities.len() as u8,
            mid_count: mid_entities.len() as u8,
            event_count: snapshot_events.len() as u8,
        },
        near_entities: near_entities.to_vec(),
        mid_entities: mid_entities.to_vec(),
        events: snapshot_events,
    }
    .encode()
}

/// Test-only convenience: build a snapshot with no entities or
/// events. Used by cadence-only tests that just need a valid byte
/// sequence.
#[cfg(test)]
pub(crate) fn build_empty_snapshot(
    snapshot_id: u16,
    base_id: u16,
    flags: u8,
    tick: u32,
) -> Vec<u8> {
    build_snapshot(
        0,
        snapshot_id,
        base_id,
        flags,
        tick,
        &[],
        &[],
        &PendingSnapshotEvents::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Compute the per-recipient AOI entity lists. The recipient's own
/// authoritative server state is always included in the near tier when
/// they have a position, so the browser can clamp HUD energy to server
/// truth without trusting local prediction. Other joined players are
/// classified by tier relative to the recipient's `last_position`.
/// Spectators (ship >= 8) are skipped. Caps each tier at 255 entries
/// since the wire-format count fields are u8 — at 200-player density
/// the near tier is expected to hold ~16-25 entities, well under the
/// cap, but the guard prevents a malformed snapshot if a future arena
/// pushes past it.
#[cfg(test)]
pub(crate) fn compute_aoi_entities(
    state: &ServerState,
    recipient_id: u16,
) -> (Vec<(u16, NearEntityState)>, Vec<(u16, u16, u16, u32)>) {
    compute_aoi_entities_with_candidates(state, recipient_id, None)
}

fn compute_aoi_entities_with_candidates(
    state: &ServerState,
    recipient_id: u16,
    cached_candidates: Option<&std::collections::BTreeSet<u16>>,
) -> (Vec<(u16, NearEntityState)>, Vec<(u16, u16, u16, u32)>) {
    let Some(recipient) = state.clients.get(&recipient_id) else {
        return (Vec::new(), Vec::new());
    };
    if !recipient.joined || !recipient.snapshot_ready {
        return (Vec::new(), Vec::new());
    }
    let Some(recipient_pos) = recipient
        .last_position
        .as_deref()
        .or(recipient.aoi_observer_position.as_deref())
    else {
        return (Vec::new(), Vec::new());
    };

    // Phase C: widen the candidate gather by the predictive lead cap so a
    // fast ship just outside mid-AOI can still be pulled in early by the
    // predictive classification below.
    let gathered;
    let candidates = if let Some(candidates) = cached_candidates {
        candidates
    } else {
        gathered = state.visibility_index.candidate_player_ids(
            recipient_pos,
            MID_AOI_RADIUS_PX + crate::visibility::AOI_PREDICTIVE_LEAD_MAX_PX,
        );
        &gathered
    };

    let mut near = Vec::new();
    let mut mid = Vec::new();

    if recipient.alive
        && let Some(mut entity_state) = recipient
            .last_position
            .as_deref()
            .and_then(extract_near_entity_state)
    {
        // Self snapshots are for authoritative state reconciliation. The
        // local weapon visual is already driven by local input, so don't echo
        // the one-shot weapon byte back to the same browser.
        entity_state.weapon = 0;
        entity_state.life_seq = recipient.life_seq;
        near.push((recipient_id, entity_state));
    }

    for &id in candidates {
        if id == recipient_id {
            continue;
        }
        let Some(client) = state.clients.get(&id) else {
            continue;
        };
        if !client.joined || !client.snapshot_ready || !client.alive || client.player.ship >= 8 {
            continue;
        }
        let Some(other_pos) = client.last_position.as_deref() else {
            continue;
        };
        // Phase B: widen the radii for the tier this candidate is already
        // in (sticky demotion) so a boundary-hovering ship doesn't flicker
        // tiers. Promotion uses the plain radius, so entering the screen
        // stays instant.
        let (near_radius, mid_radius) = match recipient.aoi_tier_memory.get(&id).copied() {
            Some(AoiTier::Near) => (
                NEAR_AOI_RADIUS_PX + crate::visibility::AOI_HYSTERESIS_MARGIN_PX,
                MID_AOI_RADIUS_PX + crate::visibility::AOI_HYSTERESIS_MARGIN_PX,
            ),
            Some(AoiTier::Mid) => (
                NEAR_AOI_RADIUS_PX,
                MID_AOI_RADIUS_PX + crate::visibility::AOI_HYSTERESIS_MARGIN_PX,
            ),
            _ => (NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
        };
        // Phase C: predictive classification pulls a closing candidate
        // into a higher tier early (no pop-in). `recipient_pos` is the
        // viewer; `other_pos` is the candidate whose velocity is projected.
        let tier = crate::visibility::aoi_tier_between_positions_predictive(
            recipient_pos,
            other_pos,
            near_radius,
            mid_radius,
            crate::visibility::AOI_PREDICTIVE_LEAD_SECONDS,
        );
        match tier {
            AoiTier::Near if near.len() < u8::MAX as usize => {
                if let Some(mut entity_state) = extract_near_entity_state(other_pos) {
                    entity_state.life_seq = client.life_seq;
                    // Weapon is a one-shot snapshot field, never persistent
                    // position state. An accepted weapon remains in the
                    // source's last LARGE_POSITION bytes until a later
                    // position commits; copying that stale byte into a FULL
                    // fallback would replay Repel (and every other weapon)
                    // after queue coalescing or input loss.
                    entity_state.weapon = if client.arena_stream_id == 0 {
                        client.pending_snapshot_weapon
                    } else {
                        0
                    };
                    near.push((id, entity_state));
                }
            }
            AoiTier::Mid if mid.len() < u8::MAX as usize => {
                if let Some((x, y)) = crate::arena::large_position_coordinates(other_pos) {
                    mid.push((id, x, y, client.life_seq));
                }
            }
            _ => {}
        }
    }
    for bot in state.bots.iter().filter(|bot| bot.alive()) {
        let id = bot.player.id;
        let bot_position = bot.large_position_packet();
        let (near_radius, mid_radius) = match recipient.aoi_tier_memory.get(&id).copied() {
            Some(AoiTier::Near) => (
                NEAR_AOI_RADIUS_PX + crate::visibility::AOI_HYSTERESIS_MARGIN_PX,
                MID_AOI_RADIUS_PX + crate::visibility::AOI_HYSTERESIS_MARGIN_PX,
            ),
            Some(AoiTier::Mid) => (
                NEAR_AOI_RADIUS_PX,
                MID_AOI_RADIUS_PX + crate::visibility::AOI_HYSTERESIS_MARGIN_PX,
            ),
            _ => (NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
        };
        match crate::visibility::aoi_tier_between_positions_predictive(
            recipient_pos,
            &bot_position,
            near_radius,
            mid_radius,
            crate::visibility::AOI_PREDICTIVE_LEAD_SECONDS,
        ) {
            AoiTier::Near if near.len() < u8::MAX as usize => {
                if let Some(mut entity_state) = extract_near_entity_state(&bot_position) {
                    entity_state.life_seq = bot.life_seq;
                    entity_state.weapon = 0;
                    near.push((id, entity_state));
                }
            }
            AoiTier::Mid if mid.len() < u8::MAX as usize => {
                mid.push((
                    id,
                    bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                    bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                    bot.life_seq,
                ));
            }
            _ => {}
        }
    }
    // The near-snapshot carry countdown. `extract_near_entity_state` above always yields 0 for this
    // field in production: the bytes ride in the client's own `LARGE_POSITION` extension, and the
    // server's canonical position form (`large_position_from_client_snapshot`, 23 bytes) deliberately
    // does not carry client-provided time or prize state, so nothing reached it. The objective registry
    // is the only party that knows a carry deadline, so this is the producer: remaining ticks for a
    // carrier in a zone that authors a carry expiry, 0 for everyone else, including bots.
    let flag_timers = crate::objectives::authoritative_near_flag_timers(state);
    for (actor_id, entity_state) in &mut near {
        entity_state.flag_timer = flag_timers.get(actor_id).copied().unwrap_or(0);
    }
    (near, mid)
}

/// Phase 3e-5: queue a `WatchDamage` event onto every joined client
/// whose near-AOI of `anchor_player_id` (the victim) contains the
/// anchor, plus any explicit `forced_recipients` (typically the
/// shooter). The pending event is drained into the next snapshot's
/// `events[]` by `emit_snapshots_for_tick_inner`. This is the only
/// path for both generation-bound `WatchDamage` wire forms — the legacy 3c
/// reliable broadcast was retired with the rest of the per-pair fan-out.
#[cfg(test)]
pub(crate) fn queue_watch_damage_event_to_near_aoi_inner(
    state: &Arc<Mutex<ServerState>>,
    anchor_player_id: u16,
    forced_recipients: &[u16],
    event: PendingWatchDamage,
    snapshots_enabled: bool,
) {
    if !snapshots_enabled {
        return;
    }
    let mut state = state.lock().expect("server state should not be poisoned");
    let recipients = watch_damage_recipient_identities(&state, anchor_player_id, forced_recipients);
    queue_watch_damage_event_to_recipients_inner(&mut state, &recipients, event);
}

pub(crate) fn watch_damage_recipient_identities(
    state: &ServerState,
    anchor_player_id: u16,
    forced_recipients: &[u16],
) -> Vec<PlayerRuntimeIdentity> {
    watch_damage_recipient_identities_for_position(state, anchor_player_id, forced_recipients, None)
}

pub(crate) fn watch_damage_recipient_identities_for_position(
    state: &ServerState,
    anchor_player_id: u16,
    forced_recipients: &[u16],
    anchor_position_override: Option<&[u8]>,
) -> Vec<PlayerRuntimeIdentity> {
    let stored_anchor_position = state
        .clients
        .get(&anchor_player_id)
        .and_then(|client| client.last_position.as_deref());
    let anchor_position = anchor_position_override.or(stored_anchor_position);
    {
        state
            .clients
            .iter()
            .filter_map(|(&id, client)| {
                if !client.joined {
                    return None;
                }
                if forced_recipients.contains(&id) || id == anchor_player_id {
                    return state.active_player_runtime_identity(id);
                }
                let Some(anchor_pos) = anchor_position else {
                    // Anchor has no position yet — fall back to delivering
                    // to every joined client, matching the legacy 3c path.
                    return state.active_player_runtime_identity(id);
                };
                let tier = match client.last_position.as_deref() {
                    Some(rp) => crate::visibility::aoi_tier_between_positions(
                        anchor_pos,
                        rp,
                        crate::visibility::NEAR_AOI_RADIUS_PX,
                        crate::visibility::MID_AOI_RADIUS_PX,
                    ),
                    // Unpositioned joined clients are treated as near.
                    None => crate::visibility::AoiTier::Near,
                };
                tier.is_near()
                    .then(|| state.active_player_runtime_identity(id))
                    .flatten()
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) fn queue_watch_damage_event_to_recipients(
    state: &Arc<Mutex<ServerState>>,
    recipients: &[PlayerRuntimeIdentity],
    event: PendingWatchDamage,
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    queue_watch_damage_event_to_recipients_inner(&mut state, recipients, event);
}

#[cfg(test)]
fn queue_watch_damage_event_to_recipients_inner(
    state: &mut ServerState,
    recipients: &[PlayerRuntimeIdentity],
    event: PendingWatchDamage,
) {
    let payload = watch_damage_realtime_payload(&state, event);
    let reservation = payload.and_then(|_| {
        reserve_realtime_event_deliveries(
            state,
            &[recipients],
            RealtimeEventPressureDisposition::Rejected,
        )
    });
    queue_watch_damage_snapshot_event_to_recipients_inner(state, recipients, event);
    if let (Some(payload), Some(reservation)) = (payload, reservation) {
        reservation
            .commit(
                state,
                vec![RealtimeEventDelivery {
                    recipients: recipients.to_vec(),
                    server_tick: event.server_tick,
                    payload,
                }],
            )
            .expect("exclusive realtime reservation must remain valid");
    }
}

pub(crate) fn queue_watch_damage_snapshot_event_to_recipients(
    state: &Arc<Mutex<ServerState>>,
    recipients: &[PlayerRuntimeIdentity],
    event: PendingWatchDamage,
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    queue_watch_damage_snapshot_event_to_recipients_inner(&mut state, recipients, event);
}

fn queue_watch_damage_snapshot_event_to_recipients_inner(
    state: &mut ServerState,
    recipients: &[PlayerRuntimeIdentity],
    event: PendingWatchDamage,
) {
    for &recipient in recipients {
        if state.player_runtime_matches(recipient) {
            let client = state
                .clients
                .get_mut(&recipient.player_id())
                .expect("matching recipient should remain registered");
            // Bound the queue so a runaway can't OOM the server. The
            // wire `event_count` is u8 anyway; `emit_snapshots_for_tick`
            // truncates at u8::MAX. Drop the oldest if we're already at
            // the cap (prefer keeping fresh combat over stale).
            if client.pending_snapshot_events.watch_damage.len() >= u8::MAX as usize {
                client.pending_snapshot_events.watch_damage.pop_front();
            }
            client
                .pending_snapshot_events
                .watch_damage
                .push_back(PendingWatchDamageEnvelope { recipient, event });
        }
    }
}

pub(crate) fn watch_damage_realtime_payload(
    state: &ServerState,
    event: PendingWatchDamage,
) -> Option<crate::realtime_event::RealtimeEventPayload> {
    let victim = state
        .clients
        .get(&event.victim_id)
        .map(|client| client.session_id)
        .or_else(|| state.bot(event.victim_id).map(|bot| bot.session_id))?;
    let shooter = state
        .clients
        .get(&event.shooter_id)
        .map(|client| client.session_id)
        .or_else(|| state.bot(event.shooter_id).map(|bot| bot.session_id))?;
    Some(crate::realtime_event::RealtimeEventPayload::WatchDamage(
        crate::realtime_event::RealtimeWatchDamageEvent {
            victim_player_id: event.victim_id,
            victim_session_id: victim.get(),
            victim_life_seq: event.victim_life_seq,
            shooter_player_id: event.shooter_id,
            shooter_session_id: shooter.get(),
            shooter_life_seq: event.shooter_life_seq,
            weapon_data: event.weapon_data,
            damage: event.damage,
            energy_before: event.energy_before,
            energy_after: event.energy_after,
            projectile_id: event.projectile_id,
            damage_seq: event.damage_seq,
            lethal: event.lethal,
        },
    ))
}

pub(crate) fn reserve_realtime_event_deliveries(
    state: &mut ServerState,
    recipient_groups: &[&[PlayerRuntimeIdentity]],
    disposition: RealtimeEventPressureDisposition,
) -> Option<RealtimeEventReservation> {
    let mut slots_by_player = std::collections::BTreeMap::<u16, usize>::new();
    for recipients in recipient_groups {
        for &recipient in *recipients {
            if state.player_runtime_matches(recipient) {
                let slots = slots_by_player.entry(recipient.player_id()).or_default();
                *slots = slots.saturating_add(1);
            }
        }
    }
    reserve_realtime_event_slots(state, slots_by_player, disposition)
}

pub(crate) fn reserve_realtime_event_session_deliveries(
    state: &mut ServerState,
    recipient_groups: &[&[PlayerSessionIdentity]],
    disposition: RealtimeEventPressureDisposition,
) -> Option<RealtimeEventReservation> {
    let mut slots_by_player = std::collections::BTreeMap::<u16, usize>::new();
    for recipients in recipient_groups {
        for &recipient in *recipients {
            if state.player_session_matches(recipient) && state.clients[&recipient.player_id].joined
            {
                let slots = slots_by_player.entry(recipient.player_id).or_default();
                *slots = slots.saturating_add(1);
            }
        }
    }
    reserve_realtime_event_slots(state, slots_by_player, disposition)
}

pub(crate) fn reserve_realtime_event_slots(
    state: &mut ServerState,
    slots_by_player: std::collections::BTreeMap<u16, usize>,
    disposition: RealtimeEventPressureDisposition,
) -> Option<RealtimeEventReservation> {
    let has_capacity = slots_by_player.iter().all(|(&player_id, &slots)| {
        state.clients.get(&player_id).is_some_and(|client| {
            client
                .pending_realtime_events
                .queue
                .len()
                .checked_add(slots)
                .is_some_and(|total| total <= DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY)
        })
    });
    if !has_capacity {
        state.realtime_event_queue_overflows =
            state.realtime_event_queue_overflows.saturating_add(1);
        match disposition {
            RealtimeEventPressureDisposition::Rejected => {
                state.realtime_event_actions_rejected =
                    state.realtime_event_actions_rejected.saturating_add(1);
            }
            RealtimeEventPressureDisposition::Deferred => {
                state.realtime_event_actions_deferred =
                    state.realtime_event_actions_deferred.saturating_add(1);
            }
        }
        return None;
    }
    Some(RealtimeEventReservation { slots_by_player })
}

impl RealtimeEventReservation {
    pub(crate) fn commit(
        self,
        state: &mut ServerState,
        deliveries: Vec<RealtimeEventDelivery>,
    ) -> Result<Vec<u32>, &'static str> {
        let mut actual_slots = std::collections::BTreeMap::<u16, usize>::new();
        for delivery in &deliveries {
            for &recipient in &delivery.recipients {
                if state.player_runtime_matches(recipient) {
                    let slots = actual_slots.entry(recipient.player_id()).or_default();
                    *slots = slots.checked_add(1).ok_or("realtime slot count overflow")?;
                }
            }
        }
        for (&player_id, &slots) in &actual_slots {
            if slots > self.slots_by_player.get(&player_id).copied().unwrap_or(0) {
                return Err("realtime delivery exceeded its reservation");
            }
            let client = state
                .clients
                .get(&player_id)
                .ok_or("reserved realtime recipient disappeared")?;
            if client
                .pending_realtime_events
                .queue
                .len()
                .saturating_add(slots)
                > DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
            {
                return Err("reserved realtime capacity changed before commit");
            }
        }

        let mut event_ids = Vec::new();
        for delivery in deliveries {
            let matching = delivery
                .recipients
                .into_iter()
                .filter(|recipient| state.player_runtime_matches(*recipient))
                .collect::<Vec<_>>();
            if matching.is_empty() {
                continue;
            }
            for recipient in matching {
                let client = state
                    .clients
                    .get_mut(&recipient.player_id())
                    .expect("matching realtime recipient should remain registered");
                let event_id = client.allocate_realtime_event_id();
                if let Some(identity) = projectile_presentation_identity(delivery.payload)
                    && !client.threat_presentation.observe_projectile_queued(
                        identity,
                        event_id,
                        delivery.server_tick,
                    )
                {
                    return Err("projectile presentation ledger capacity exhausted");
                }
                client
                    .pending_realtime_events
                    .queue
                    .push_back(PendingRealtimeEventEnvelope {
                        flight_transition: false,
                        recipient,
                        event_id,
                        server_tick: delivery.server_tick,
                        payload: delivery.payload,
                    });
                event_ids.push(event_id);
            }
        }
        let current = state.realtime_event_queue_current();
        state.realtime_event_queue_peak = state.realtime_event_queue_peak.max(current);
        Ok(event_ids)
    }

    pub(crate) fn commit_sessions(
        self,
        state: &mut ServerState,
        deliveries: Vec<RealtimeEventSessionDelivery>,
    ) -> Vec<u32> {
        let mut actual_slots = std::collections::BTreeMap::<u16, usize>::new();
        for delivery in &deliveries {
            for &recipient in &delivery.recipients {
                if state.player_session_matches(recipient)
                    && state.clients[&recipient.player_id].joined
                {
                    let slots = actual_slots.entry(recipient.player_id).or_default();
                    *slots = slots.saturating_add(1);
                }
            }
        }
        for (&player_id, &slots) in &actual_slots {
            debug_assert!(
                slots <= self.slots_by_player.get(&player_id).copied().unwrap_or(0),
                "frozen session delivery must be an exact subset of its reservation"
            );
            debug_assert!(
                state.clients.get(&player_id).is_some_and(|client| {
                    client
                        .pending_realtime_events
                        .queue
                        .len()
                        .saturating_add(slots)
                        <= DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
                }),
                "exclusive frozen session reservation must retain capacity"
            );
        }

        let mut event_ids = Vec::new();
        for delivery in deliveries {
            for session in delivery.recipients {
                if !state.player_session_matches(session)
                    || !state.clients[&session.player_id].joined
                {
                    continue;
                }
                let Some(recipient) = state.active_player_runtime_identity(session.player_id)
                else {
                    continue;
                };
                let Some(client) = state.clients.get_mut(&session.player_id) else {
                    continue;
                };
                let event_id = client.allocate_realtime_event_id();
                if let Some(identity) = projectile_presentation_identity(delivery.payload) {
                    let recorded = client.threat_presentation.observe_projectile_queued(
                        identity,
                        event_id,
                        delivery.server_tick,
                    );
                    assert!(
                        recorded,
                        "session projectile delivery must reserve presentation capacity before committing"
                    );
                }
                client
                    .pending_realtime_events
                    .queue
                    .push_back(PendingRealtimeEventEnvelope {
                        flight_transition: false,
                        recipient,
                        event_id,
                        server_tick: delivery.server_tick,
                        payload: delivery.payload,
                    });
                event_ids.push(event_id);
            }
        }
        let current = state.realtime_event_queue_current();
        state.realtime_event_queue_peak = state.realtime_event_queue_peak.max(current);
        event_ids
    }
}

/// Emit one snapshot per joined client for the authoritative gameplay tick.
pub(crate) fn emit_snapshots_for_tick_at_tick(state: &Arc<Mutex<ServerState>>, server_tick: u32) {
    emit_snapshots_for_tick_inner_at_tick(state, true, server_tick);
}

/// Native load attribution only. Thread-local fixed storage prevents parallel
/// tests from mixing observations; none of this exists in the shipping server.
#[cfg(test)]
pub(crate) mod publication_timing {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    thread_local! {
        static TOTALS: Cell<[Duration; 3]> = const { Cell::new([Duration::ZERO; 3]) };
    }

    pub(crate) fn take() -> [Duration; 3] {
        TOTALS.with(|totals| totals.replace([Duration::ZERO; 3]))
    }

    pub(super) struct Turn {
        phase: usize,
        started: Instant,
    }

    impl Turn {
        pub(super) fn new() -> Self {
            Self {
                phase: 0,
                started: Instant::now(),
            }
        }

        pub(super) fn advance(&mut self) {
            self.record();
            self.phase += 1;
            assert!(self.phase < 3);
            self.started = Instant::now();
        }

        fn record(&self) {
            let elapsed = self.started.elapsed();
            TOTALS.with(|totals| {
                let mut values = totals.get();
                values[self.phase] += elapsed;
                totals.set(values);
            });
        }
    }

    impl Drop for Turn {
        fn drop(&mut self) {
            self.record();
        }
    }
}

#[cfg(test)]
pub(crate) fn emit_snapshots_for_tick_inner(
    state: &Arc<Mutex<ServerState>>,
    snapshots_enabled: bool,
) {
    emit_snapshots_for_tick_inner_at_tick(state, snapshots_enabled, crate::wire::current_tick());
}

fn emit_snapshots_for_tick_inner_at_tick(
    state: &Arc<Mutex<ServerState>>,
    snapshots_enabled: bool,
    tick: u32,
) {
    if !snapshots_enabled {
        return;
    }
    let mut state = state.lock().expect("server state should not be poisoned");
    // Phase A: near-tier entities emit every near interval (up to the sim
    // tick), while mid-tier entities only ride the slower mid interval.
    // We emit a frame on every near tick; `include_mid` decides whether
    // this frame also carries the peripheral mid tier.
    let near_interval = near_snapshot_interval_ticks(state.config.tick_rate_hz);
    let mid_interval = snapshot_interval_ticks(state.config.tick_rate_hz);
    let tick_rate_hz = state.config.tick_rate_hz.max(1);
    if state.arena_tick % near_interval != 0 {
        return;
    }
    let include_mid = state.arena_tick % mid_interval == 0;
    // Phase 4 load-shedding: read the pressure ratio once per emission cycle
    // so every recipient is treated consistently. Whole-frame shedding is a
    // last resort and additionally requires consecutive actual tick-budget
    // overruns; one transient scheduler spike must not suppress later healthy
    // clean-LAN snapshots while its EWMA decays.
    let load_pressure = state.load_pressure_ratio();
    let shed_mid = load_pressure >= LOAD_SHED_MID_THRESHOLD;
    let shed_frame =
        load_pressure >= LOAD_SHED_FRAME_THRESHOLD && state.sustained_frame_load_pressure();
    let near_emission_index = state.arena_tick / near_interval;
    let refresh_shed_mid =
        shed_mid && near_emission_index.is_multiple_of(LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS);
    let trace_bot_snapshots = state.config.sim_observe;
    let recipient_ids: Vec<u16> = state
        .clients
        .iter()
        .filter_map(|(&id, client)| (client.joined && client.snapshot_ready).then_some(id))
        .collect();
    let frozen_projectile_ids = state.active_projectiles.frozen_projectile_ids();
    let active_projectile_ids: std::collections::HashSet<u32> = state
        .active_projectiles
        .iter()
        .map(|p| p.id)
        .chain(state.active_decoys.iter().map(|d| d.presentation_id))
        .collect();
    let mut active_presentation_identities = std::collections::BTreeSet::new();
    for projectile in state.active_projectiles.iter() {
        let Some(source) = projectile.presentation_source else {
            continue;
        };
        let runtime = projectile_source_runtime(source);
        if let Some(group) = crate::threat_presentation::ProjectilePresentationIdentity::shot_group(
            runtime,
            projectile.shot_group_id,
        ) {
            active_presentation_identities.insert(group);
        }
        if let Some(exact) = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
            runtime,
            projectile.shot_group_id,
            projectile.id,
        ) {
            active_presentation_identities.insert(exact);
        }
    }
    for client in state.clients.values_mut() {
        client
            .projectile_motion_frozen_published
            .retain(|id| active_projectile_ids.contains(id));
        client
            .projectile_refresh_pending
            .retain(|id| active_projectile_ids.contains(id));
        client
            .threat_presentation
            .retain_projectiles(|identity| active_presentation_identities.contains(&identity));
    }
    let mut active_projectile_queue_peak = 0_usize;
    // Actor histories are immutable throughout this locked emission cycle.
    // Resolve a due state/time pair once per actor, including unavailable
    // samples. This cache never crosses a tick or replaces self authority.
    let mut publication_samples = std::collections::HashMap::new();
    // The index cannot change while this emission owns ServerState. Candidate
    // gathering depends only on the viewer's cell and this fixed radius, not
    // its exact position, life, or per-recipient tier memory. Keep the sorted
    // candidate set local to this one emission; classification is never cached.
    let mut candidate_sets = std::collections::HashMap::new();
    let mut emission = crate::observability::SnapshotEmission::default();
    for recipient_id in recipient_ids {
        // Expired heads that were never applied are cancelled before they can
        // be offered. An admitted head stays until its exact reliable 0x7a is
        // acknowledged, but can no longer affect movement a second time.
        if let Some(client) = state.clients.get_mut(&recipient_id) {
            while client
                .pending_repel_observations
                .front()
                .is_some_and(|event| {
                    event.awaiting_application()
                        && crate::continuum::continuum_tick_reached(tick, event.expires_at_tick)
                })
            {
                client.pending_repel_observations.pop_front();
            }
            crate::reliable::release_deferred_warp_prize_acks_if_ready(client);
        }
        if shed_frame {
            // Whole snapshot skipped for this recipient — the next
            // tick's full-state fallback (delta base will be missing
            // from history) catches them up.
            state.load_shed_frame_skipped = state.load_shed_frame_skipped.saturating_add(1);
            continue;
        }
        #[cfg(test)]
        let mut publication_phase = publication_timing::Turn::new();
        let candidates = state.clients.get(&recipient_id).and_then(|recipient| {
            let position = recipient
                .last_position
                .as_deref()
                .or(recipient.aoi_observer_position.as_deref())?;
            let cell = crate::visibility::visibility_cell_for_large_position(position);
            Some(&*candidate_sets.entry(cell).or_insert_with(|| {
                state.visibility_index.candidate_player_ids(
                    position,
                    MID_AOI_RADIUS_PX + crate::visibility::AOI_PREDICTIVE_LEAD_MAX_PX,
                )
            }))
        });
        let (mut near_raw, mut mid_raw) =
            compute_aoi_entities_with_candidates(&state, recipient_id, candidates);
        // Phase B: record this frame's classified tiers for next frame's
        // hysteresis — done before mid is suppressed from the wire below,
        // so near-only frames don't forget a mid entity's tier. Both tiers
        // are always classified by `compute_aoi_entities`; `include_mid`
        // only gates wire inclusion.
        if let Some(client) = state.clients.get_mut(&recipient_id) {
            client.aoi_tier_memory.clear();
            for (id, _) in &near_raw {
                if *id != recipient_id {
                    client.aoi_tier_memory.insert(*id, AoiTier::Near);
                }
            }
            for &(id, _, _, _) in &mid_raw {
                client.aoi_tier_memory.insert(id, AoiTier::Mid);
            }
        }
        if shed_mid && !refresh_shed_mid {
            // Mid-tier is peripheral awareness (normally 30 Hz);
            // dropping it frees encoder + bandwidth budget while
            // preserving near-AOI fidelity. Count the skipped
            // entities (not the recipient) so the summary tracks
            // actual entity-level shed pressure.
            state.load_shed_mid_skipped = state
                .load_shed_mid_skipped
                .saturating_add(mid_raw.len() as u64);
            mid_raw.clear();
        } else if !include_mid && !refresh_shed_mid {
            // Phase A near-only frame: the peripheral mid tier rides the
            // slower 30 Hz cadence and is simply absent here (not shed
            // under pressure, so not counted). The client holds mid
            // proxies across these frames via the mid-tier lifetime.
            mid_raw.clear();
        }
        near_raw.retain_mut(|(id, entity)| {
            if *id == recipient_id {
                // Self reconciliation is current authority, not delayed remote
                // presentation, and must not change the local prediction path.
                // Self has no remote source sample; its zero value is unused
                // by directed presentation and stable across unchanged frames.
                return true;
            }
            let sample = publication_samples.entry(*id).or_insert_with(|| {
                state
                    .clients
                    .get(id)
                    .map(|client| &client.actor_motion_history)
                    .or_else(|| {
                        state
                            .bots
                            .iter()
                            .find(|bot| bot.player.id == *id)
                            .map(|bot| &bot.actor_motion_history)
                    })
                    .and_then(|history| {
                        history
                            .publication_sample_at(tick)
                            .ok()
                            .map(|sample| (sample, history.relocation_generation()))
                    })
            });
            let Some((sample, relocation_generation)) = sample else {
                return false;
            };
            if sample.segment.actor.life.life_seq != entity.life_seq {
                return false;
            }
            entity.x = (sample.x_q8 as f32 / 256.0)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16;
            entity.y = (sample.y_q8 as f32 / 256.0)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16;
            entity.velocity_x = (sample.velocity_x_q8 as f32 / 256.0 * 10.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            entity.velocity_y = (sample.velocity_y_q8 as f32 / 256.0 * 10.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            entity.direction = sample.protocol_direction;
            entity.sample_time = sample.sample_time;
            entity.motion_segment = sample.segment.discontinuity_generation;
            entity.relocation_generation = *relocation_generation;
            true
        });
        let trace_bot_near_ids = trace_bot_snapshots.then(|| {
            near_raw
                .iter()
                .filter_map(|(id, _)| (*id >= crate::state::SERVER_BOT_ID_BASE).then_some(*id))
                .collect::<Vec<_>>()
        });
        let trace_bot_mid_ids = trace_bot_snapshots.then(|| {
            mid_raw
                .iter()
                .filter_map(|(id, _, _, _)| {
                    (*id >= crate::state::SERVER_BOT_ID_BASE).then_some(*id)
                })
                .collect::<Vec<_>>()
        });
        let mut emitted_actor_runtimes = near_raw
            .iter()
            .map(|(id, _)| *id)
            .chain(mid_raw.iter().map(|(id, _, _, _)| *id))
            .filter_map(|id| state.active_actor_runtime_identity(id))
            .collect::<Vec<_>>();
        emitted_actor_runtimes.sort_unstable();
        emitted_actor_runtimes.dedup();
        let Some(current_recipient) = state.active_player_runtime_identity(recipient_id) else {
            continue;
        };
        let mut presentation_actors =
            observer_presentation_samples(&state, recipient_id, &near_raw);
        let active_projectile_states = projectile_recovery_center(&state, recipient_id, &near_raw)
            .map(|(x, y)| {
                active_projectile_state_for_recipient_with_frozen_ids(
                    &state,
                    recipient_id,
                    x,
                    y,
                    &frozen_projectile_ids,
                )
            })
            .unwrap_or_default();
        let Some(client) = state.clients.get_mut(&recipient_id) else {
            continue;
        };
        for (identity, projectile_state) in active_projectile_states {
            if client.pending_realtime_events.queue.len() >= DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
            {
                break;
            }
            let event_id = client.allocate_realtime_event_id();
            if !client
                .threat_presentation
                .observe_projectile_queued(identity, event_id, tick)
            {
                break;
            }
            let flight_transition = projectile_state.motion_frozen
                || client
                    .projectile_motion_frozen_published
                    .contains(&projectile_state.projectile_id);
            client
                .projectile_refresh_pending
                .remove(&projectile_state.projectile_id);
            if projectile_state.motion_frozen {
                client
                    .projectile_motion_frozen_published
                    .insert(projectile_state.projectile_id);
            } else {
                client
                    .projectile_motion_frozen_published
                    .remove(&projectile_state.projectile_id);
            }
            client
                .pending_realtime_events
                .queue
                .push_back(PendingRealtimeEventEnvelope {
                    flight_transition,
                    recipient: current_recipient,
                    event_id,
                    server_tick: tick,
                    payload: crate::realtime_event::RealtimeEventPayload::ProjectileState(
                        projectile_state,
                    ),
                });
        }
        active_projectile_queue_peak =
            active_projectile_queue_peak.max(client.pending_realtime_events.queue.len());

        // Pick a delta base only when the acknowledged state is still in
        // history. FULL is an explicit wire flag; base id zero remains only
        // as a defense-in-depth value on full frames.
        #[cfg(test)]
        publication_phase.advance();
        let base = client
            .last_acked_snapshot_id
            .and_then(|id| find_in_history(&client.snapshot_history, id));
        let base_id = base.map(|entry| entry.snapshot_id).unwrap_or(0);
        let mut flags = if base.is_none() {
            SNAPSHOT_FLAG_FULL
        } else {
            0
        };
        if client.protocol_revision >= crate::snapshot_wire::CLIENT_PROTOCOL_COMPACT_SAMPLE_TIME {
            flags |= crate::snapshot_wire::SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME;
        }
        let compact_private =
            client.protocol_revision >= crate::snapshot_wire::CLIENT_PROTOCOL_COMPACT_PRIVATE;
        if compact_private {
            flags |= crate::snapshot_wire::SNAPSHOT_FLAG_COMPACT_PRIVATE;
        }
        let base_prize_known = base.and_then(|entry| entry.prize_known);
        let compact_tick = (flags & crate::snapshot_wire::SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME != 0)
            .then_some(tick & crate::continuum::CONTINUUM_TICK_MASK);
        let base_entities = base.map(|entry| std::sync::Arc::clone(&entry.entities));

        // The private Repel application is a self-contained lifecycle record,
        // not a synthetic mutable near-entity. Public entity deltas therefore
        // remain ordinary presentation and can never apply target authority.
        let position_offsets =
            client.protocol_revision >= crate::snapshot_wire::CLIENT_PROTOCOL_POSITION_OFFSETS;
        // Deltas against the acknowledged base, with one-byte position
        // offsets where the client accepts them and they fit.
        let encode_entities = |near: &[(u16, NearEntityState)],
                               mid: &[(u16, u16, u16, u32)],
                               base: Option<&SnapshotEntityMap>| {
            let mut near_entities = delta_encode_near_tier(near, base);
            let mut mid_entities = delta_encode_mid_tier(mid, base);
            if let Some(base) = base.filter(|_| position_offsets) {
                crate::snapshot_wire::offset_positions(&mut near_entities, base);
                crate::snapshot_wire::offset_positions(&mut mid_entities, base);
            }
            (near_entities, mid_entities)
        };
        let (mut near_entities, mut mid_entities) =
            encode_entities(&near_raw, &mid_raw, base_entities.as_deref());

        // Phase 3e-5: drain the recipient's pending event queue.
        // Events are accumulated by the runtime as gameplay happens
        // (e.g. server_damage queues WATCH_DAMAGE on near-AOI peers).
        // Cap at u8::MAX since the wire `event_count` is 1 byte; in
        // practice an arena should never produce >255 events for one
        // recipient in a frame, but the cap keeps the wire well-formed if it
        // ever does.
        let mut events = std::mem::take(&mut client.pending_snapshot_events);
        let mut realtime_events = std::mem::take(&mut client.pending_realtime_events);
        events.retain_for_recipient(current_recipient);
        realtime_events.retain_for_recipient(current_recipient);
        if events.len() >= u8::MAX as usize - 4 {
            // Reserve five event slots for repeated private Rocket,
            // lifecycle, prize/loadout, observer-timeline and applied-input
            // authority.
            events.watch_damage.truncate(u8::MAX as usize - 5);
        }

        let rocket_authorization = client.server_rocket_authorizations.back().copied();
        // This is a receipt for a Rocket activation pulse, not merely a newest-
        // input watermark. A later false record can overtake the activation
        // record on the unordered game channel; echoing that false record's
        // sequence would make the client cancel a prediction before the
        // bounded history has had a chance to repair the missing edge.
        let observed_rocket_assertion_sequence = client.last_input_seq.and_then(|newest| {
            client
                .recent_input_rocket_assertions
                .iter()
                .filter(|(_, asserted)| *asserted)
                .min_by_key(|(sequence, _)| newest.wrapping_sub(*sequence))
                .map(|(sequence, _)| *sequence)
        });
        let rocket_state = SnapshotRocketState {
            life_seq: client.life_seq,
            observed_input_sequence: observed_rocket_assertion_sequence,
            input_sequence_watermark: client.last_input_seq,
            activation_sequence: rocket_authorization
                .map(|authorization| authorization.start_sequence),
            until_tick: rocket_authorization.map(|authorization| authorization.deadline_tick),
            active: rocket_authorization.zip(client.last_input_seq).is_some_and(
                |(authorization, newest)| {
                    authorization.contains_sequence(newest)
                        && client.server_rocket_until_tick.is_some_and(|until_tick| {
                            !crate::continuum::continuum_tick_reached(tick, until_tick)
                        })
                },
            ),
            rockets: client.server_weapons.rockets,
        };
        let delivers_pending_repel = client
            .pending_repel_observations
            .front()
            .filter(|observation| {
                observation.awaiting_application()
                    && !crate::continuum::continuum_tick_reached(tick, observation.expires_at_tick)
            })
            .is_some();
        let repel_active_steps = delivers_pending_repel.then(|| {
            let event = client
                .pending_repel_observations
                .front()
                .expect("pending Repel delivery");
            let remaining_ticks =
                crate::continuum::continuum_tick_diff(event.expires_at_tick, tick).max(0) as u32;
            let window = crate::gameplay_rules::repel_sequence_window(0, remaining_ticks);
            window.1.wrapping_sub(window.0).saturating_sub(1)
        });
        let lifecycle_state = SnapshotLifecycleState {
            life_seq: client.life_seq,
            alive: client.alive,
            respawn_at_tick: client.respawn_at_tick,
            repel_until_tick: client
                .server_repel_until_tick
                // Pending frames expose the original server-owned deadline,
                // but the browser arms RepelTime only from the exact frame
                // that also carries the one-shot weapon. Repetition therefore
                // cannot start or reset the sequence lifetime after loss.
                .or_else(|| {
                    client
                        .pending_repel_observations
                        .front()
                        .map(|event| event.expires_at_tick)
                })
                .filter(|until_tick| {
                    client.alive && !crate::continuum::continuum_tick_reached(tick, *until_tick)
                }),
            repel_delivery: delivers_pending_repel.then(|| {
                let event = client
                    .pending_repel_observations
                    .front()
                    .expect("pending Repel delivery");
                SnapshotRepelDelivery {
                    active_steps: repel_active_steps.expect("nonzero pending Repel steps"),
                    delivery_generation: event.delivery_generation,
                    source_player_id: event.source.player_id(),
                    source_life_seq: event.source.life.life_seq,
                    origin_x: event.origin_x as u16,
                    origin_y: event.origin_y as u16,
                    source_frequency: event.source_frequency,
                    expected_velocity_x_bits: event.expected_velocity_x.to_bits(),
                    expected_velocity_y_bits: event.expected_velocity_y.to_bits(),
                    weapon_data: event.weapon_data,
                    source_ship: event.source_ship,
                }
            }),
        };
        let mut prize_flags = u16::from(client.server_weapons.bouncing_bullets)
            | (u16::from(client.server_weapons.multifire_capable) << 1)
            | (u16::from(client.server_weapons.proximity_bombs) << 2)
            | (u16::from(client.server_status.stealth_capable) << 3)
            | (u16::from(client.server_status.cloak_capable) << 4)
            | (u16::from(client.server_status.xradar_capable) << 5)
            | (u16::from(client.server_status.antiwarp_capable) << 6);
        let active_deadline = |deadline: Option<u32>| {
            deadline.filter(|until| !crate::continuum::continuum_tick_reached(tick, *until))
        };
        let super_until_tick = active_deadline(client.server_super_until_tick);
        let shield_until_tick = active_deadline(client.server_shield_until_tick);
        let shutdown_application_pending = client.server_engine_shutdown_start_sequence.is_none()
            && client
                .pending_prize_effect_applications
                .iter()
                .any(|pending| pending.prize_id.checked_abs() == Some(14));
        let movement_application_pending = client
            .pending_prize_effect_applications
            .iter()
            .any(|pending| pending.movement_tuning_before_bits.is_some());
        let prize_shutdown_is_visible =
            client.server_engine_shutdown_start_sequence.is_some() || shutdown_application_pending;
        let engine_shutdown_until_tick = prize_shutdown_is_visible
            .then(|| active_deadline(client.server_engine_shutdown_until_tick))
            .flatten();
        // The reference game's EMP pauses recharge without shutting down movement/fire.
        let emp_until_tick = active_deadline(client.server_emp_until_tick);
        prize_flags |= u16::from(super_until_tick.is_some()) << 7;
        prize_flags |= u16::from(shield_until_tick.is_some()) << 8;
        prize_flags |= u16::from(engine_shutdown_until_tick.is_some()) << 9;
        prize_flags |= u16::from(emp_until_tick.is_some()) << 13;
        if client.maxed_spawn_loadout {
            prize_flags |= SnapshotPrizeState::MAXED_SPAWN_LOADOUT;
        }
        let damage_loss = client
            .last_damage_prize_loss_seq
            .zip(client.last_damage_prize_loss_id);
        let has_damage_loss = damage_loss.is_some();
        prize_flags |= u16::from(has_damage_loss) << 10;
        prize_flags |= u16::from(shutdown_application_pending) << 11;
        prize_flags |= u16::from(movement_application_pending) << 12;
        let prize_state = SnapshotPrizeState {
            life_seq: client.life_seq,
            revision: client.server_prize_state_revision.max(1),
            flags: prize_flags,
            bounty: client.server_bounty,
            max_energy: client.server_max_energy,
            rotation_bits: client.server_rotation_per_second.to_bits(),
            thrust_bits: client.server_thrust_acceleration.to_bits(),
            top_speed_bits: client.server_top_speed.to_bits(),
            recharge_bits: client.server_recharge_per_second.to_bits(),
            gun_level: client.server_weapons.gun_level,
            bomb_level: client.server_weapons.bomb_level,
            shrapnel: client.server_weapons.shrapnel,
            repels: client.server_weapons.repels,
            bursts: client.server_weapons.bursts,
            decoys: client.server_weapons.decoys,
            thors: client.server_weapons.thors,
            bricks: client.server_weapons.bricks,
            portals: client.server_weapons.portals,
            super_until_tick,
            shield_until_tick,
            engine_shutdown_until_tick,
            emp_until_tick,
            damage_loss_seq: damage_loss.map(|(seq, _)| seq),
            damage_loss_prize_id: damage_loss.map(|(_, id)| id),
            energy: Some(client.server_energy),
        };

        // Each event is self-contained and already captured against the
        // recipient runtime. The tick's events are sent in one call, so the
        // RTC edge can carry them in one message; this queue neither coalesces
        // nor embeds private snapshot state in the event carrier.
        let mut sending = Vec::new();
        let mut packets = Vec::new();
        let mut withheld = Vec::new();
        while let Some(envelope) = realtime_events.queue.pop_front() {
            #[cfg(debug_assertions)]
            if projectile_presentation_identity(envelope.payload).is_some()
                && crate::qa_presentation_hold::holds(&client.player.name)
            {
                realtime_events.queue.push_front(envelope);
                break;
            }
            if let crate::realtime_event::RealtimeEventPayload::ProjectileState(state) =
                envelope.payload
                && let Some(field) = state.invalid_field()
            {
                warn_withheld_projectile_state(recipient_id, envelope.event_id, tick, field, state);
                withheld.push(envelope);
                continue;
            }
            let packet = crate::realtime_event::AuthenticatedRealtimeEvent {
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: client.arena_stream_id,
                event_id: envelope.event_id,
                server_tick: envelope.server_tick,
                payload: envelope.payload,
            }
            .encode();
            if !client.send_pacing.try_consume(packet.len()) {
                emission.realtime_event_paced_out();
                realtime_events.queue.push_front(envelope);
                break;
            }
            sending.push(envelope);
            packets.push(packet);
        }
        let kinds = packets.iter().map(|packet| packet[2]).collect::<Vec<_>>();
        // Dedicated facts are already authenticated and independently
        // deduplicated at the client. The snapshot loss simulator must never
        // discard them before an RTC channel sees the bytes.
        if !packets.is_empty() && client.tx.send_events(packets).is_err() {
            // Nothing was queued; keep the tick's events ahead of any paced-out ones.
            for envelope in sending.drain(..).rev() {
                realtime_events.queue.push_front(envelope);
            }
        }
        for (envelope, kind) in sending.iter().zip(kinds) {
            emission.realtime_event_sent(kind);
            if let Some(identity) = projectile_presentation_identity(envelope.payload) {
                let _ = client.threat_presentation.observe_projectile_emitted(
                    identity,
                    envelope.event_id,
                    tick,
                );
            }
        }
        // A withheld state keeps the presentation clock it would have had if
        // sent and rejected, so a pending hit is never held back by it.
        for envelope in &withheld {
            if let Some(identity) = projectile_presentation_identity(envelope.payload) {
                let _ = client.threat_presentation.observe_projectile_emitted(
                    identity,
                    envelope.event_id,
                    tick,
                );
            }
        }
        client
            .pending_realtime_events
            .queue
            .append(&mut realtime_events.queue);

        // Snapshot id zero is never allocated even though FULL no longer
        // depends on it.
        let snapshot_id = next_snapshot_id(client.snapshot_seq);
        client.snapshot_seq = snapshot_id;

        let prize_baseline_released = qa_prize_baseline_delay_released(client, snapshot_id);
        let prize_state = prize_baseline_released.then_some(prize_state);
        // A compact-private client is sent the full prize body when it differs
        // from the one its acknowledged base guarantees (other than energy), at
        // least every PRIZE_BODY_REFRESH_SNAPSHOTS, and in full frames.
        // Otherwise it gets a reference to that body with the current energy,
        // which its decoder expands into the same PrizeState.
        let (prize_state, prize_reference, prize_known) = match (prize_state, base_prize_known) {
            (Some(current), Some((carrier, known)))
                if compact_private
                    && current.energy.is_some()
                    && SnapshotPrizeState {
                        energy: None,
                        ..current
                    } == SnapshotPrizeState {
                        energy: None,
                        ..known
                    }
                    && snapshot_id.wrapping_sub(carrier) < PRIZE_BODY_REFRESH_SNAPSHOTS =>
            {
                let reference = crate::snapshot_wire::SnapshotPrizeReference {
                    snapshot_id: carrier,
                    energy: current.energy.unwrap_or_default(),
                };
                (None, Some(reference), Some((carrier, known)))
            }
            (Some(current), _) => (Some(current), None, Some((snapshot_id, current))),
            (None, known) => (None, None, known),
        };
        let arena_stream_id = client.arena_stream_id;
        let observer_timeline = client.observer_combat_timeline.current_directive();
        let input_applied =
            client
                .last_position_sequence
                .map(|through_sequence| SnapshotInputApplied {
                    life_seq: client.life_seq,
                    through_sequence,
                });
        let build = |near_entities: &[SnapshotEntity], mid_entities: &[SnapshotEntity]| {
            build_snapshot(
                arena_stream_id,
                snapshot_id,
                base_id,
                flags,
                tick,
                near_entities,
                mid_entities,
                &events,
                Some(rocket_state),
                Some(lifecycle_state),
                prize_state,
                prize_reference,
                observer_timeline,
                input_applied,
            )
        };
        let mut packet = build(&near_entities, &mid_entities);

        // Running low on the send budget: hold some listed actors at their
        // acknowledged state so frames shrink towards one tick's refill and
        // the budget stops draining, instead of skipping whole frames once it
        // is empty. Holding changes what this recipient is shown, so the
        // presentation record and history below use the held state too.
        let budget = client.send_pacing.tokens.max(0.0) as usize;
        let refill = (client.send_pacing.cap_bytes_per_second / f64::from(tick_rate_hz)) as usize;
        let target = snapshot_hold_target(packet.len(), budget, refill);
        let mut held = Vec::new();
        if packet.len() > target
            && let Some(base_entities) = base_entities.as_deref()
        {
            let origin = near_raw
                .iter()
                .find(|(id, _)| *id == recipient_id)
                .map(|(_, own)| (own.x, own.y));
            held = choose_snapshot_holds(
                &near_raw,
                &near_entities,
                &mid_raw,
                &mid_entities,
                base_entities,
                recipient_id,
                origin,
                &client.snapshot_holds,
                compact_tick,
                packet.len() - target,
            );
            if !held.is_empty() {
                for (id, current) in &mut near_raw {
                    if let Some(base_state) = base_entities.get(id).filter(|_| held.contains(id)) {
                        *current = *base_state;
                    }
                }
                for (id, x, y, _) in &mut mid_raw {
                    if let Some(base_state) = base_entities.get(id).filter(|_| held.contains(id)) {
                        (*x, *y) = (base_state.x, base_state.y);
                    }
                }
                for sample in &mut presentation_actors {
                    let id = sample.actor.player_id();
                    if let Some(base_state) = base_entities.get(&id).filter(|_| held.contains(&id))
                    {
                        *sample = observer_presentation_sample(
                            sample.actor,
                            sample.motion_segment,
                            base_state,
                        );
                    }
                }
                (near_entities, mid_entities) =
                    encode_entities(&near_raw, &mid_raw, Some(base_entities));
                packet = build(&near_entities, &mid_entities);
            }
        }

        // Record this snapshot's full entity map in history so it
        // can serve as a base for future deltas once the client acks
        // it. Drop history older than `last_acked_snapshot_id` plus
        // cap at `SNAPSHOT_HISTORY_CAPACITY` so memory is bounded.
        let mut entities = SnapshotEntityMap::new();
        for (id, state) in &near_raw {
            entities.insert(*id, *state);
        }
        // Mid-tier entries only carry x/y; pad the rest with defaults
        // so the next delta can compare uniformly.
        for &(id, x, y, life_seq) in &mid_raw {
            entities.insert(
                id,
                NearEntityState {
                    x,
                    y,
                    life_seq,
                    ..NearEntityState::default()
                },
            );
        }
        client.snapshot_history.push_back(SnapshotHistoryEntry {
            snapshot_id,
            server_tick: tick,
            entities: std::sync::Arc::new(entities),
            emitted_at: std::time::Instant::now(),
            prize_known,
        });
        trim_snapshot_history(&mut client.snapshot_history, client.last_acked_snapshot_id);

        #[cfg(test)]
        publication_phase.advance();
        // Phase 4 send pacing: snapshot is the high-volume outbound
        // path. If the per-client token bucket is empty (runaway AOI
        // density or oversized frame), skip this emission; the next
        // received frame remains a delta against the last acknowledged base.
        //
        // Phase 5 verification: when `ZUBSPACE_SERVER_TX_LOSS_PCT` is
        // set, randomly drop the frame to simulate WebRTC unreliable
        // channel loss. Counter increment lives inside the predicate.
        let mut qa_snapshot_event = None;
        let within_budget = client.send_pacing.try_consume(packet.len());
        if !within_budget {
            emission.snapshot_paced_out();
        }
        if within_budget && !client.should_drop_simulated_tx(packet.len()) {
            let packet_len = packet.len();
            let sent = client.tx.send(packet).is_ok();
            if sent {
                emission.sent(flags & SNAPSHOT_FLAG_FULL != 0, packet_len);
                emission.entities_held(held.len());
                client.snapshot_holds.retain(|id, _| held.contains(id));
                for id in &held {
                    *client.snapshot_holds.entry(*id).or_insert(0) += 1;
                }
                for source in emitted_actor_runtimes.iter().copied() {
                    client
                        .threat_presentation
                        .observe_emitted(source, snapshot_id, tick);
                }
                client
                    .observer_presentation_history
                    .begin_scope(current_recipient, client.arena_stream_id);
                let disposition = client.observer_presentation_history.observe_emitted(
                    crate::combat_timeline::ObserverPresentationFrameV1 {
                        observer: current_recipient,
                        arena_stream_id: client.arena_stream_id,
                        snapshot_id,
                        server_tick: tick,
                        actors: presentation_actors,
                    },
                );
                debug_assert_eq!(
                    disposition,
                    crate::combat_timeline::ObserverPresentationAppendDisposition::Applied,
                    "successful snapshot emission must append one exact presentation frame"
                );
            }
            if sent && let Some(bundle_id) = client.qa_trace_bundle_id {
                qa_snapshot_event = Some((bundle_id, packet_len, client.life_seq));
            }
            if sent && trace_bot_snapshots {
                tracing::info!(
                    server_tick = tick,
                    recipient_id,
                    recipient_life_seq = client.life_seq,
                    snapshot_id,
                    base_id,
                    full = flags & SNAPSHOT_FLAG_FULL != 0,
                    packet_len,
                    bot_near_ids = ?trace_bot_near_ids.as_deref().unwrap_or_default(),
                    bot_mid_ids = ?trace_bot_mid_ids.as_deref().unwrap_or_default(),
                    "server bot snapshot trace"
                );
            }
            if sent && delivers_pending_repel {
                if let Some(observation) = client.pending_repel_observations.front_mut() {
                    let slot = usize::from(observation.delivered_snapshot_cursor)
                        % crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS;
                    observation.delivered_snapshots[slot] =
                        Some(crate::state::ServerRepelDelivery {
                            snapshot_id,
                            snapshot_tick: tick,
                            active_steps: repel_active_steps
                                .expect("delivered Repel has remaining steps"),
                            acknowledged_at_tick: None,
                        });
                    observation.delivered_snapshot_cursor =
                        observation.delivered_snapshot_cursor.wrapping_add(1);
                    observation.delivered_snapshot_id = Some(snapshot_id);
                    observation.delivered_snapshot_tick = Some(tick);
                    observation.delivered_active_steps = repel_active_steps;
                    observation.delivery_acked = false;
                }
            }
        } else {
            // [REVIEW H5] The frame was dropped by send pacing or the
            // loss simulator. Positional state is self-healing (the next
            // frame overrides), but the drained events are one-shot
            // snapshot combat copies. Re-queue them so a later state frame
            // can still provide the compatibility copy; the dedicated 0x7d
            // event has its own non-coalesced, independently sequenced path.
            client.pending_snapshot_events = events;
        }
        if let Some((bundle_id, packet_len, recipient_life_seq)) = qa_snapshot_event {
            state.qa_traces.record_for_bundle_lazy(bundle_id, || {
                serde_json::json!({
                    "type": "snapshot_emitted",
                    "serverTick": tick,
                    "snapshotId": snapshot_id,
                    "baseId": base_id,
                    "full": flags & SNAPSHOT_FLAG_FULL != 0,
                    "packetBytes": packet_len,
                    "recipientId": recipient_id,
                    "recipientLifeSeq": recipient_life_seq,
                    "nearActors": near_raw
                        .iter()
                        .map(|(id, state)| serde_json::json!({
                            "playerId": id,
                            "lifeSeq": state.life_seq,
                        }))
                        .collect::<Vec<_>>(),
                    "midActors": mid_raw
                        .iter()
                        .map(|(id, _, _, life_seq)| serde_json::json!({
                            "playerId": id,
                            "lifeSeq": life_seq,
                        }))
                        .collect::<Vec<_>>(),
                })
            });
        }
    }
    state.telemetry.record_snapshot_emission(emission);
    if !shed_frame {
        for client in state.clients.values_mut() {
            client.pending_snapshot_weapon = 0;
        }
    }
    state.realtime_event_queue_peak = state
        .realtime_event_queue_peak
        .max(active_projectile_queue_peak);
}

/// Linear scan to find a snapshot in history. Capacity is small
/// (`SNAPSHOT_HISTORY_CAPACITY`) so this stays sub-microsecond.
fn find_in_history(
    history: &VecDeque<SnapshotHistoryEntry>,
    snapshot_id: u16,
) -> Option<&SnapshotHistoryEntry> {
    history
        .iter()
        .find(|entry| entry.snapshot_id == snapshot_id)
}

fn trim_snapshot_history(
    history: &mut VecDeque<SnapshotHistoryEntry>,
    active_base_id: Option<u16>,
) {
    if let Some(active_base_id) = active_base_id {
        history.retain(|entry| {
            entry.snapshot_id == active_base_id
                || !snapshot_id_is_forward(active_base_id, entry.snapshot_id)
        });
    }
    while history.len() > SNAPSHOT_HISTORY_CAPACITY {
        let removable = history
            .iter()
            .position(|entry| Some(entry.snapshot_id) != active_base_id)
            .expect("history over capacity must contain a non-active entry");
        history.remove(removable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoy_recovery_is_exact_clocked_and_refreshes_already_known_state() {
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let owner_id = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, owner_id, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let observer_id = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, observer_id, 8);
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: owner_id,
            source_session_id: owner.session.session_id.get(),
            source_life_seq: owner.life.life_seq,
            shot_group_id: 42,
            x: 100.0,
            y: 100.0,
            velocity_x: 50.0,
            velocity_y: 0.0,
            orientation: 0.25,
            frequency: 0,
            ship: 0,
            weapon_data: 6,
        };
        state.active_decoys.push(crate::state::ServerDecoy {
            presentation_id: 99,
            presentation_source: Some(source),
            source_input_sequence: Some(55),
            owner,
            source_weapon_data: 6,
            frequency: 0,
            ship: 0,
            position_x: 150.0,
            position_y: 100.0,
            velocity_x: 50.0,
            velocity_y: 0.0,
            orientation: 0.25,
            created_tick: 100,
            last_step_tick: 200,
            expires_at_tick: 500,
        });
        let observer = state.clients.get_mut(&observer_id).unwrap();
        observer.snapshot_ready = true;
        let position = observer.aoi_observer_position.as_mut().unwrap();
        position[4..6].copy_from_slice(&100_u16.to_le_bytes());
        position[15..17].copy_from_slice(&100_u16.to_le_bytes());
        let initial = active_projectile_state_for_recipient(&state, observer_id, 100.0, 100.0);
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].1.source_input_sequence, Some(55));
        assert_eq!(initial[0].1.source_server_tick, 100);
        assert_eq!(initial[0].1.expiry_tick, Some(500));
        assert!(
            active_projectile_state_for_recipient(&state, observer_id, 10000.0, 10000.0).is_empty()
        );
        let scope = state.clients[&observer_id].arena_stream_id;
        drop(state);
        emit_snapshots_for_tick_at_tick(&shared, 200);
        assert!(rx.try_iter().any(|bytes| crate::realtime_event::AuthenticatedRealtimeEvent::parse(&bytes).is_ok_and(|event|
            matches!(event.payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(p) if p.projectile_id == 99 && p.weapon_data == 6))));
        assert!(
            active_projectile_state_for_recipient(
                &shared.lock().unwrap(),
                observer_id,
                100.0,
                100.0
            )
            .is_empty()
        );
        let refresh = crate::snapshot_wire::ProjectileRefreshRequestPacket {
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: scope,
        };
        crate::connection::record_snapshot_full_request(&shared, observer_id, &refresh.encode());
        assert_eq!(
            active_projectile_state_for_recipient(
                &shared.lock().unwrap(),
                observer_id,
                100.0,
                100.0
            )
            .len(),
            1
        );
    }

    /// An observer with an owner's Thor in range, whose sensor is armed on
    /// `proximity_target`. Returns the shared state, the observer's id and
    /// receiver, and the Thor's projectile id.
    fn observer_with_armed_thor(
        proximity_target: crate::authority_identity::LifeIdentity,
    ) -> (
        std::sync::Arc<std::sync::Mutex<crate::state::ServerState>>,
        u16,
        std::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let owner_id = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, owner_id, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let observer_id = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, observer_id, 8);
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: owner_id,
            source_session_id: owner.session.session_id.get(),
            source_life_seq: owner.life.life_seq,
            shot_group_id: 43,
            x: 100.0,
            y: 100.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.25,
            frequency: state.clients[&owner_id].player.frequency,
            ship: 0,
            weapon_data: 8,
        };
        state.record_projectile(crate::state::ServerProjectile {
            id: 77,
            shot_group_id: 43,
            shot_index: 0,
            owner_id,
            owner_life_seq: owner.life.life_seq,
            presentation_source: Some(source),
            combat_timeline: None,
            weapon_data: 8,
            x: 100,
            y: 100,
            timestamp: 100,
            created_tick: 100,
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: Some(500),
            pos_x: 100.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 100.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: Some(proximity_target),
            proximity_sensor_end_tick: Some(300),
            proximity_highest_offset: 40.0,
        });
        let observer = state.clients.get_mut(&observer_id).unwrap();
        observer.snapshot_ready = true;
        let position = observer.aoi_observer_position.as_mut().unwrap();
        position[4..6].copy_from_slice(&100_u16.to_le_bytes());
        position[15..17].copy_from_slice(&100_u16.to_le_bytes());
        drop(state);
        (shared, observer_id, rx)
    }

    fn sent_projectile_states(
        rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    ) -> Vec<crate::realtime_event::RealtimeProjectileStateEvent> {
        rx.try_iter()
            .filter(|packet| packet.first() == Some(&crate::realtime_event::REALTIME_EVENT_PACKET))
            .map(|packet| {
                crate::realtime_event::AuthenticatedRealtimeEvent::parse(&packet)
                    .expect("every sent realtime event parses at the client")
            })
            .filter_map(|event| match event.payload {
                crate::realtime_event::RealtimeEventPayload::ProjectileState(state) => Some(state),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_thor_with_an_armed_sensor_is_sent_as_a_state_the_client_accepts() {
        let (shared, observer_id, rx) =
            observer_with_armed_thor(crate::authority_identity::LifeIdentity::new(9, 3));
        let built = active_projectile_state_for_recipient(
            &shared.lock().unwrap(),
            observer_id,
            100.0,
            100.0,
        );
        assert_eq!(built.len(), 1);
        let proximity = built[0].1.proximity.expect("the armed sensor is published");
        assert_eq!(
            (proximity.target_player_id, proximity.target_life_seq),
            (9, 3)
        );
        assert_eq!(built[0].1.invalid_field(), None);
        emit_snapshots_for_tick_at_tick(&shared, 200);
        let sent = sent_projectile_states(&rx);
        assert!(
            sent.iter().any(|state| state.projectile_id == 77
                && state.weapon_data == 8
                && state.proximity == Some(proximity)),
            "the Thor state reaches the observer: {sent:?}"
        );
    }

    #[test]
    fn a_state_the_client_would_reject_is_withheld_but_keeps_its_presentation_clock() {
        // A sensor bound to life zero fails the client's validation.
        let (shared, observer_id, rx) =
            observer_with_armed_thor(crate::authority_identity::LifeIdentity::new(9, 0));
        let built = active_projectile_state_for_recipient(
            &shared.lock().unwrap(),
            observer_id,
            100.0,
            100.0,
        );
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].1.invalid_field(), Some("proximity"));
        let identity = built[0].0;
        let withheld = withheld_projectile_states();
        emit_snapshots_for_tick_at_tick(&shared, 200);
        assert!(
            sent_projectile_states(&rx).is_empty(),
            "the invalid state never reaches the wire"
        );
        assert!(withheld_projectile_states() > withheld);
        let state = shared.lock().unwrap();
        let client = &state.clients[&observer_id];
        assert!(client.pending_realtime_events.queue.is_empty());
        let open_tick = 200 + crate::threat_presentation::PROJECTILE_PRESENTATION_LEAD_TICKS;
        let readiness = client
            .threat_presentation
            .exact_projectile_readiness(identity, open_tick);
        assert!(
            readiness.harmful,
            "readiness opens as if the state had been sent: {readiness:?}"
        );
    }

    #[test]
    fn owner_reconciliation_recovery_keeps_launch_input_across_respawn_and_rejects_other_sessions()
    {
        let shared =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let id = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, id, 0);
        let (observer_tx, observer_rx) = std::sync::mpsc::channel();
        let observer_id = crate::arena::register_connection(&shared, observer_tx);
        crate::arena::join_arena(&shared, observer_id, 8);
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(id).unwrap();
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: id,
            source_session_id: owner.session.session_id.get(),
            source_life_seq: owner.life.life_seq,
            shot_group_id: 41,
            x: 100.0,
            y: 100.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            frequency: state.clients[&id].player.frequency,
            ship: 0,
            weapon_data: 2,
        };
        state.record_projectile(crate::state::ServerProjectile {
            id: 99,
            shot_group_id: 41,
            shot_index: 0,
            owner_id: id,
            owner_life_seq: owner.life.life_seq,
            presentation_source: Some(source),
            combat_timeline: Some(crate::combat_timeline::ProjectileCombatTimelineV1 {
                owner,
                owner_input_sequence: u16::MAX,
                accepted_timeline_generation: 1,
                accepted_game_applied_snapshot_id: 1,
                accepted_server_tick: 100,
                maximum_rewind_ticks: 7,
            }),
            weapon_data: 2,
            x: 100,
            y: 100,
            timestamp: 100,
            created_tick: 100,
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: Some(500),
            pos_x: 100.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 100.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::BouncingBullet,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
        let initial = active_projectile_state_for_recipient(&state, id, 100.0, 100.0);
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].1.source_input_sequence, Some(u16::MAX));
        state
            .advance_client_life_after_damage_death(id, 110)
            .unwrap();
        state.clients.get_mut(&id).unwrap().alive = false;
        assert!(active_projectile_state_for_recipient(&state, id, 100.0, 100.0).is_empty());
        state.clients.get_mut(&id).unwrap().alive = true;
        let recovered = active_projectile_state_for_recipient(&state, id, 100.0, 100.0);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].1, initial[0].1);
        let frequency = state.clients[&id].player.frequency;
        let observer = state.clients.get_mut(&observer_id).unwrap();
        observer.snapshot_ready = true;
        observer.alive = false;
        observer.player.frequency = frequency; // spectators see every frequency
        let position = observer.aoi_observer_position.as_mut().unwrap();
        position[4..6].copy_from_slice(&100_u16.to_le_bytes());
        position[15..17].copy_from_slice(&100_u16.to_le_bytes());
        assert!(observer.last_position.is_none());
        assert_eq!(
            projectile_recovery_center(&state, observer_id, &[]),
            Some((100.0, 100.0))
        );
        assert_eq!(
            active_projectile_state_for_recipient(&state, observer_id, 100.0, 100.0).len(),
            1
        );
        assert!(
            active_projectile_state_for_recipient(&state, observer_id, 10000.0, 10000.0).is_empty()
        );
        assert_eq!(projectile_recovery_center(&state, id, &[]), None);
        drop(state);
        emit_snapshots_for_tick_at_tick(&shared, 200);
        let state = shared.lock().unwrap();
        assert!(observer_rx.try_iter().any(|packet| {
            crate::realtime_event::AuthenticatedRealtimeEvent::parse(&packet)
                .is_ok_and(|event| matches!(event.payload,
                    crate::realtime_event::RealtimeEventPayload::ProjectileState(p)
                    if p.projectile_id == 99 && p.source.source_session_id == owner.session.session_id.get()))
        }));
        assert!(state.clients[&observer_id].last_position.is_none());
        assert!(!state.clients[&observer_id].alive);
        assert!(
            active_projectile_state_for_recipient(&state, observer_id, 100.0, 100.0).is_empty()
        );
        let readiness_before = state.clients[&observer_id].threat_presentation.clone();
        let refresh = crate::snapshot_wire::ProjectileRefreshRequestPacket {
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: state.clients[&observer_id].arena_stream_id,
        };
        let now = std::time::Instant::now();
        drop(state);
        let wrong_scope = crate::snapshot_wire::ProjectileRefreshRequestPacket {
            arena_stream_id: refresh.arena_stream_id + 1,
            ..refresh
        };
        crate::connection::record_snapshot_full_request_at(
            &shared,
            observer_id,
            &wrong_scope.encode(),
            now,
        );
        assert!(
            shared.lock().unwrap().clients[&observer_id]
                .projectile_refresh_pending
                .is_empty()
        );
        crate::connection::record_snapshot_full_request_at(
            &shared,
            observer_id,
            &refresh.encode(),
            now,
        );
        let mut state = shared.lock().unwrap();
        assert_eq!(
            state.clients[&observer_id].projectile_refresh_pending.len(),
            1
        );
        assert_eq!(
            active_projectile_state_for_recipient(&state, observer_id, 100.0, 100.0).len(),
            1,
            "explicit refresh includes an already-known exact projectile"
        );
        assert!(
            active_projectile_state_for_recipient(&state, observer_id, 10000.0, 10000.0).is_empty(),
            "refresh preserves AOI"
        );
        assert_eq!(
            state.clients[&observer_id].threat_presentation, readiness_before,
            "pose recovery must not buy another harmless interval"
        );
        state
            .clients
            .get_mut(&observer_id)
            .unwrap()
            .projectile_refresh_pending
            .clear();
        drop(state);
        crate::connection::record_snapshot_full_request_at(
            &shared,
            observer_id,
            &refresh.encode(),
            now,
        );
        assert!(
            shared.lock().unwrap().clients[&observer_id]
                .projectile_refresh_pending
                .is_empty(),
            "duplicate refresh is rate limited"
        );
        crate::connection::record_snapshot_full_request_at(
            &shared,
            observer_id,
            &refresh.encode(),
            now + std::time::Duration::from_secs(2),
        );
        emit_snapshots_for_tick_at_tick(&shared, 201);
        assert!(observer_rx.try_iter().any(|packet| {
            crate::realtime_event::AuthenticatedRealtimeEvent::parse(&packet).is_ok_and(|event|
                matches!(event.payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(p) if p.projectile_id == 99))
        }));
        let mut state = shared.lock().unwrap();
        assert!(
            state.clients[&observer_id]
                .projectile_refresh_pending
                .is_empty()
        );
        assert_eq!(
            state.clients[&observer_id].threat_presentation,
            readiness_before
        );
        state.clients.get_mut(&id).unwrap().session_id =
            crate::authority_identity::PlayerSessionId::new(owner.session.session_id.get() + 1);
        assert!(active_projectile_state_for_recipient(&state, id, 100.0, 100.0).is_empty());
    }

    #[test]
    fn snapshot_interval_is_2_ticks_at_local_60_hz() {
        // 60 Hz tick / 30 Hz snapshot = every other tick.
        assert_eq!(snapshot_interval_ticks(60), 2);
        assert_eq!(snapshot_interval_ticks(40), 1);
    }

    #[test]
    fn snapshot_interval_clamps_to_one_at_low_tick_rate() {
        assert_eq!(snapshot_interval_ticks(10), 1);
        assert_eq!(snapshot_interval_ticks(0), 1);
    }

    #[test]
    fn near_snapshot_interval_targets_min_tick_and_60_hz() {
        // Near = min(tick, 60): every tick at 40/60 Hz, every other at 120.
        assert_eq!(near_snapshot_interval_ticks(60), 1);
        assert_eq!(near_snapshot_interval_ticks(40), 1);
        assert_eq!(near_snapshot_interval_ticks(120), 2);
        assert_eq!(near_snapshot_interval_ticks(10), 1);
        assert_eq!(near_snapshot_interval_ticks(0), 1);
        // The near interval must always divide the mid interval so every
        // mid-bearing frame is also a near frame.
        for hz in [40u16, 60, 90, 120] {
            assert_eq!(
                snapshot_interval_ticks(hz) % near_snapshot_interval_ticks(hz),
                0,
                "near interval must divide mid interval at {hz} Hz"
            );
        }
    }

    #[test]
    fn empty_snapshot_packet_uses_correct_type_and_length() {
        let packet = build_empty_snapshot(0x1234, 0, SNAPSHOT_FLAG_FULL, 0x0001_0203);
        assert_eq!(packet[0], SNAPSHOT_PACKET);
        assert_eq!(packet.len(), SNAPSHOT_HEADER_LEN);
        assert_eq!(packet[18], 0); // near_count
        assert_eq!(packet[19], 0); // mid_count
        assert_eq!(packet[20], 0); // event_count
    }

    #[test]
    fn snapshot_with_entities_serializes_full_body() {
        let near = vec![
            SnapshotEntity::full(7, 100, 200),
            SnapshotEntity::full(8, 300, 400),
        ];
        let mid = vec![SnapshotEntity::full(9, 5_000, 6_000)];
        let packet = build_snapshot(
            1,
            1,
            0,
            SNAPSHOT_FLAG_FULL,
            99,
            &near,
            &mid,
            &PendingSnapshotEvents::default(),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        // 12 header + 3 x+y entities × 8 bytes each (4 header + x + y).
        assert_eq!(packet.len(), SNAPSHOT_HEADER_LEN + 3 * 8);
        assert_eq!(packet[18], 2); // near_count
        assert_eq!(packet[19], 1); // mid_count
        assert_eq!(packet[20], 0); // event_count
        // First entity at offset 21: id (2 bytes), mask u16 (2 bytes), x, y.
        assert_eq!(u16::from_le_bytes([packet[21], packet[22]]), 7);
        assert_eq!(
            u16::from_le_bytes([packet[23], packet[24]]),
            SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y
        );
        assert_eq!(u16::from_le_bytes([packet[25], packet[26]]), 100);
        assert_eq!(u16::from_le_bytes([packet[27], packet[28]]), 200);
    }
}
