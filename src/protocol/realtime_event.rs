//! Authenticated, arena-generation-bound realtime one-shot events.
//!
//! These packets are deliberately independent from `0x70` snapshot state.
//! The server may duplicate the exact bytes onto the game and event RTC
//! channels; the client validates the reliable-bootstrap identity before any
//! event is published and deduplicates the wrapping event id separately from
//! snapshot ordering.

#![allow(dead_code)]

use crate::packet_io::{read_u16, read_u32};

pub(crate) const REALTIME_EVENT_PACKET: u8 = 0x7d;
/// A recipient can have all 3,072 committed server envelopes overtake an
/// older copy on the other unordered channel. Keep one power-of-two window
/// above that bound so an unseen old copy remains admissible while duplicates
/// and packets outside the bounded outstanding set fail closed.
pub(crate) const REALTIME_EVENT_REORDER_WINDOW_BITS: usize = 4_096;
pub(crate) const REALTIME_EVENT_VERSION: u8 = 2;
pub(crate) const REALTIME_EVENT_WEAPON: u8 = 1;
pub(crate) const REALTIME_EVENT_WATCH_DAMAGE: u8 = 2;
pub(crate) const REALTIME_EVENT_REPEL_MUTATIONS: u8 = 3;
pub(crate) const REALTIME_EVENT_PROJECTILE_STATE: u8 = 4;
pub(crate) const REALTIME_EVENT_OWNER_WEAPONS_CANCELLED: u8 = 5;
const OWNER_WEAPONS_CANCELLED_PACKET_LEN: usize = HEADER_LEN + 2 + 8 + 8 + 4 + 1 + 2;
pub(crate) const REPEL_MUTATION_PROJECTILE: u8 = 1;
pub(crate) const REPEL_MUTATION_DECOY: u8 = 2;
const REPEL_MUTATION_MOTION_FROZEN: u8 = 0x80;
pub(crate) const MAX_REPEL_MUTATIONS_PER_EVENT: usize = 16;
const HEADER_LEN: usize = 1 + 1 + 1 + 8 + 8 + 4 + 4;
const WEAPON_PAYLOAD_LEN: usize = 2 + 8 + 4 + 8 + 5 * 4 + 2 + 1 + 2;
pub(crate) const REALTIME_WEAPON_PACKET_LEN: usize = HEADER_LEN + WEAPON_PAYLOAD_LEN;
const WATCH_DAMAGE_PACKET_LEN: usize = HEADER_LEN + 2 * (2 + 8 + 4) + 2 + 2 + 2 + 2 + 4 + 4 + 1;
const REPEL_MUTATION_STATE_LEN: usize = 1 + 2 + 8 + 4 + 4 + 2 + 1 + 2 + 4 * 4 + 4 + 1;
const MAX_REPEL_MUTATIONS_PACKET_LEN: usize =
    HEADER_LEN + 1 + MAX_REPEL_MUTATIONS_PER_EVENT * REPEL_MUTATION_STATE_LEN;
const PROJECTILE_STATE_TAIL_LEN: usize = 4 + 4 + 1 + 2 + 4 * 4 + 4 + 1 + 1 + 1 + 2 + 4 + 4 + 4;
const PROJECTILE_STATE_PACKET_LEN: usize =
    HEADER_LEN + WEAPON_PAYLOAD_LEN + PROJECTILE_STATE_TAIL_LEN;
const _: () = assert!(OWNER_WEAPONS_CANCELLED_PACKET_LEN <= MAX_REALTIME_EVENT_PACKET_LEN);
pub(crate) const MAX_REALTIME_EVENT_PACKET_LEN: usize =
    if MAX_REPEL_MUTATIONS_PACKET_LEN > PROJECTILE_STATE_PACKET_LEN {
        MAX_REPEL_MUTATIONS_PACKET_LEN
    } else {
        PROJECTILE_STATE_PACKET_LEN
    };

pub(crate) const PROJECTILE_STATE_MOTION_FROZEN: u8 = 1 << 4;
pub(crate) const PROJECTILE_STATE_TERMINAL: u8 = 1 << 2;
pub(crate) const PROJECTILE_STATE_HAS_INPUT_SEQUENCE: u8 = 1 << 3;
pub(crate) const PROJECTILE_STATE_HAS_EXPIRY: u8 = 1 << 0;
pub(crate) const PROJECTILE_STATE_HAS_PROXIMITY_SENSOR: u8 = 1 << 1;
pub(crate) const PROJECTILE_BEHAVIOR_STANDARD: u8 = 0;
pub(crate) const PROJECTILE_BEHAVIOR_BURST_INACTIVE: u8 = 1;
pub(crate) const PROJECTILE_BEHAVIOR_BURST_ACTIVE: u8 = 2;

fn read_u64(packet: &[u8], offset: usize) -> Result<u64, String> {
    let bytes = packet
        .get(offset..offset + 8)
        .ok_or_else(|| format!("packet missing u64 at offset {offset}"))?;
    Ok(u64::from_le_bytes(bytes.try_into().expect("eight bytes")))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RealtimeWeaponEvent {
    pub(crate) source_player_id: u16,
    pub(crate) source_session_id: u64,
    pub(crate) source_life_seq: u32,
    pub(crate) shot_group_id: u64,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) orientation: f32,
    pub(crate) frequency: u16,
    pub(crate) ship: u8,
    pub(crate) weapon_data: u16,
}

/// Self-contained, replaceable-by-newer-current-state presentation fact for
/// one authoritative projectile. It deliberately repeats the immutable weapon
/// source so a recipient entering the projectile's AOI after the original
/// fan-out can render it without synthesizing a source actor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RealtimeProjectileStateEvent {
    /// Exact accepted human input which launched this shot, for owner prediction
    /// reconciliation. Absent for native sources and legacy state carriers.
    pub(crate) source_input_sequence: Option<u16>,
    /// An authoritative consumed projectile. Ordinary guns terminate the linked
    /// shot group; burst/shrapnel children terminate independently.
    pub(crate) terminal: bool,
    /// Flight is held at a server-detected contact while its immutable damage
    /// transaction waits. This is not consumption and grants no damage authority.
    pub(crate) motion_frozen: bool,
    pub(crate) source: RealtimeWeaponEvent,
    pub(crate) projectile_id: u32,
    pub(crate) source_server_tick: u32,
    pub(crate) source_projectile_index: u8,
    pub(crate) weapon_data: u16,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) expiry_tick: Option<u32>,
    pub(crate) behavior: u8,
    pub(crate) bounces_remaining: u8,
    pub(crate) proximity: Option<RealtimeProjectileProximityState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RealtimeProjectileProximityState {
    pub(crate) target_player_id: u16,
    pub(crate) target_life_seq: u32,
    pub(crate) sensor_end_tick: u32,
    pub(crate) highest_axis_offset: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RealtimeWatchDamageEvent {
    pub(crate) victim_player_id: u16,
    pub(crate) victim_session_id: u64,
    pub(crate) victim_life_seq: u32,
    pub(crate) shooter_player_id: u16,
    pub(crate) shooter_session_id: u64,
    pub(crate) shooter_life_seq: u32,
    pub(crate) weapon_data: u16,
    pub(crate) damage: u16,
    pub(crate) energy_before: u16,
    pub(crate) energy_after: u16,
    pub(crate) projectile_id: u32,
    pub(crate) damage_seq: u32,
    pub(crate) lethal: bool,
}

/// Exact server-owned post-Repel presentation state for one projectile or
/// Decoy. The immutable source tuple identifies the corresponding weapon
/// carrier even when the state event overtakes that carrier on RTC.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RealtimeRepelMutationState {
    /// A Repel can change velocity/deadline while authority work still holds
    /// position. Packed into entity_kind's high bit; payload size is unchanged.
    pub(crate) motion_frozen: bool,
    pub(crate) entity_kind: u8,
    pub(crate) source_player_id: u16,
    pub(crate) source_session_id: u64,
    pub(crate) source_life_seq: u32,
    pub(crate) source_server_tick: u32,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RealtimeRepelMutationBatch {
    pub(crate) count: u8,
    pub(crate) states: [RealtimeRepelMutationState; MAX_REPEL_MUTATIONS_PER_EVENT],
}

impl RealtimeRepelMutationBatch {
    pub(crate) fn from_states(states: &[RealtimeRepelMutationState]) -> Option<Self> {
        if states.is_empty() || states.len() > MAX_REPEL_MUTATIONS_PER_EVENT {
            return None;
        }
        let mut batch = Self {
            count: states.len() as u8,
            states: [RealtimeRepelMutationState::default(); MAX_REPEL_MUTATIONS_PER_EVENT],
        };
        batch.states[..states.len()].copy_from_slice(states);
        Some(batch)
    }

    pub(crate) fn entries(&self) -> &[RealtimeRepelMutationState] {
        &self.states[..usize::from(self.count)]
    }
}

/// Silent source-session cancellation through the last allocated shot group.
/// Later groups remain valid even when this carrier is delayed or reordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RealtimeOwnerWeaponCancellation {
    pub(crate) source_player_id: u16,
    pub(crate) source_session_id: u64,
    pub(crate) through_shot_group_id: u64,
    pub(crate) owner_life_seq: u32,
    pub(crate) owner_input_sequence: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RealtimeEventPayload {
    Weapon(RealtimeWeaponEvent),
    WatchDamage(RealtimeWatchDamageEvent),
    RepelMutations(RealtimeRepelMutationBatch),
    ProjectileState(RealtimeProjectileStateEvent),
    OwnerWeaponsCancelled(RealtimeOwnerWeaponCancellation),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AuthenticatedRealtimeEvent {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) event_id: u32,
    pub(crate) server_tick: u32,
    pub(crate) payload: RealtimeEventPayload,
}

impl AuthenticatedRealtimeEvent {
    pub(crate) const TYPE: u8 = REALTIME_EVENT_PACKET;

    pub(crate) fn encode(self) -> Vec<u8> {
        let (kind, capacity) = match self.payload {
            RealtimeEventPayload::Weapon(_) => (REALTIME_EVENT_WEAPON, REALTIME_WEAPON_PACKET_LEN),
            RealtimeEventPayload::WatchDamage(_) => {
                (REALTIME_EVENT_WATCH_DAMAGE, WATCH_DAMAGE_PACKET_LEN)
            }
            RealtimeEventPayload::RepelMutations(batch) => (
                REALTIME_EVENT_REPEL_MUTATIONS,
                HEADER_LEN + 1 + usize::from(batch.count) * REPEL_MUTATION_STATE_LEN,
            ),
            RealtimeEventPayload::OwnerWeaponsCancelled(_) => (
                REALTIME_EVENT_OWNER_WEAPONS_CANCELLED,
                OWNER_WEAPONS_CANCELLED_PACKET_LEN,
            ),
            RealtimeEventPayload::ProjectileState(state) => (
                REALTIME_EVENT_PROJECTILE_STATE,
                PROJECTILE_STATE_PACKET_LEN
                    + usize::from(state.source_input_sequence.is_some()) * 2,
            ),
        };
        let mut packet = Vec::with_capacity(capacity);
        packet.extend([REALTIME_EVENT_PACKET, REALTIME_EVENT_VERSION, kind]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.arena_stream_id.to_le_bytes());
        packet.extend(self.event_id.to_le_bytes());
        packet.extend(self.server_tick.to_le_bytes());
        match self.payload {
            RealtimeEventPayload::OwnerWeaponsCancelled(cancel) => {
                packet.extend(cancel.source_player_id.to_le_bytes());
                packet.extend(cancel.source_session_id.to_le_bytes());
                packet.extend(cancel.through_shot_group_id.to_le_bytes());
                packet.extend(cancel.owner_life_seq.to_le_bytes());
                packet.push(u8::from(cancel.owner_input_sequence.is_some()));
                packet.extend(cancel.owner_input_sequence.unwrap_or(0).to_le_bytes());
            }
            RealtimeEventPayload::Weapon(event) => {
                packet.extend(event.source_player_id.to_le_bytes());
                packet.extend(event.source_session_id.to_le_bytes());
                packet.extend(event.source_life_seq.to_le_bytes());
                packet.extend(event.shot_group_id.to_le_bytes());
                packet.extend(event.x.to_bits().to_le_bytes());
                packet.extend(event.y.to_bits().to_le_bytes());
                packet.extend(event.velocity_x.to_bits().to_le_bytes());
                packet.extend(event.velocity_y.to_bits().to_le_bytes());
                packet.extend(event.orientation.to_bits().to_le_bytes());
                packet.extend(event.frequency.to_le_bytes());
                packet.push(event.ship);
                packet.extend(event.weapon_data.to_le_bytes());
            }
            RealtimeEventPayload::WatchDamage(event) => {
                packet.extend(event.victim_player_id.to_le_bytes());
                packet.extend(event.victim_session_id.to_le_bytes());
                packet.extend(event.victim_life_seq.to_le_bytes());
                packet.extend(event.shooter_player_id.to_le_bytes());
                packet.extend(event.shooter_session_id.to_le_bytes());
                packet.extend(event.shooter_life_seq.to_le_bytes());
                packet.extend(event.weapon_data.to_le_bytes());
                packet.extend(event.damage.to_le_bytes());
                packet.extend(event.energy_before.to_le_bytes());
                packet.extend(event.energy_after.to_le_bytes());
                packet.extend(event.projectile_id.to_le_bytes());
                packet.extend(event.damage_seq.to_le_bytes());
                packet.push(u8::from(event.lethal));
            }
            RealtimeEventPayload::RepelMutations(batch) => {
                packet.push(batch.count);
                for state in batch.entries() {
                    packet.push(
                        state.entity_kind
                            | u8::from(state.motion_frozen) * REPEL_MUTATION_MOTION_FROZEN,
                    );
                    packet.extend(state.source_player_id.to_le_bytes());
                    packet.extend(state.source_session_id.to_le_bytes());
                    packet.extend(state.source_life_seq.to_le_bytes());
                    packet.extend(state.source_server_tick.to_le_bytes());
                    packet.extend(state.source_weapon_data.to_le_bytes());
                    packet.push(state.source_projectile_index);
                    packet.extend(state.result_weapon_data.to_le_bytes());
                    packet.extend(state.position_x.to_bits().to_le_bytes());
                    packet.extend(state.position_y.to_bits().to_le_bytes());
                    packet.extend(state.velocity_x.to_bits().to_le_bytes());
                    packet.extend(state.velocity_y.to_bits().to_le_bytes());
                    packet.extend(state.expires_at_tick.to_le_bytes());
                    packet.push(state.bounces_remaining);
                }
            }
            RealtimeEventPayload::ProjectileState(state) => {
                let source = state.source;
                packet.extend(source.source_player_id.to_le_bytes());
                packet.extend(source.source_session_id.to_le_bytes());
                packet.extend(source.source_life_seq.to_le_bytes());
                packet.extend(source.shot_group_id.to_le_bytes());
                packet.extend(source.x.to_bits().to_le_bytes());
                packet.extend(source.y.to_bits().to_le_bytes());
                packet.extend(source.velocity_x.to_bits().to_le_bytes());
                packet.extend(source.velocity_y.to_bits().to_le_bytes());
                packet.extend(source.orientation.to_bits().to_le_bytes());
                packet.extend(source.frequency.to_le_bytes());
                packet.push(source.ship);
                packet.extend(source.weapon_data.to_le_bytes());
                packet.extend(state.projectile_id.to_le_bytes());
                packet.extend(state.source_server_tick.to_le_bytes());
                packet.push(state.source_projectile_index);
                packet.extend(state.weapon_data.to_le_bytes());
                packet.extend(state.position_x.to_bits().to_le_bytes());
                packet.extend(state.position_y.to_bits().to_le_bytes());
                packet.extend(state.velocity_x.to_bits().to_le_bytes());
                packet.extend(state.velocity_y.to_bits().to_le_bytes());
                packet.extend(state.expiry_tick.unwrap_or(0).to_le_bytes());
                let flags = u8::from(state.expiry_tick.is_some()) * PROJECTILE_STATE_HAS_EXPIRY
                    | u8::from(state.proximity.is_some()) * PROJECTILE_STATE_HAS_PROXIMITY_SENSOR
                    | u8::from(state.terminal) * PROJECTILE_STATE_TERMINAL
                    | u8::from(state.motion_frozen) * PROJECTILE_STATE_MOTION_FROZEN
                    | u8::from(state.source_input_sequence.is_some())
                        * PROJECTILE_STATE_HAS_INPUT_SEQUENCE;
                packet.push(flags);
                packet.push(state.behavior);
                packet.push(state.bounces_remaining);
                let proximity = state.proximity.unwrap_or(RealtimeProjectileProximityState {
                    target_player_id: 0,
                    target_life_seq: 0,
                    sensor_end_tick: 0,
                    highest_axis_offset: 0.0,
                });
                packet.extend(proximity.target_player_id.to_le_bytes());
                packet.extend(proximity.target_life_seq.to_le_bytes());
                packet.extend(proximity.sensor_end_tick.to_le_bytes());
                packet.extend(proximity.highest_axis_offset.to_bits().to_le_bytes());
                if let Some(sequence) = state.source_input_sequence {
                    packet.extend(sequence.to_le_bytes());
                }
            }
        }
        debug_assert_eq!(packet.len(), capacity);
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(REALTIME_EVENT_PACKET) {
            return Err("unexpected realtime event packet type".to_string());
        }
        if packet.get(1).copied() != Some(REALTIME_EVENT_VERSION) {
            return Err("unsupported realtime event packet version".to_string());
        }
        let kind = *packet
            .get(2)
            .ok_or_else(|| "realtime event packet is truncated".to_string())?;
        let expected_len = match kind {
            REALTIME_EVENT_WEAPON => REALTIME_WEAPON_PACKET_LEN,
            REALTIME_EVENT_WATCH_DAMAGE => WATCH_DAMAGE_PACKET_LEN,
            REALTIME_EVENT_REPEL_MUTATIONS => {
                let count =
                    usize::from(*packet.get(HEADER_LEN).ok_or_else(|| {
                        "realtime Repel mutation packet is truncated".to_string()
                    })?);
                if count == 0 || count > MAX_REPEL_MUTATIONS_PER_EVENT {
                    return Err("realtime Repel mutation count is invalid".to_string());
                }
                HEADER_LEN + 1 + count * REPEL_MUTATION_STATE_LEN
            }
            REALTIME_EVENT_OWNER_WEAPONS_CANCELLED => OWNER_WEAPONS_CANCELLED_PACKET_LEN,
            REALTIME_EVENT_PROJECTILE_STATE => {
                let flags = *packet
                    .get(105)
                    .ok_or("realtime projectile state is truncated")?;
                PROJECTILE_STATE_PACKET_LEN
                    + usize::from(flags & PROJECTILE_STATE_HAS_INPUT_SEQUENCE != 0) * 2
            }
            _ => return Err(format!("unknown realtime event kind: {kind}")),
        };
        if packet.len() != expected_len {
            return Err(format!(
                "realtime event packet length is {}, expected {expected_len}",
                packet.len()
            ));
        }
        let server_session_id = read_u64(packet, 3)?;
        let arena_stream_id = read_u64(packet, 11)?;
        let event_id = read_u32(packet, 19)?;
        let server_tick = read_u32(packet, 23)?;
        if server_session_id == 0 || arena_stream_id == 0 || event_id == 0 {
            return Err("realtime event identity fields must be nonzero".to_string());
        }
        let payload = match kind {
            REALTIME_EVENT_OWNER_WEAPONS_CANCELLED => {
                let cancel = RealtimeOwnerWeaponCancellation {
                    source_player_id: read_u16(packet, 27)?,
                    source_session_id: read_u64(packet, 29)?,
                    through_shot_group_id: read_u64(packet, 37)?,
                    owner_life_seq: read_u32(packet, 45)?,
                    owner_input_sequence: (packet[49] & 1 != 0)
                        .then(|| read_u16(packet, 50))
                        .transpose()?,
                };
                if cancel.source_session_id == 0
                    || cancel.owner_life_seq == 0
                    || packet[49] & !1 != 0
                    || (cancel.owner_input_sequence.is_none() && read_u16(packet, 50)? != 0)
                {
                    return Err("realtime owner weapon cancellation is invalid".to_string());
                }
                RealtimeEventPayload::OwnerWeaponsCancelled(cancel)
            }
            REALTIME_EVENT_WEAPON => {
                let event = RealtimeWeaponEvent {
                    source_player_id: read_u16(packet, 27)?,
                    source_session_id: read_u64(packet, 29)?,
                    source_life_seq: read_u32(packet, 37)?,
                    shot_group_id: read_u64(packet, 41)?,
                    x: f32::from_bits(read_u32(packet, 49)?),
                    y: f32::from_bits(read_u32(packet, 53)?),
                    velocity_x: f32::from_bits(read_u32(packet, 57)?),
                    velocity_y: f32::from_bits(read_u32(packet, 61)?),
                    orientation: f32::from_bits(read_u32(packet, 65)?),
                    frequency: read_u16(packet, 69)?,
                    ship: packet[71],
                    weapon_data: read_u16(packet, 72)?,
                };
                if event.source_session_id == 0
                    || event.source_life_seq == 0
                    || event.shot_group_id == 0
                    || event.weapon_data == 0
                    || !(1..=8).contains(&(event.weapon_data as u8 & 0x1f))
                    || event.ship >= 8
                    || ![
                        event.x,
                        event.y,
                        event.velocity_x,
                        event.velocity_y,
                        event.orientation,
                    ]
                    .into_iter()
                    .all(f32::is_finite)
                    || !(0.0..=1.0).contains(&event.orientation)
                {
                    return Err("realtime weapon event payload is invalid".to_string());
                }
                RealtimeEventPayload::Weapon(event)
            }
            REALTIME_EVENT_WATCH_DAMAGE => {
                let event = RealtimeWatchDamageEvent {
                    victim_player_id: read_u16(packet, 27)?,
                    victim_session_id: read_u64(packet, 29)?,
                    victim_life_seq: read_u32(packet, 37)?,
                    shooter_player_id: read_u16(packet, 41)?,
                    shooter_session_id: read_u64(packet, 43)?,
                    shooter_life_seq: read_u32(packet, 51)?,
                    weapon_data: read_u16(packet, 55)?,
                    damage: read_u16(packet, 57)?,
                    energy_before: read_u16(packet, 59)?,
                    energy_after: read_u16(packet, 61)?,
                    projectile_id: read_u32(packet, 63)?,
                    damage_seq: read_u32(packet, 67)?,
                    lethal: packet[71] != 0,
                };
                if event.victim_session_id == 0
                    || event.victim_life_seq == 0
                    || event.shooter_session_id == 0
                    || event.shooter_life_seq == 0
                    || event.damage_seq == 0
                    || packet[71] > 1
                {
                    return Err("realtime WATCH_DAMAGE event payload is invalid".to_string());
                }
                RealtimeEventPayload::WatchDamage(event)
            }
            REALTIME_EVENT_REPEL_MUTATIONS => {
                let count = packet[HEADER_LEN];
                let mut states =
                    [RealtimeRepelMutationState::default(); MAX_REPEL_MUTATIONS_PER_EVENT];
                let mut offset = HEADER_LEN + 1;
                for state in states.iter_mut().take(usize::from(count)) {
                    *state = RealtimeRepelMutationState {
                        motion_frozen: packet[offset] & REPEL_MUTATION_MOTION_FROZEN != 0,
                        entity_kind: packet[offset] & !REPEL_MUTATION_MOTION_FROZEN,
                        source_player_id: read_u16(packet, offset + 1)?,
                        source_session_id: read_u64(packet, offset + 3)?,
                        source_life_seq: read_u32(packet, offset + 11)?,
                        source_server_tick: read_u32(packet, offset + 15)?,
                        source_weapon_data: read_u16(packet, offset + 19)?,
                        source_projectile_index: packet[offset + 21],
                        result_weapon_data: read_u16(packet, offset + 22)?,
                        position_x: f32::from_bits(read_u32(packet, offset + 24)?),
                        position_y: f32::from_bits(read_u32(packet, offset + 28)?),
                        velocity_x: f32::from_bits(read_u32(packet, offset + 32)?),
                        velocity_y: f32::from_bits(read_u32(packet, offset + 36)?),
                        expires_at_tick: read_u32(packet, offset + 40)?,
                        bounces_remaining: packet[offset + 44],
                    };
                    let source_type = state.source_weapon_data as u8 & 0x1f;
                    let result_type = state.result_weapon_data as u8 & 0x1f;
                    if !matches!(
                        state.entity_kind,
                        REPEL_MUTATION_PROJECTILE | REPEL_MUTATION_DECOY
                    ) || state.source_session_id == 0
                        || state.source_life_seq == 0
                        || !matches!(source_type, 1..=8 | 15)
                        || !matches!(result_type, 1..=8 | 15)
                        || (state.entity_kind == REPEL_MUTATION_DECOY
                            && (state.motion_frozen
                                || source_type != 6
                                || result_type != 6
                                || state.source_projectile_index != 0))
                        || ![
                            state.position_x,
                            state.position_y,
                            state.velocity_x,
                            state.velocity_y,
                        ]
                        .into_iter()
                        .all(f32::is_finite)
                    {
                        return Err("realtime Repel mutation state is invalid".to_string());
                    }
                    offset += REPEL_MUTATION_STATE_LEN;
                }
                RealtimeEventPayload::RepelMutations(RealtimeRepelMutationBatch { count, states })
            }
            REALTIME_EVENT_PROJECTILE_STATE => {
                let source = RealtimeWeaponEvent {
                    source_player_id: read_u16(packet, 27)?,
                    source_session_id: read_u64(packet, 29)?,
                    source_life_seq: read_u32(packet, 37)?,
                    shot_group_id: read_u64(packet, 41)?,
                    x: f32::from_bits(read_u32(packet, 49)?),
                    y: f32::from_bits(read_u32(packet, 53)?),
                    velocity_x: f32::from_bits(read_u32(packet, 57)?),
                    velocity_y: f32::from_bits(read_u32(packet, 61)?),
                    orientation: f32::from_bits(read_u32(packet, 65)?),
                    frequency: read_u16(packet, 69)?,
                    ship: packet[71],
                    weapon_data: read_u16(packet, 72)?,
                };
                let flags = packet[105];
                let state = RealtimeProjectileStateEvent {
                    source_input_sequence: (flags & PROJECTILE_STATE_HAS_INPUT_SEQUENCE != 0)
                        .then(|| read_u16(packet, PROJECTILE_STATE_PACKET_LEN))
                        .transpose()?,
                    terminal: flags & PROJECTILE_STATE_TERMINAL != 0,
                    motion_frozen: flags & PROJECTILE_STATE_MOTION_FROZEN != 0,
                    source,
                    projectile_id: read_u32(packet, 74)?,
                    source_server_tick: read_u32(packet, 78)?,
                    source_projectile_index: packet[82],
                    weapon_data: read_u16(packet, 83)?,
                    position_x: f32::from_bits(read_u32(packet, 85)?),
                    position_y: f32::from_bits(read_u32(packet, 89)?),
                    velocity_x: f32::from_bits(read_u32(packet, 93)?),
                    velocity_y: f32::from_bits(read_u32(packet, 97)?),
                    expiry_tick: (flags & PROJECTILE_STATE_HAS_EXPIRY != 0)
                        .then(|| read_u32(packet, 101))
                        .transpose()?,
                    behavior: packet[106],
                    bounces_remaining: packet[107],
                    proximity: (flags & PROJECTILE_STATE_HAS_PROXIMITY_SENSOR != 0).then(|| {
                        RealtimeProjectileProximityState {
                            target_player_id: read_u16(packet, 108)
                                .expect("validated packet length"),
                            target_life_seq: read_u32(packet, 110)
                                .expect("validated packet length"),
                            sensor_end_tick: read_u32(packet, 114)
                                .expect("validated packet length"),
                            highest_axis_offset: f32::from_bits(
                                read_u32(packet, 118).expect("validated packet length"),
                            ),
                        }
                    }),
                };
                let source_type = source.weapon_data as u8 & 0x1f;
                let state_type = state.weapon_data as u8 & 0x1f;
                if source.source_session_id == 0
                    || source.source_life_seq == 0
                    || source.shot_group_id == 0
                    || source.ship >= 8
                    || state.projectile_id == 0
                    || !(1..=8).contains(&source_type)
                    || (!((1..=8).contains(&state_type) || state_type == 15))
                    || (state_type != source_type && state_type != 15)
                    || flags
                        & !(PROJECTILE_STATE_HAS_EXPIRY
                            | PROJECTILE_STATE_HAS_PROXIMITY_SENSOR
                            | PROJECTILE_STATE_TERMINAL
                            | PROJECTILE_STATE_MOTION_FROZEN
                            | PROJECTILE_STATE_HAS_INPUT_SEQUENCE)
                        != 0
                    || !matches!(
                        state.behavior,
                        PROJECTILE_BEHAVIOR_STANDARD
                            | PROJECTILE_BEHAVIOR_BURST_INACTIVE
                            | PROJECTILE_BEHAVIOR_BURST_ACTIVE
                    )
                    || (state.terminal && !matches!(state_type, 1 | 2 | 3 | 4 | 7 | 8 | 15))
                    || (state.terminal && state.proximity.is_some())
                    || (state.motion_frozen
                        && (state.terminal
                            || state.expiry_tick.is_none()
                            || !matches!(state_type, 1 | 2 | 3 | 4 | 7 | 8 | 15)))
                    || (state.behavior != PROJECTILE_BEHAVIOR_STANDARD && state_type != 7)
                    || state.proximity.is_some_and(|proximity| {
                        state_type != 4
                            || proximity.target_life_seq == 0
                            || !proximity.highest_axis_offset.is_finite()
                            || proximity.highest_axis_offset < 0.0
                    })
                    || ![
                        source.x,
                        source.y,
                        source.velocity_x,
                        source.velocity_y,
                        source.orientation,
                        state.position_x,
                        state.position_y,
                        state.velocity_x,
                        state.velocity_y,
                    ]
                    .into_iter()
                    .all(f32::is_finite)
                    || !(0.0..=1.0).contains(&source.orientation)
                {
                    return Err("realtime projectile state payload is invalid".to_string());
                }
                RealtimeEventPayload::ProjectileState(state)
            }
            _ => unreachable!(),
        };
        Ok(Self {
            server_session_id,
            arena_stream_id,
            event_id,
            server_tick,
            payload,
        })
    }
}

/// `0x84` carries several `0x7d` events for one recipient in one RTC message.
/// Every event to one recipient states the same session and arena-stream ids,
/// so the batch states them once:
/// `type=0x84:u8 | version=1:u8 | count:u8 | server_session_id:u64 |
/// arena_stream_id:u64`, then per event `len:u16 | that 0x7d packet without
/// its 16 id bytes`. The browser rebuilds every exact `0x7d` packet as the
/// batch arrives (`js/zubspace_rtc_config.js`), so identity checks,
/// deduplication and packet logs see the same bytes as unbatched delivery.
pub(crate) const REALTIME_EVENT_BATCH_PACKET: u8 = 0x84;
pub(crate) const REALTIME_EVENT_BATCH_VERSION: u8 = 1;
const REALTIME_EVENT_BATCH_HEADER_LEN: usize = 1 + 1 + 1 + 8 + 8;
const REALTIME_EVENT_IDS: std::ops::Range<usize> = 3..19;
/// Below the 1,163-byte DATA payload of one default 1,191-byte SCTP packet, so
/// a batch never spans datagrams and one lost datagram costs one batch.
pub(crate) const MAX_REALTIME_EVENT_BATCH_LEN: usize = 1_100;
// The largest event fits a batch with room to spare.
const _: () = assert!(
    REALTIME_EVENT_BATCH_HEADER_LEN + 2 + MAX_REALTIME_EVENT_PACKET_LEN
        <= MAX_REALTIME_EVENT_BATCH_LEN
);

/// Packs runs of consecutive `0x7d` packets into `0x84` batches, keeping their
/// order. Any other packet passes through and ends the run. A batch also ends
/// at the size cap, at 255 events or when the ids change; a batch of one event
/// goes out as its plain `0x7d` packet, which is smaller.
pub(crate) fn batch_realtime_events(packets: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut output = Vec::with_capacity(packets.len());
    let mut run: Vec<Vec<u8>> = Vec::new();
    let mut run_len = REALTIME_EVENT_BATCH_HEADER_LEN;
    for packet in packets {
        if packet.first() != Some(&REALTIME_EVENT_PACKET) || packet.len() < HEADER_LEN {
            flush_realtime_event_batch(&mut run, &mut output);
            run_len = REALTIME_EVENT_BATCH_HEADER_LEN;
            output.push(packet);
            continue;
        }
        let entry_len = 2 + packet.len() - REALTIME_EVENT_IDS.len();
        let same_ids = run
            .first()
            .is_none_or(|first| first[REALTIME_EVENT_IDS] == packet[REALTIME_EVENT_IDS]);
        if !same_ids
            || run_len + entry_len > MAX_REALTIME_EVENT_BATCH_LEN
            || run.len() == usize::from(u8::MAX)
        {
            flush_realtime_event_batch(&mut run, &mut output);
            run_len = REALTIME_EVENT_BATCH_HEADER_LEN;
        }
        run_len += entry_len;
        run.push(packet);
    }
    flush_realtime_event_batch(&mut run, &mut output);
    output
}

fn flush_realtime_event_batch(run: &mut Vec<Vec<u8>>, output: &mut Vec<Vec<u8>>) {
    if run.len() < 2 {
        output.append(run);
        return;
    }
    let mut batch = Vec::with_capacity(MAX_REALTIME_EVENT_BATCH_LEN);
    batch.extend([
        REALTIME_EVENT_BATCH_PACKET,
        REALTIME_EVENT_BATCH_VERSION,
        run.len() as u8,
    ]);
    batch.extend_from_slice(&run[0][REALTIME_EVENT_IDS]);
    for event in run.drain(..) {
        batch.extend(((event.len() - REALTIME_EVENT_IDS.len()) as u16).to_le_bytes());
        batch.extend_from_slice(&event[..REALTIME_EVENT_IDS.start]);
        batch.extend_from_slice(&event[REALTIME_EVENT_IDS.end..]);
    }
    output.push(batch);
}

/// Rebuilds the exact `0x7d` packets of a `0x84` batch: the inverse of
/// [`batch_realtime_events`], mirrored by the browser's
/// `zubspaceExpandRealtimeEventBatch`.
pub(crate) fn expand_realtime_event_batch(batch: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if batch.first() != Some(&REALTIME_EVENT_BATCH_PACKET) {
        return Err("unexpected realtime event batch type".to_string());
    }
    if batch.len() < REALTIME_EVENT_BATCH_HEADER_LEN
        || batch[1] != REALTIME_EVENT_BATCH_VERSION
        || batch[2] < 2
    {
        return Err("realtime event batch header is invalid".to_string());
    }
    let ids = &batch[REALTIME_EVENT_IDS];
    let mut events = Vec::with_capacity(usize::from(batch[2]));
    let mut offset = REALTIME_EVENT_BATCH_HEADER_LEN;
    for _ in 0..batch[2] {
        let len = usize::from(read_u16(batch, offset)?);
        offset += 2;
        let entry = batch
            .get(offset..offset + len)
            .filter(|entry| {
                entry.len() >= REALTIME_EVENT_IDS.start && entry[0] == REALTIME_EVENT_PACKET
            })
            .ok_or("realtime event batch entry is invalid")?;
        offset += len;
        let mut event = Vec::with_capacity(len + ids.len());
        event.extend_from_slice(&entry[..REALTIME_EVENT_IDS.start]);
        event.extend_from_slice(ids);
        event.extend_from_slice(&entry[REALTIME_EVENT_IDS.start..]);
        events.push(event);
    }
    if offset != batch.len() {
        return Err("realtime event batch has trailing bytes".to_string());
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weapon_event(arena_stream_id: u64, event_id: u32) -> Vec<u8> {
        AuthenticatedRealtimeEvent {
            server_session_id: 0x0102_0304_0506_0708,
            arena_stream_id,
            event_id,
            server_tick: 1_000 + event_id,
            payload: RealtimeEventPayload::Weapon(RealtimeWeaponEvent {
                source_player_id: 4,
                source_session_id: 8,
                source_life_seq: 2,
                shot_group_id: u64::from(event_id),
                x: 100.25,
                y: 200.5,
                velocity_x: -3.0,
                velocity_y: 4.0,
                orientation: 0.25,
                frequency: 12,
                ship: 7,
                weapon_data: 0x44,
            }),
        }
        .encode()
    }

    #[test]
    fn a_ticks_events_share_one_batch_that_expands_to_the_exact_packets() {
        let events: Vec<_> = (1..=3).map(|id| weapon_event(9, id)).collect();
        let snapshot = vec![0x70, 1, 2, 3];
        let mut packets = events.clone();
        packets.push(snapshot.clone());

        let sent = batch_realtime_events(packets);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1], snapshot);
        let batch = &sent[0];
        assert_eq!(batch[..3], [REALTIME_EVENT_BATCH_PACKET, 1, 3]);
        // The ids are stated once; each entry is 14 bytes smaller than its
        // packet alone.
        assert_eq!(
            batch.len(),
            REALTIME_EVENT_BATCH_HEADER_LEN + 3 * (2 + REALTIME_WEAPON_PACKET_LEN - 16)
        );
        assert_eq!(expand_realtime_event_batch(batch), Ok(events));
    }

    #[test]
    fn single_events_other_streams_and_full_batches_start_new_messages() {
        // One event alone stays a plain 0x7d packet.
        assert_eq!(
            batch_realtime_events(vec![weapon_event(9, 1)]),
            vec![weapon_event(9, 1)]
        );
        // A non-event packet ends the run.
        let sent = batch_realtime_events(vec![
            weapon_event(9, 1),
            vec![0x00, 0x04, 1, 0, 0, 0],
            weapon_event(9, 2),
        ]);
        assert_eq!(sent[0], weapon_event(9, 1));
        assert_eq!(sent[2], weapon_event(9, 2));
        // A new arena stream never shares a batch with the old one.
        let sent = batch_realtime_events(vec![
            weapon_event(9, 1),
            weapon_event(9, 2),
            weapon_event(10, 3),
            weapon_event(10, 4),
        ]);
        assert_eq!(sent.len(), 2);
        assert_eq!(
            expand_realtime_event_batch(&sent[1]),
            Ok(vec![weapon_event(10, 3), weapon_event(10, 4)])
        );
        // The size cap splits a long run and keeps the order.
        let events: Vec<_> = (1..=40).map(|id| weapon_event(9, id)).collect();
        let sent = batch_realtime_events(events.clone());
        assert!(sent.len() > 1);
        assert!(
            sent.iter()
                .all(|batch| batch.len() <= MAX_REALTIME_EVENT_BATCH_LEN)
        );
        let expanded: Vec<_> = sent
            .iter()
            .flat_map(|batch| expand_realtime_event_batch(batch).unwrap())
            .collect();
        assert_eq!(expanded, events);
    }

    /// The browser's `zubspaceExpandRealtimeEventBatch` is tested against the
    /// same bytes (`tests/rtc_config.test.mjs`).
    #[test]
    fn the_batch_fixture_shared_with_the_browser_test_is_current() {
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let cancellation = AuthenticatedRealtimeEvent {
            server_session_id: 0x0102_0304_0506_0708,
            arena_stream_id: 9,
            event_id: 3,
            server_tick: 1_003,
            payload: RealtimeEventPayload::OwnerWeaponsCancelled(RealtimeOwnerWeaponCancellation {
                source_player_id: 5,
                source_session_id: 6,
                through_shot_group_id: 7,
                owner_life_seq: 8,
                owner_input_sequence: Some(9),
            }),
        }
        .encode();
        let events = vec![weapon_event(9, 1), weapon_event(9, 2), cancellation];
        let batch = batch_realtime_events(events.clone()).remove(0);
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/realtime_event_batch.json"
        ))
        .unwrap();
        let expected = serde_json::json!({
            "batch": hex(&batch),
            "events": events.iter().map(|event| hex(event)).collect::<Vec<_>>(),
        });
        assert_eq!(
            fixture, expected,
            "regenerate tests/fixtures/realtime_event_batch.json"
        );
    }

    #[test]
    fn malformed_batches_are_rejected() {
        let batch = batch_realtime_events(vec![weapon_event(9, 1), weapon_event(9, 2)]).remove(0);
        assert!(expand_realtime_event_batch(&batch[..batch.len() - 1]).is_err());
        let mut trailing = batch.clone();
        trailing.push(0);
        assert!(expand_realtime_event_batch(&trailing).is_err());
        let mut version = batch.clone();
        version[1] = 2;
        assert!(expand_realtime_event_batch(&version).is_err());
        let mut inner_type = batch.clone();
        inner_type[REALTIME_EVENT_BATCH_HEADER_LEN + 2] = 0x70;
        assert!(expand_realtime_event_batch(&inner_type).is_err());
        let mut single = batch;
        single[2] = 1;
        assert!(expand_realtime_event_batch(&single).is_err());
    }

    #[test]
    fn authenticated_realtime_events_round_trip_and_require_exact_framing() {
        let event = AuthenticatedRealtimeEvent {
            server_session_id: 7,
            arena_stream_id: u64::MAX,
            event_id: u32::MAX,
            server_tick: 99,
            payload: RealtimeEventPayload::Weapon(RealtimeWeaponEvent {
                source_player_id: 4,
                source_session_id: 8,
                source_life_seq: u32::MAX,
                shot_group_id: u64::MAX,
                x: 100.25,
                y: 200.5,
                velocity_x: -3.0,
                velocity_y: 4.0,
                orientation: 1.0,
                frequency: 12,
                ship: 7,
                weapon_data: 0x44,
            }),
        };
        let encoded = event.encode();
        assert_eq!(AuthenticatedRealtimeEvent::parse(&encoded), Ok(event));
        assert!(AuthenticatedRealtimeEvent::parse(&encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded;
        trailing.push(0);
        assert!(AuthenticatedRealtimeEvent::parse(&trailing).is_err());
    }

    #[test]
    fn realtime_weapon_rejects_unknown_weapon_types_before_publication() {
        let event = AuthenticatedRealtimeEvent {
            server_session_id: 7,
            arena_stream_id: 8,
            event_id: 9,
            server_tick: 10,
            payload: RealtimeEventPayload::Weapon(RealtimeWeaponEvent {
                source_player_id: 4,
                source_session_id: 11,
                source_life_seq: 12,
                shot_group_id: 13,
                x: 100.0,
                y: 200.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: 0.5,
                frequency: 7,
                ship: 0,
                weapon_data: 9,
            }),
        };

        assert!(AuthenticatedRealtimeEvent::parse(&event.encode()).is_err());
    }

    #[test]
    fn authoritative_repel_mutation_batches_round_trip_with_exact_state() {
        let states = [
            RealtimeRepelMutationState {
                motion_frozen: false,
                entity_kind: REPEL_MUTATION_PROJECTILE,
                source_player_id: 4,
                source_session_id: 11,
                source_life_seq: 12,
                source_server_tick: u32::MAX,
                source_weapon_data: 8,
                source_projectile_index: 3,
                result_weapon_data: 8,
                position_x: 101.25,
                position_y: 202.5,
                velocity_x: -300.5,
                velocity_y: 400.25,
                expires_at_tick: 77,
                bounces_remaining: 2,
            },
            RealtimeRepelMutationState {
                motion_frozen: false,
                entity_kind: REPEL_MUTATION_DECOY,
                source_player_id: 5,
                source_session_id: 13,
                source_life_seq: 14,
                source_server_tick: 15,
                source_weapon_data: 6,
                source_projectile_index: 0,
                result_weapon_data: 6,
                position_x: 303.75,
                position_y: 404.5,
                velocity_x: 500.0,
                velocity_y: 0.0,
                expires_at_tick: u32::MAX,
                bounces_remaining: 0,
            },
        ];
        let event = AuthenticatedRealtimeEvent {
            server_session_id: 7,
            arena_stream_id: 8,
            event_id: 9,
            server_tick: 10,
            payload: RealtimeEventPayload::RepelMutations(
                RealtimeRepelMutationBatch::from_states(&states).unwrap(),
            ),
        };

        let encoded = event.encode();
        assert_eq!(AuthenticatedRealtimeEvent::parse(&encoded), Ok(event));
        assert!(AuthenticatedRealtimeEvent::parse(&encoded[..encoded.len() - 1]).is_err());

        let mut held_states = states;
        held_states[0].motion_frozen = true;
        let held = AuthenticatedRealtimeEvent {
            payload: RealtimeEventPayload::RepelMutations(
                RealtimeRepelMutationBatch::from_states(&held_states).unwrap(),
            ),
            ..event
        };
        let held_bytes = held.encode();
        assert_eq!(
            held_bytes.len(),
            encoded.len(),
            "capacity pause does not grow mutation packets"
        );
        assert_eq!(
            held_bytes[HEADER_LEN + 1],
            REPEL_MUTATION_PROJECTILE | REPEL_MUTATION_MOTION_FROZEN
        );
        assert_eq!(AuthenticatedRealtimeEvent::parse(&held_bytes), Ok(held));
        let mut invalid_decoy = held_bytes;
        invalid_decoy[HEADER_LEN + 1 + REPEL_MUTATION_STATE_LEN] |= REPEL_MUTATION_MOTION_FROZEN;
        assert!(
            AuthenticatedRealtimeEvent::parse(&invalid_decoy).is_err(),
            "Decoys cannot claim a projectile pause"
        );
        let mut unknown_flags = encoded.clone();
        unknown_flags[HEADER_LEN + 1] |= 0x40;
        assert!(AuthenticatedRealtimeEvent::parse(&unknown_flags).is_err());

        let mut invalid_count = encoded;
        invalid_count[HEADER_LEN] = 0;
        assert!(AuthenticatedRealtimeEvent::parse(&invalid_count).is_err());
    }

    #[test]
    fn active_projectile_state_is_self_contained_exact_and_strict() {
        let state = RealtimeProjectileStateEvent {
            source_input_sequence: None,
            terminal: false,
            motion_frozen: false,
            source: RealtimeWeaponEvent {
                source_player_id: 4,
                source_session_id: 11,
                source_life_seq: 12,
                shot_group_id: 13,
                x: 100.25,
                y: 200.5,
                velocity_x: -3.0,
                velocity_y: 4.0,
                orientation: 1.0,
                frequency: 12,
                ship: 7,
                weapon_data: 0x44,
            },
            projectile_id: u32::MAX,
            source_server_tick: u32::MAX - 1,
            source_projectile_index: 3,
            weapon_data: 0x44,
            position_x: 120.25,
            position_y: 240.5,
            velocity_x: -30.0,
            velocity_y: 40.0,
            expiry_tick: Some(0),
            behavior: PROJECTILE_BEHAVIOR_STANDARD,
            bounces_remaining: 2,
            proximity: Some(RealtimeProjectileProximityState {
                target_player_id: 5,
                target_life_seq: 6,
                sensor_end_tick: 7,
                highest_axis_offset: 80.5,
            }),
        };
        let event = AuthenticatedRealtimeEvent {
            server_session_id: 7,
            arena_stream_id: 8,
            event_id: 9,
            server_tick: 10,
            payload: RealtimeEventPayload::ProjectileState(state),
        };
        let encoded = event.encode();
        assert_eq!(encoded.len(), PROJECTILE_STATE_PACKET_LEN);
        assert_eq!(AuthenticatedRealtimeEvent::parse(&encoded), Ok(event));
        for sequence in [0, u16::MAX] {
            let correlated = AuthenticatedRealtimeEvent {
                payload: RealtimeEventPayload::ProjectileState(RealtimeProjectileStateEvent {
                    source_input_sequence: Some(sequence),
                    ..state
                }),
                ..event
            };
            let bytes = correlated.encode();
            assert_eq!(bytes.len(), PROJECTILE_STATE_PACKET_LEN + 2);
            assert_eq!(AuthenticatedRealtimeEvent::parse(&bytes), Ok(correlated));
            assert!(AuthenticatedRealtimeEvent::parse(&bytes[..bytes.len() - 1]).is_err());
            let mut unflagged = bytes;
            unflagged[105] &= !PROJECTILE_STATE_HAS_INPUT_SEQUENCE;
            assert!(AuthenticatedRealtimeEvent::parse(&unflagged).is_err());
        }
        assert!(AuthenticatedRealtimeEvent::parse(&encoded[..encoded.len() - 1]).is_err());

        let mut terminal_state = state;
        terminal_state.terminal = true;
        terminal_state.source.weapon_data = 2;
        terminal_state.weapon_data = 2;
        terminal_state.proximity = None;
        let terminal = AuthenticatedRealtimeEvent {
            payload: RealtimeEventPayload::ProjectileState(terminal_state),
            ..event
        };
        assert_eq!(
            AuthenticatedRealtimeEvent::parse(&terminal.encode()),
            Ok(terminal)
        );
        let mut invalid_terminal = encoded.clone();
        invalid_terminal[105] |= PROJECTILE_STATE_TERMINAL;
        assert!(
            AuthenticatedRealtimeEvent::parse(&invalid_terminal).is_err(),
            "terminal states cannot retain a live proximity sensor"
        );
        for weapon_type in [3, 4, 8] {
            let mut explosive = terminal_state;
            explosive.source.weapon_data = weapon_type;
            explosive.weapon_data = weapon_type;
            let explosive = AuthenticatedRealtimeEvent {
                payload: RealtimeEventPayload::ProjectileState(explosive),
                ..event
            };
            assert_eq!(
                AuthenticatedRealtimeEvent::parse(&explosive.encode()),
                Ok(explosive)
            );
        }
        for kind in [1, 2, 3, 4, 7, 8, 15] {
            let mut held = state;
            held.source.weapon_data = if kind == 15 { 3 } else { kind };
            held.weapon_data = kind;
            held.proximity = None;
            held.motion_frozen = true;
            held.behavior = if kind == 7 {
                PROJECTILE_BEHAVIOR_BURST_ACTIVE
            } else {
                PROJECTILE_BEHAVIOR_STANDARD
            };
            let frozen = AuthenticatedRealtimeEvent {
                payload: RealtimeEventPayload::ProjectileState(held),
                ..event
            };
            assert_eq!(
                AuthenticatedRealtimeEvent::parse(&frozen.encode()),
                Ok(frozen)
            );
            assert_eq!(
                frozen.encode().len(),
                PROJECTILE_STATE_PACKET_LEN,
                "hold uses a flag, not a new unbounded payload"
            );
            let mut invalid = held;
            invalid.terminal = true;
            assert!(
                AuthenticatedRealtimeEvent::parse(
                    &AuthenticatedRealtimeEvent {
                        payload: RealtimeEventPayload::ProjectileState(invalid),
                        ..event
                    }
                    .encode()
                )
                .is_err()
            );
            invalid = held;
            invalid.expiry_tick = None;
            assert!(
                AuthenticatedRealtimeEvent::parse(
                    &AuthenticatedRealtimeEvent {
                        payload: RealtimeEventPayload::ProjectileState(invalid),
                        ..event
                    }
                    .encode()
                )
                .is_err()
            );
        }
        let mut held_decoy = state;
        held_decoy.source.weapon_data = 6;
        held_decoy.weapon_data = 6;
        held_decoy.proximity = None;
        held_decoy.motion_frozen = true;
        assert!(
            AuthenticatedRealtimeEvent::parse(
                &AuthenticatedRealtimeEvent {
                    payload: RealtimeEventPayload::ProjectileState(held_decoy),
                    ..event
                }
                .encode()
            )
            .is_err()
        );

        let mut unknown_flag = terminal.encode();
        unknown_flag[105] |= 0x80;
        assert!(AuthenticatedRealtimeEvent::parse(&unknown_flag).is_err());

        let mut invalid_behavior = encoded.clone();
        invalid_behavior[106] = 9;
        assert!(AuthenticatedRealtimeEvent::parse(&invalid_behavior).is_err());
        let mut invalid_group = encoded;
        invalid_group[41..49].copy_from_slice(&0_u64.to_le_bytes());
        assert!(AuthenticatedRealtimeEvent::parse(&invalid_group).is_err());
    }
    #[test]
    fn owner_weapon_cancellation_roundtrips_exact_scope_and_cutoffs() {
        for sequence in [None, Some(0), Some(u16::MAX)] {
            let event = AuthenticatedRealtimeEvent {
                server_session_id: 7,
                arena_stream_id: 8,
                event_id: 9,
                server_tick: 10,
                payload: RealtimeEventPayload::OwnerWeaponsCancelled(
                    RealtimeOwnerWeaponCancellation {
                        source_player_id: u16::MAX,
                        source_session_id: u64::MAX,
                        through_shot_group_id: u64::MAX,
                        owner_life_seq: u32::MAX,
                        owner_input_sequence: sequence,
                    },
                ),
            };
            let bytes = event.encode();
            assert_eq!(bytes.len(), OWNER_WEAPONS_CANCELLED_PACKET_LEN);
            assert_eq!(AuthenticatedRealtimeEvent::parse(&bytes), Ok(event));
            assert!(AuthenticatedRealtimeEvent::parse(&bytes[..bytes.len() - 1]).is_err());
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(AuthenticatedRealtimeEvent::parse(&trailing).is_err());
            let mut invalid = bytes.clone();
            invalid[49] |= 2;
            assert!(AuthenticatedRealtimeEvent::parse(&invalid).is_err());
            let mut invalid = bytes.clone();
            invalid[29..37].fill(0);
            assert!(AuthenticatedRealtimeEvent::parse(&invalid).is_err());
            let mut invalid = bytes;
            invalid[45..49].fill(0);
            assert!(AuthenticatedRealtimeEvent::parse(&invalid).is_err());
        }
    }

    #[test]
    fn realtime_fragment_repel_wire_preserves_shrapnel_identity() {
        for motion_frozen in [false, true] {
            let state = RealtimeRepelMutationState {
                motion_frozen,
                entity_kind: REPEL_MUTATION_PROJECTILE,
                source_player_id: 4,
                source_session_id: 11,
                source_life_seq: 12,
                source_server_tick: 100,
                source_weapon_data: 15 | (2 << 5),
                source_projectile_index: 17,
                result_weapon_data: 15 | (2 << 5),
                position_x: 100.0,
                position_y: 200.0,
                velocity_x: 50.0,
                velocity_y: -20.0,
                expires_at_tick: 1000,
                bounces_remaining: 0,
            };
            let event = AuthenticatedRealtimeEvent {
                server_session_id: 7,
                arena_stream_id: 8,
                event_id: 9,
                server_tick: 160,
                payload: RealtimeEventPayload::RepelMutations(
                    RealtimeRepelMutationBatch::from_states(&[state]).unwrap(),
                ),
            };
            assert_eq!(
                AuthenticatedRealtimeEvent::parse(&event.encode()),
                Ok(event),
                "a real shrapnel Repel must survive the codec, not only a direct Game method call"
            );
            for invalid in [0, 9, 14, 16, 31] {
                for source_field in [false, true] {
                    let mut bad = state;
                    if source_field {
                        bad.source_weapon_data = invalid;
                    } else {
                        bad.result_weapon_data = invalid;
                    }
                    let malformed = AuthenticatedRealtimeEvent {
                        payload: RealtimeEventPayload::RepelMutations(
                            RealtimeRepelMutationBatch::from_states(&[bad]).unwrap(),
                        ),
                        ..event
                    };
                    assert!(AuthenticatedRealtimeEvent::parse(&malformed.encode()).is_err());
                }
            }
        }
    }
}
