// This file is included by both the library/client protocol tree and the
// standalone server binary. Keep it independent of either crate root.
#![allow(dead_code)]

//! Server → client snapshot frame (zubspace extension, type `0x70`).
//!
//! Per-recipient AOI snapshots replace the legacy per-tick relay broadcasts. Each
//! snapshot is a self-describing frame on the unreliable channel, ack'd
//! over the reliable channel; the client diffs consecutive snapshots'
//! entity lists to derive AOI enter/leave transitions instead of
//! consuming a parallel event stream.
//!
//! The wire layout is documented in `docs/protocol_compatibility.md` and the
//! behaviour (bases, AOI tiers, budget) in `docs/networking.md`:
//!
//! ```text
//! type:u8 = 0x70
//! flags:u8                  // bit 0 = FULL; bits 1/2/3 = required source/motion/relocation revisions
//! snapshot_id:u16          // monotonic per (server, recipient)
//! base_id:u16              // ack id this delta is relative to
//! tick:u32                 // server arena tick (31-bit masked)
//! arena_stream_id:u64      // reliable-bootstrap arena generation
//! near_count:u8            // entities with full-state delta
//! mid_count:u8             // entities with position-only payload
//! event_count:u8           // snapshot-scoped events
//! near_entities[near_count]
//! mid_entities[mid_count]
//! events[event_count]
//! ```
//!
//! Entity entries use a fixed-width field mask followed by the absolute values
//! selected by that mask. Encoding, decoding, delta generation, and delta
//! application live here so both endpoints use the same base semantics.

use crate::combat_timeline::PresentationSampleTime;
use std::collections::BTreeMap;

pub(crate) const SNAPSHOT_PACKET: u8 = 0x70;
pub(crate) const SNAPSHOT_ACK_PACKET: u8 = 0x71;
pub(crate) const SNAPSHOT_FULL_REQUEST_PACKET: u8 = 0x73;
pub(crate) const SNAPSHOT_HZ: u16 = 30;
pub(crate) const NEAR_SNAPSHOT_HZ: u16 = 60;
pub(crate) const SNAPSHOT_HISTORY_CAPACITY: usize = 32;
pub(crate) const SNAPSHOT_ACK_INTERVAL: u8 = 5;
/// Maximum number of advancing snapshot ids for which a client retains an
/// actor that is temporarily absent from snapshot membership. This is shared
/// with the server so load shedding cannot suppress the MID tier longer than
/// the render-proxy liveness contract permits.
pub(crate) const AOI_PROXY_LIFETIME_SNAPSHOTS: u16 = 12;
/// Local monotonic cooldown for missing-base recovery requests. Recovery
/// pacing is transport-local and must not inherit the wrapped gameplay clock.
pub(crate) const SNAPSHOT_FULL_REQUEST_COOLDOWN_MICROS: u64 = 600_000;
pub(crate) const SNAPSHOT_FLAG_FULL: u8 = 1 << 0;
/// Mandatory wire revision: near entities carry exact source-sample time.
/// Kept out of the in-memory logical FULL flags after parsing.
const SNAPSHOT_FLAG_SOURCE_TIME: u8 = 1 << 1;
/// Mandatory revision: near entities persist their motion discontinuity generation.
const SNAPSHOT_FLAG_MOTION_SEGMENT: u8 = 1 << 2;
/// Motion fields carry both the physical and last relocation generation.
const SNAPSHOT_FLAG_RELOCATION_GENERATION: u8 = 1 << 3;
const SNAPSHOT_WIRE_REVISIONS: u8 =
    SNAPSHOT_FLAG_SOURCE_TIME | SNAPSHOT_FLAG_MOTION_SEGMENT | SNAPSHOT_FLAG_RELOCATION_GENERATION;
/// Entity sample times are one byte relative to the header tick (see
/// [`SnapshotEntity::encode_into_at`]). Only sent to clients whose arena login
/// declares [`CLIENT_PROTOCOL_COMPACT_SAMPLE_TIME`]; older clients reject
/// unknown flags.
pub(crate) const SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME: u8 = 1 << 4;
/// Private state is compact: a lifecycle state without Repel delivery uses
/// its 13-byte form. Only sent to clients that declared
/// [`CLIENT_PROTOCOL_COMPACT_PRIVATE`], which also accept
/// [`SnapshotEvent::PrizeReference`].
pub(crate) const SNAPSHOT_FLAG_COMPACT_PRIVATE: u8 = 1 << 5;
const SNAPSHOT_KNOWN_FLAGS: u8 = SNAPSHOT_FLAG_FULL
    | SNAPSHOT_WIRE_REVISIONS
    | SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME
    | SNAPSHOT_FLAG_COMPACT_PRIVATE;
/// Client protocol revision, appended to the arena login, from which the
/// server may send [`SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME`] frames.
pub(crate) const CLIENT_PROTOCOL_COMPACT_SAMPLE_TIME: u8 = 1;
/// From this revision the server may send [`SNAPSHOT_FLAG_COMPACT_PRIVATE`]
/// frames and prize references.
pub(crate) const CLIENT_PROTOCOL_COMPACT_PRIVATE: u8 = 2;
/// From this revision entries may carry [`SNAPSHOT_FIELD_POSITION_OFFSET`].
pub(crate) const CLIENT_PROTOCOL_POSITION_OFFSETS: u8 = 3;
/// A compact sample time is `ticks_behind * 3 + third` for a sample up to 84
/// ticks (1.4 s) behind the frame tick. This byte instead introduces the full
/// five-byte form, for any other sample.
const COMPACT_SAMPLE_TIME_FULL: u8 = 0xff;

/// The one-byte form of `sample` relative to the frame tick, if it has one.
fn compact_sample_time(frame_tick: u32, sample: PresentationSampleTime) -> Option<u8> {
    let (tick, third) = sample.parts();
    let behind = crate::continuum::continuum_tick_diff(frame_tick, tick);
    u8::try_from(behind)
        .ok()
        .and_then(|behind| behind.checked_mul(3)?.checked_add(third))
        .filter(|&byte| byte != COMPACT_SAMPLE_TIME_FULL)
}

pub(crate) const SNAPSHOT_FIELD_X: u16 = 1 << 0;
pub(crate) const SNAPSHOT_FIELD_Y: u16 = 1 << 1;
pub(crate) const SNAPSHOT_FIELD_VELOCITY_X: u16 = 1 << 2;
pub(crate) const SNAPSHOT_FIELD_VELOCITY_Y: u16 = 1 << 3;
pub(crate) const SNAPSHOT_FIELD_DIRECTION: u16 = 1 << 4;
pub(crate) const SNAPSHOT_FIELD_ENERGY: u16 = 1 << 5;
pub(crate) const SNAPSHOT_FIELD_WEAPON: u16 = 1 << 6;
pub(crate) const SNAPSHOT_FIELD_STATUS: u16 = 1 << 7;
pub(crate) const SNAPSHOT_FIELD_BOUNTY: u16 = 1 << 8;
pub(crate) const SNAPSHOT_FIELD_PING: u16 = 1 << 9;
pub(crate) const SNAPSHOT_FIELD_FLAG_TIMER: u16 = 1 << 10;
pub(crate) const SNAPSHOT_FIELD_LIFE_SEQ: u16 = 1 << 11;
pub(crate) const SNAPSHOT_FIELD_SAMPLE_TIME: u16 = 1 << 12;
pub(crate) const SNAPSHOT_FIELD_MOTION_SEGMENT: u16 = 1 << 13;
/// The entry's x and y (whichever are present) are signed bytes relative to
/// its state in the acknowledged base, not absolute u16s. Only in delta frames,
/// for clients that declared [`CLIENT_PROTOCOL_POSITION_OFFSETS`].
pub(crate) const SNAPSHOT_FIELD_POSITION_OFFSET: u16 = 1 << 14;
const SNAPSHOT_NEAR_COMPLETE_MASK: u16 = SNAPSHOT_FIELD_X
    | SNAPSHOT_FIELD_Y
    | SNAPSHOT_FIELD_VELOCITY_X
    | SNAPSHOT_FIELD_VELOCITY_Y
    | SNAPSHOT_FIELD_DIRECTION
    | SNAPSHOT_FIELD_ENERGY
    | SNAPSHOT_FIELD_WEAPON
    | SNAPSHOT_FIELD_STATUS
    | SNAPSHOT_FIELD_BOUNTY
    | SNAPSHOT_FIELD_PING
    | SNAPSHOT_FIELD_FLAG_TIMER
    | SNAPSHOT_FIELD_LIFE_SEQ
    | SNAPSHOT_FIELD_SAMPLE_TIME
    | SNAPSHOT_FIELD_MOTION_SEGMENT;

pub(crate) fn snapshot_id_is_forward(candidate: u16, previous: u16) -> bool {
    let delta = candidate.wrapping_sub(previous);
    delta != 0 && delta < (1_u16 << 15)
}

pub(crate) fn next_snapshot_id(current: u16) -> u16 {
    match current.wrapping_add(1) {
        0 => 1,
        next => next,
    }
}

fn push_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn read_u16(packet: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = packet
        .get(offset..offset + 2)
        .ok_or_else(|| format!("u16 field truncated at offset {offset}"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(packet: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = packet
        .get(offset..offset + 4)
        .ok_or_else(|| format!("u32 field truncated at offset {offset}"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(packet: &[u8], offset: usize) -> Result<u64, String> {
    let bytes = packet
        .get(offset..offset + 8)
        .ok_or_else(|| format!("u64 field truncated at offset {offset}"))?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("checked u64 slice"),
    ))
}

/// Client → server snapshot acknowledgement (zubspace extension,
/// type `0x71`). Lets the server know the latest `snapshot_id` the
/// client has applied so it can compute deltas against a known base
/// (Phase 3e-4b). Three bytes total: `type:u8 | snapshot_id:u16`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotAckPacket {
    pub(crate) snapshot_id: u16,
}

impl SnapshotAckPacket {
    pub(crate) const TYPE: u8 = SNAPSHOT_ACK_PACKET;
    pub(crate) const PACKET_LEN: usize = 3;

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::PACKET_LEN);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.snapshot_id);
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "snapshot ack too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!("unexpected snapshot ack type: {:#04x}", packet[0]));
        }
        let snapshot_id = read_u16(packet, 1)?;
        Ok(Self { snapshot_id })
    }
}

/// Client → server request for an immediate full snapshot. The request is
/// carried inside the reliable wrapper and is sent only after a delta refers
/// to a base the client no longer retains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotFullRequestPacket;

impl SnapshotFullRequestPacket {
    pub(crate) const TYPE: u8 = SNAPSHOT_FULL_REQUEST_PACKET;
    pub(crate) const PACKET_LEN: usize = 1;

    pub(crate) fn encode(self) -> Vec<u8> {
        vec![Self::TYPE]
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet != [Self::TYPE] {
            return Err("invalid snapshot full request".to_string());
        }
        Ok(Self)
    }
}

/// Explicit projectile-pose refresh after a suspended renderer. An extended
/// 0x73 request keeps missing-snapshot-base recovery's one-byte form unchanged.
/// The reliable authenticated session plus both generations prevent an old
/// arena request from refreshing a replacement stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectileRefreshRequestPacket {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
}

impl ProjectileRefreshRequestPacket {
    pub(crate) const PACKET_LEN: usize = 18;
    pub(crate) fn encode(self) -> Vec<u8> {
        let mut bytes = vec![SNAPSHOT_FULL_REQUEST_PACKET, 1];
        bytes.extend(self.server_session_id.to_le_bytes());
        bytes.extend(self.arena_stream_id.to_le_bytes());
        bytes
    }
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::PACKET_LEN || bytes[..2] != [SNAPSHOT_FULL_REQUEST_PACKET, 1] {
            return None;
        }
        let request = Self {
            server_session_id: u64::from_le_bytes(bytes[2..10].try_into().ok()?),
            arena_stream_id: u64::from_le_bytes(bytes[10..18].try_into().ok()?),
        };
        (request.server_session_id != 0 && request.arena_stream_id != 0).then_some(request)
    }
}

/// Decoded snapshot frame metadata. Fixed 21 bytes on the wire; the
/// counts here describe how many bytes of body follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotHeader {
    pub(crate) flags: u8,
    pub(crate) snapshot_id: u16,
    pub(crate) base_id: u16,
    pub(crate) tick: u32,
    pub(crate) arena_stream_id: u64,
    pub(crate) near_count: u8,
    pub(crate) mid_count: u8,
    pub(crate) event_count: u8,
}

impl SnapshotHeader {
    pub(crate) const TYPE: u8 = SNAPSHOT_PACKET;
    pub(crate) const HEADER_LEN: usize = 1 + 1 + 2 + 2 + 4 + 8 + 1 + 1 + 1;

    pub(crate) fn is_full(self) -> bool {
        self.flags & SNAPSHOT_FLAG_FULL != 0
    }

    pub(crate) fn encode_into(&self, bytes: &mut Vec<u8>) {
        push_u8(bytes, Self::TYPE);
        push_u8(bytes, self.flags | SNAPSHOT_WIRE_REVISIONS);
        push_u16(bytes, self.snapshot_id);
        push_u16(bytes, self.base_id);
        push_u32(bytes, self.tick);
        push_u64(bytes, self.arena_stream_id);
        push_u8(bytes, self.near_count);
        push_u8(bytes, self.mid_count);
        push_u8(bytes, self.event_count);
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_LEN {
            return Err(format!(
                "snapshot frame too short: {} < {}",
                packet.len(),
                Self::HEADER_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected snapshot frame type: {:#04x}",
                packet[0]
            ));
        }
        let flags = packet[1];
        if flags & !SNAPSHOT_KNOWN_FLAGS != 0 {
            return Err(format!("unsupported snapshot flags: {flags:#04x}"));
        }
        if flags & SNAPSHOT_FLAG_SOURCE_TIME == 0 {
            return Err("snapshot source-time wire revision required".to_owned());
        }
        if flags & SNAPSHOT_FLAG_MOTION_SEGMENT == 0 {
            return Err("snapshot motion-segment wire revision required".to_owned());
        }
        if flags & SNAPSHOT_FLAG_RELOCATION_GENERATION == 0 {
            return Err("snapshot relocation-generation wire revision required".to_owned());
        }
        let flags = flags & !SNAPSHOT_WIRE_REVISIONS;
        let snapshot_id = read_u16(packet, 2)?;
        let base_id = read_u16(packet, 4)?;
        let tick = read_u32(packet, 6)?;
        let arena_stream_id = read_u64(packet, 10)?;
        let near_count = packet[18];
        let mid_count = packet[19];
        let event_count = packet[20];
        Ok(Self {
            flags,
            snapshot_id,
            base_id,
            tick,
            arena_stream_id,
            near_count,
            mid_count,
            event_count,
        })
    }
}

/// One entity entry inside a snapshot. Variable-length on the wire:
/// `entity_id:u16 | field_mask:u16 | [optional fields per mask bit]`.
///
/// Phase 3e-7 widened `field_mask` from u8 to u16 and added the full
/// per-entity state that Continuum's `LARGE_POSITION` carries
/// (velocity, direction, energy, weapon, status/togglables, bounty,
/// ping, flag_timer) so the snapshot stream can fully replace the
/// legacy per-tick position relay. Mid-tier entries use bits 0 + 1
/// only (radar position); near-tier entries fill in everything the
/// shooter / observer needs to render combat at full fidelity.
///
/// Delta semantics:
/// - `header.flags & SNAPSHOT_FLAG_FULL != 0` → full state frame: every listed entity
///   has the relevant mask bits set with full values.
/// - otherwise → delta against the `base_id` snapshot
///   in the recipient's history; only fields whose mask bit is set
///   appear in the payload, others keep their last-applied value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotEntity {
    pub(crate) entity_id: u16,
    pub(crate) field_mask: u16,
    pub(crate) x: Option<u16>,
    pub(crate) y: Option<u16>,
    pub(crate) velocity_x: Option<i16>,
    pub(crate) velocity_y: Option<i16>,
    pub(crate) direction: Option<u8>,
    pub(crate) energy: Option<u16>,
    pub(crate) weapon: Option<u16>,
    pub(crate) status: Option<u8>,
    pub(crate) bounty: Option<u16>,
    pub(crate) ping: Option<u8>,
    pub(crate) flag_timer: Option<u16>,
    pub(crate) life_seq: Option<u32>,
    pub(crate) sample_time: Option<PresentationSampleTime>,
    pub(crate) motion_segment: Option<u32>,
    /// Coupled to FIELD_MOTION_SEGMENT; unchanged on position-continuous Repel.
    pub(crate) relocation_generation: Option<u32>,
    /// With `FIELD_POSITION_OFFSET`, the wire carries these instead of `x`
    /// and `y`: offsets from the base position, resolved by the decoder.
    pub(crate) x_offset: Option<i8>,
    pub(crate) y_offset: Option<i8>,
}

impl SnapshotEntity {
    pub(crate) const FIELD_X: u16 = SNAPSHOT_FIELD_X;
    pub(crate) const FIELD_Y: u16 = SNAPSHOT_FIELD_Y;
    pub(crate) const FIELD_VELOCITY_X: u16 = SNAPSHOT_FIELD_VELOCITY_X;
    pub(crate) const FIELD_VELOCITY_Y: u16 = SNAPSHOT_FIELD_VELOCITY_Y;
    pub(crate) const FIELD_DIRECTION: u16 = SNAPSHOT_FIELD_DIRECTION;
    pub(crate) const FIELD_ENERGY: u16 = SNAPSHOT_FIELD_ENERGY;
    pub(crate) const FIELD_WEAPON: u16 = SNAPSHOT_FIELD_WEAPON;
    pub(crate) const FIELD_STATUS: u16 = SNAPSHOT_FIELD_STATUS;
    pub(crate) const FIELD_BOUNTY: u16 = SNAPSHOT_FIELD_BOUNTY;
    pub(crate) const FIELD_PING: u16 = SNAPSHOT_FIELD_PING;
    pub(crate) const FIELD_FLAG_TIMER: u16 = SNAPSHOT_FIELD_FLAG_TIMER;
    pub(crate) const FIELD_LIFE_SEQ: u16 = SNAPSHOT_FIELD_LIFE_SEQ;
    pub(crate) const FIELD_SAMPLE_TIME: u16 = SNAPSHOT_FIELD_SAMPLE_TIME;
    pub(crate) const FIELD_POSITION_OFFSET: u16 = SNAPSHOT_FIELD_POSITION_OFFSET;
    pub(crate) const FIELD_MOTION_SEGMENT: u16 = SNAPSHOT_FIELD_MOTION_SEGMENT;

    /// Length of the entry header (`entity_id:u16 | field_mask:u16`).
    /// Total wire length depends on which mask bits are set; callers
    /// should use `wire_len()` for the full size.
    pub(crate) const HEADER_LEN: usize = 4;

    /// Mid-tier entity: radar position only (mask = `FIELD_X | FIELD_Y`).
    pub(crate) fn full(entity_id: u16, x: u16, y: u16) -> Self {
        Self {
            entity_id,
            field_mask: Self::FIELD_X | Self::FIELD_Y,
            x: Some(x),
            y: Some(y),
            ..Self::default()
        }
    }

    /// Mid-tier entity with exact actor generation as well as radar position.
    pub(crate) fn mid_full(entity_id: u16, x: u16, y: u16, life_seq: u32) -> Self {
        Self {
            entity_id,
            field_mask: Self::FIELD_X | Self::FIELD_Y | Self::FIELD_LIFE_SEQ,
            x: Some(x),
            y: Some(y),
            life_seq: Some(life_seq),
            ..Self::default()
        }
    }

    /// Delta entry with no field changes: still in AOI, all fields
    /// match the base snapshot.
    pub(crate) fn unchanged(entity_id: u16) -> Self {
        Self {
            entity_id,
            field_mask: 0,
            ..Self::default()
        }
    }

    /// Near-tier entity with all supported fields present.
    pub(crate) fn near_full(entity_id: u16, state: SnapshotEntityState) -> Self {
        Self {
            entity_id,
            field_mask: SNAPSHOT_NEAR_COMPLETE_MASK,
            x: Some(state.x),
            y: Some(state.y),
            velocity_x: Some(state.velocity_x),
            velocity_y: Some(state.velocity_y),
            direction: Some(state.direction),
            energy: Some(state.energy),
            weapon: Some(state.weapon),
            status: Some(state.status),
            bounty: Some(state.bounty),
            ping: Some(state.ping),
            flag_timer: Some(state.flag_timer),
            life_seq: Some(state.life_seq),
            sample_time: Some(state.sample_time),
            motion_segment: Some(state.motion_segment),
            relocation_generation: Some(state.relocation_generation),
            x_offset: None,
            y_offset: None,
        }
    }

    /// Bytes consumed on the wire for this entity entry.
    pub(crate) fn wire_len(&self) -> usize {
        self.wire_len_at(None)
    }

    /// As [`Self::wire_len`], with sample time relative to `compact_tick`
    /// when the frame is compact.
    pub(crate) fn wire_len_at(&self, compact_tick: Option<u32>) -> usize {
        let mut len = Self::HEADER_LEN;
        let m = self.field_mask;
        let position_len = if m & Self::FIELD_POSITION_OFFSET != 0 {
            1
        } else {
            2
        };
        if m & Self::FIELD_X != 0 {
            len += position_len;
        }
        if m & Self::FIELD_Y != 0 {
            len += position_len;
        }
        if m & Self::FIELD_VELOCITY_X != 0 {
            len += 2;
        }
        if m & Self::FIELD_VELOCITY_Y != 0 {
            len += 2;
        }
        if m & Self::FIELD_DIRECTION != 0 {
            len += 1;
        }
        if m & Self::FIELD_ENERGY != 0 {
            len += 2;
        }
        if m & Self::FIELD_WEAPON != 0 {
            len += 2;
        }
        if m & Self::FIELD_STATUS != 0 {
            len += 1;
        }
        if m & Self::FIELD_BOUNTY != 0 {
            len += 2;
        }
        if m & Self::FIELD_PING != 0 {
            len += 1;
        }
        if m & Self::FIELD_FLAG_TIMER != 0 {
            len += 2;
        }
        if m & Self::FIELD_LIFE_SEQ != 0 {
            len += 4;
        }
        if m & Self::FIELD_SAMPLE_TIME != 0 {
            len += match compact_tick.zip(self.sample_time) {
                Some((frame_tick, sample)) if compact_sample_time(frame_tick, sample).is_some() => {
                    1
                }
                Some(_) => 6,
                None => 5,
            };
        }
        if m & Self::FIELD_MOTION_SEGMENT != 0 {
            len += 8;
        }
        len
    }

    pub(crate) fn encode_into(&self, bytes: &mut Vec<u8>) {
        self.encode_into_at(bytes, None);
    }

    /// Encodes the entry. In a compact frame (`compact_tick` is the frame's
    /// tick) the sample time is one byte, `ticks_behind * 3 + third`, or
    /// `0xff` followed by the five-byte form when it does not fit.
    pub(crate) fn encode_into_at(&self, bytes: &mut Vec<u8>, compact_tick: Option<u32>) {
        push_u16(bytes, self.entity_id);
        push_u16(bytes, self.field_mask);
        let mask = self.field_mask;
        if mask & Self::FIELD_POSITION_OFFSET != 0 {
            if mask & Self::FIELD_X != 0 {
                push_u8(
                    bytes,
                    self.x_offset.expect("FIELD_X requires an x offset") as u8,
                );
            }
            if mask & Self::FIELD_Y != 0 {
                push_u8(
                    bytes,
                    self.y_offset.expect("FIELD_Y requires a y offset") as u8,
                );
            }
        } else {
            if mask & Self::FIELD_X != 0 {
                push_u16(bytes, self.x.expect("FIELD_X requires an x value"));
            }
            if mask & Self::FIELD_Y != 0 {
                push_u16(bytes, self.y.expect("FIELD_Y requires a y value"));
            }
        }
        if mask & Self::FIELD_VELOCITY_X != 0 {
            bytes.extend_from_slice(
                &self
                    .velocity_x
                    .expect("FIELD_VELOCITY_X requires a velocity_x value")
                    .to_le_bytes(),
            );
        }
        if mask & Self::FIELD_VELOCITY_Y != 0 {
            bytes.extend_from_slice(
                &self
                    .velocity_y
                    .expect("FIELD_VELOCITY_Y requires a velocity_y value")
                    .to_le_bytes(),
            );
        }
        if mask & Self::FIELD_DIRECTION != 0 {
            push_u8(
                bytes,
                self.direction
                    .expect("FIELD_DIRECTION requires a direction value"),
            );
        }
        if mask & Self::FIELD_ENERGY != 0 {
            push_u16(
                bytes,
                self.energy.expect("FIELD_ENERGY requires an energy value"),
            );
        }
        if mask & Self::FIELD_WEAPON != 0 {
            push_u16(
                bytes,
                self.weapon.expect("FIELD_WEAPON requires a weapon value"),
            );
        }
        if mask & Self::FIELD_STATUS != 0 {
            push_u8(
                bytes,
                self.status.expect("FIELD_STATUS requires a status value"),
            );
        }
        if mask & Self::FIELD_BOUNTY != 0 {
            push_u16(
                bytes,
                self.bounty.expect("FIELD_BOUNTY requires a bounty value"),
            );
        }
        if mask & Self::FIELD_PING != 0 {
            push_u8(bytes, self.ping.expect("FIELD_PING requires a ping value"));
        }
        if mask & Self::FIELD_FLAG_TIMER != 0 {
            push_u16(
                bytes,
                self.flag_timer
                    .expect("FIELD_FLAG_TIMER requires a flag_timer value"),
            );
        }
        if mask & Self::FIELD_LIFE_SEQ != 0 {
            push_u32(
                bytes,
                self.life_seq
                    .expect("FIELD_LIFE_SEQ requires a life_seq value"),
            );
        }
        if mask & Self::FIELD_SAMPLE_TIME != 0 {
            let sample = self
                .sample_time
                .expect("FIELD_SAMPLE_TIME requires a value");
            let compact = compact_tick.map(|frame_tick| compact_sample_time(frame_tick, sample));
            if let Some(Some(byte)) = compact {
                push_u8(bytes, byte);
            } else {
                if compact.is_some() {
                    push_u8(bytes, COMPACT_SAMPLE_TIME_FULL);
                }
                let (tick, third) = sample.parts();
                push_u32(bytes, tick);
                push_u8(bytes, third);
            }
        }
        if mask & Self::FIELD_MOTION_SEGMENT != 0 {
            push_u32(
                bytes,
                self.motion_segment
                    .expect("FIELD_MOTION_SEGMENT requires a value"),
            );
            push_u32(
                bytes,
                self.relocation_generation
                    .expect("FIELD_MOTION_SEGMENT requires a relocation generation"),
            );
        }
    }

    pub(crate) fn parse_at(packet: &[u8], offset: usize) -> Result<(Self, usize), String> {
        Self::parse_at_compact(packet, offset, None)
    }

    /// As [`Self::parse_at`], reading a compact frame's sample time relative
    /// to `compact_tick`.
    pub(crate) fn parse_at_compact(
        packet: &[u8],
        offset: usize,
        compact_tick: Option<u32>,
    ) -> Result<(Self, usize), String> {
        if offset + Self::HEADER_LEN > packet.len() {
            return Err(format!(
                "snapshot entity header truncated at offset {offset}"
            ));
        }
        let entity_id = read_u16(packet, offset)?;
        let field_mask = read_u16(packet, offset + 2)?;
        if field_mask & !(SNAPSHOT_NEAR_COMPLETE_MASK | SNAPSHOT_FIELD_POSITION_OFFSET) != 0 {
            return Err(format!(
                "unsupported snapshot entity mask: {field_mask:#06x}"
            ));
        }
        let mut pos = offset + Self::HEADER_LEN;
        let mut entity = Self {
            entity_id,
            field_mask,
            ..Self::default()
        };
        macro_rules! take_u16 {
            ($field:ident) => {{
                let v = read_u16(packet, pos)?;
                pos += 2;
                entity.$field = Some(v);
            }};
        }
        macro_rules! take_i16 {
            ($field:ident) => {{
                if pos + 2 > packet.len() {
                    return Err(format!("snapshot entity i16 field truncated at {pos}"));
                }
                let v = i16::from_le_bytes([packet[pos], packet[pos + 1]]);
                pos += 2;
                entity.$field = Some(v);
            }};
        }
        macro_rules! take_u8 {
            ($field:ident) => {{
                if pos + 1 > packet.len() {
                    return Err(format!("snapshot entity u8 field truncated at {pos}"));
                }
                entity.$field = Some(packet[pos]);
                pos += 1;
            }};
        }
        macro_rules! take_u32 {
            ($field:ident) => {{
                let v = read_u32(packet, pos)?;
                pos += 4;
                entity.$field = Some(v);
            }};
        }
        if field_mask & Self::FIELD_POSITION_OFFSET != 0 {
            for (bit, offset) in [
                (Self::FIELD_X, &mut entity.x_offset),
                (Self::FIELD_Y, &mut entity.y_offset),
            ] {
                if field_mask & bit != 0 {
                    *offset = Some(
                        *packet
                            .get(pos)
                            .ok_or("snapshot position offset truncated")?
                            as i8,
                    );
                    pos += 1;
                }
            }
        } else {
            if field_mask & Self::FIELD_X != 0 {
                take_u16!(x);
            }
            if field_mask & Self::FIELD_Y != 0 {
                take_u16!(y);
            }
        }
        if field_mask & Self::FIELD_VELOCITY_X != 0 {
            take_i16!(velocity_x);
        }
        if field_mask & Self::FIELD_VELOCITY_Y != 0 {
            take_i16!(velocity_y);
        }
        if field_mask & Self::FIELD_DIRECTION != 0 {
            take_u8!(direction);
        }
        if field_mask & Self::FIELD_ENERGY != 0 {
            take_u16!(energy);
        }
        if field_mask & Self::FIELD_WEAPON != 0 {
            take_u16!(weapon);
        }
        if field_mask & Self::FIELD_STATUS != 0 {
            take_u8!(status);
        }
        if field_mask & Self::FIELD_BOUNTY != 0 {
            take_u16!(bounty);
        }
        if field_mask & Self::FIELD_PING != 0 {
            take_u8!(ping);
        }
        if field_mask & Self::FIELD_FLAG_TIMER != 0 {
            take_u16!(flag_timer);
        }
        if field_mask & Self::FIELD_LIFE_SEQ != 0 {
            take_u32!(life_seq);
        }
        let compact = match compact_tick {
            Some(_) if field_mask & Self::FIELD_SAMPLE_TIME != 0 => {
                let byte = *packet
                    .get(pos)
                    .ok_or("snapshot compact sample time truncated")?;
                pos += 1;
                (byte != COMPACT_SAMPLE_TIME_FULL).then_some(byte)
            }
            _ => None,
        };
        if let (Some(frame_tick), Some(byte)) = (compact_tick, compact) {
            let tick = frame_tick.wrapping_sub(u32::from(byte / 3))
                & crate::continuum::CONTINUUM_TICK_MASK;
            entity.sample_time = Some(
                PresentationSampleTime::from_parts(tick, byte % 3)
                    .ok_or("snapshot compact sample time out of range")?,
            );
        } else if field_mask & Self::FIELD_SAMPLE_TIME != 0 {
            let tick = read_u32(packet, pos)?;
            let third = *packet
                .get(pos + 4)
                .ok_or("snapshot sample-time phase truncated")?;
            entity.sample_time = Some(
                PresentationSampleTime::from_parts(tick, third)
                    .ok_or("snapshot sample-time tick or phase out of range")?,
            );
            pos += 5;
        }
        if field_mask & Self::FIELD_MOTION_SEGMENT != 0 {
            entity.motion_segment = Some(read_u32(packet, pos)?);
            entity.relocation_generation = Some(read_u32(packet, pos + 4)?);
            pos += 8;
        }
        Ok((entity, pos - offset))
    }
}

/// Concrete entity state retained as a delta base by both endpoints. Mid-tier
/// entries use only `x` and `y`; the remaining fields stay at their defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotEntityState {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) direction: u8,
    pub(crate) energy: u16,
    pub(crate) weapon: u16,
    pub(crate) status: u8,
    pub(crate) bounty: u16,
    pub(crate) ping: u8,
    pub(crate) flag_timer: u16,
    pub(crate) life_seq: u32,
    pub(crate) sample_time: PresentationSampleTime,
    /// Scoped by actor life and arena stream. Zero is an uninitialized source.
    pub(crate) motion_segment: u32,
    pub(crate) relocation_generation: u32,
}

pub(crate) type SnapshotEntityStateMap = BTreeMap<u16, SnapshotEntityState>;

fn delta_near_entity(
    entity_id: u16,
    current: SnapshotEntityState,
    base: Option<SnapshotEntityState>,
) -> SnapshotEntity {
    let Some(base) = base else {
        return SnapshotEntity::near_full(entity_id, current);
    };
    let mut entity = SnapshotEntity {
        entity_id,
        ..SnapshotEntity::default()
    };
    macro_rules! encode_changed {
        ($field:ident, $mask:ident) => {
            if current.$field != base.$field {
                entity.field_mask |= $mask;
                entity.$field = Some(current.$field);
            }
        };
    }
    encode_changed!(x, SNAPSHOT_FIELD_X);
    encode_changed!(y, SNAPSHOT_FIELD_Y);
    encode_changed!(velocity_x, SNAPSHOT_FIELD_VELOCITY_X);
    encode_changed!(velocity_y, SNAPSHOT_FIELD_VELOCITY_Y);
    encode_changed!(direction, SNAPSHOT_FIELD_DIRECTION);
    encode_changed!(energy, SNAPSHOT_FIELD_ENERGY);
    encode_changed!(weapon, SNAPSHOT_FIELD_WEAPON);
    encode_changed!(status, SNAPSHOT_FIELD_STATUS);
    encode_changed!(bounty, SNAPSHOT_FIELD_BOUNTY);
    encode_changed!(ping, SNAPSHOT_FIELD_PING);
    encode_changed!(flag_timer, SNAPSHOT_FIELD_FLAG_TIMER);
    encode_changed!(life_seq, SNAPSHOT_FIELD_LIFE_SEQ);
    encode_changed!(sample_time, SNAPSHOT_FIELD_SAMPLE_TIME);
    if current.motion_segment != base.motion_segment
        || current.relocation_generation != base.relocation_generation
    {
        entity.field_mask |= SNAPSHOT_FIELD_MOTION_SEGMENT;
        entity.motion_segment = Some(current.motion_segment);
        entity.relocation_generation = Some(current.relocation_generation);
    }
    entity
}

pub(crate) fn delta_encode_near_tier(
    current: &[(u16, SnapshotEntityState)],
    base: Option<&SnapshotEntityStateMap>,
) -> Vec<SnapshotEntity> {
    current
        .iter()
        .map(|&(entity_id, state)| {
            delta_near_entity(
                entity_id,
                state,
                base.and_then(|states| states.get(&entity_id)).copied(),
            )
        })
        .collect()
}

pub(crate) fn delta_encode_mid_tier(
    current: &[(u16, u16, u16, u32)],
    base: Option<&SnapshotEntityStateMap>,
) -> Vec<SnapshotEntity> {
    current
        .iter()
        .map(|&(entity_id, x, y, life_seq)| {
            let Some(base) = base.and_then(|states| states.get(&entity_id)) else {
                return SnapshotEntity::mid_full(entity_id, x, y, life_seq);
            };
            let mut entity = SnapshotEntity {
                entity_id,
                ..SnapshotEntity::default()
            };
            if x != base.x {
                entity.field_mask |= SNAPSHOT_FIELD_X;
                entity.x = Some(x);
            }
            if y != base.y {
                entity.field_mask |= SNAPSHOT_FIELD_Y;
                entity.y = Some(y);
            }
            if life_seq != base.life_seq {
                entity.field_mask |= SNAPSHOT_FIELD_LIFE_SEQ;
                entity.life_seq = Some(life_seq);
            }
            entity
        })
        .collect()
}

fn apply_entity(
    entity: &SnapshotEntity,
    base: Option<SnapshotEntityState>,
    require_near: bool,
) -> Result<SnapshotEntityState, String> {
    let required_mask = if require_near {
        SNAPSHOT_NEAR_COMPLETE_MASK
    } else {
        SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y
    };
    if base.is_none() && entity.field_mask & required_mask != required_mask {
        return Err(format!(
            "snapshot entity {} has no base and mask {:#06x} lacks required fields {:#06x}",
            entity.entity_id, entity.field_mask, required_mask
        ));
    }
    if entity.field_mask & SNAPSHOT_FIELD_POSITION_OFFSET != 0 && base.is_none() {
        return Err(format!(
            "snapshot entity {} has a position offset but no base",
            entity.entity_id
        ));
    }
    let mut state = base.unwrap_or_default();
    for (offset, position) in [
        (entity.x_offset, &mut state.x),
        (entity.y_offset, &mut state.y),
    ] {
        if let Some(offset) = offset {
            *position = position
                .checked_add_signed(i16::from(offset))
                .ok_or_else(|| {
                    format!(
                        "snapshot entity {} position offset overflows",
                        entity.entity_id
                    )
                })?;
        }
    }
    macro_rules! apply_field {
        ($field:ident) => {
            if let Some(value) = entity.$field {
                state.$field = value;
            }
        };
    }
    apply_field!(x);
    apply_field!(y);
    apply_field!(velocity_x);
    apply_field!(velocity_y);
    apply_field!(direction);
    apply_field!(energy);
    apply_field!(weapon);
    apply_field!(status);
    apply_field!(bounty);
    apply_field!(ping);
    apply_field!(flag_timer);
    apply_field!(life_seq);
    apply_field!(sample_time);
    apply_field!(motion_segment);
    apply_field!(relocation_generation);
    Ok(state)
}

/// A decoded frame reconstructed into complete entity values plus the exact
/// state map that future deltas may reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppliedSnapshot {
    pub(crate) frame: SnapshotFrame,
    pub(crate) entities: SnapshotEntityStateMap,
}

/// Apply a decoded frame against its exact referenced base. A FULL frame
/// ignores `base`; callers must reject a non-full delta before calling when
/// its base id is unknown.
/// Turns absolute x/y deltas into one-byte offsets from `base` wherever every
/// present coordinate fits, for clients that declared
/// [`CLIENT_PROTOCOL_POSITION_OFFSETS`]. Entries absent from the base keep
/// absolute positions.
pub(crate) fn offset_positions(entities: &mut [SnapshotEntity], base: &SnapshotEntityStateMap) {
    for entity in entities {
        let Some(base) = base.get(&entity.entity_id) else {
            continue;
        };
        if entity.field_mask & (SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y) == 0 {
            continue;
        }
        let offset = |value: Option<u16>, from: u16| {
            value.map(|value| i8::try_from(i32::from(value) - i32::from(from)).ok())
        };
        let (x, y) = (offset(entity.x, base.x), offset(entity.y, base.y));
        if x.is_some_and(|x| x.is_none()) || y.is_some_and(|y| y.is_none()) {
            continue;
        }
        entity.field_mask |= SNAPSHOT_FIELD_POSITION_OFFSET;
        entity.x_offset = x.flatten();
        entity.y_offset = y.flatten();
        entity.x = None;
        entity.y = None;
    }
}

pub(crate) fn delta_apply(
    base: Option<&SnapshotEntityStateMap>,
    frame: &SnapshotFrame,
) -> Result<AppliedSnapshot, String> {
    if !frame.header.is_full() && base.is_none() {
        return Err(format!(
            "snapshot {} references unknown base {}",
            frame.header.snapshot_id, frame.header.base_id
        ));
    }

    let mut entities = SnapshotEntityStateMap::new();
    let mut near_entities = Vec::with_capacity(frame.near_entities.len());
    for entity in &frame.near_entities {
        let base_state = if frame.header.is_full() {
            None
        } else {
            base.and_then(|states| states.get(&entity.entity_id))
                .copied()
        };
        let state = apply_entity(entity, base_state, true)?;
        entities.insert(entity.entity_id, state);
        let mut reconstructed = SnapshotEntity::near_full(entity.entity_id, state);
        if !frame.header.is_full() {
            // Weapon and flash status are one-shot visual signals. Preserve
            // whether they were present on this wire frame. Status still
            // carries its fully reconstructed persistent bits so a delta
            // against an older base can return safety/cloak/etc. to that
            // base's value without leaking an intermediate last-received
            // value into the synthesized position update.
            reconstructed.weapon = entity.weapon;
            if reconstructed.weapon.is_none() {
                reconstructed.field_mask &= !SNAPSHOT_FIELD_WEAPON;
            }
            if entity.field_mask & SNAPSHOT_FIELD_STATUS == 0 {
                reconstructed.field_mask &= !SNAPSHOT_FIELD_STATUS;
            }
        }
        near_entities.push(reconstructed);
    }

    let mut mid_entities = Vec::with_capacity(frame.mid_entities.len());
    for entity in &frame.mid_entities {
        let base_state = if frame.header.is_full() {
            None
        } else {
            base.and_then(|states| states.get(&entity.entity_id))
                .copied()
        };
        let position = apply_entity(entity, base_state, false)?;
        // Server history deliberately stores mid-tier entities as radar
        // position plus default near-only fields. Mirror that normalization
        // exactly so an acknowledged mid snapshot is a symmetric delta base
        // if the entity later returns to the near tier.
        let state = SnapshotEntityState {
            x: position.x,
            y: position.y,
            life_seq: position.life_seq,
            ..SnapshotEntityState::default()
        };
        entities.insert(entity.entity_id, state);
        mid_entities.push(SnapshotEntity::mid_full(
            entity.entity_id,
            state.x,
            state.y,
            state.life_seq,
        ));
    }

    Ok(AppliedSnapshot {
        frame: SnapshotFrame {
            header: frame.header,
            near_entities,
            mid_entities,
            events: frame.events.clone(),
        },
        entities,
    })
}

/// One per-snapshot event. The snapshot stream carries WATCH_DAMAGE
/// as the realtime combat event channel. Wire layout per event:
/// `event_type:u8 | body_len:u8 | body[body_len]`. The
/// `body_len` prefix lets unknown event types be skipped for
/// forward compat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotEvent {
    WatchDamage(SnapshotWatchDamage),
    /// Repeated, recipient-private Rocket authority. Unlike a one-shot
    /// activation receipt, every advancing snapshot can repair a lost or
    /// rejected client prediction without allowing delivery latency to extend
    /// the absolute server deadline.
    RocketState(SnapshotRocketState),
    /// Repeated, recipient-private lifecycle authority. This rides the
    /// unordered snapshot channel so a lost/HOL-blocked reliable death packet
    /// cannot leave the local ship alive until the reliable resend timer.
    LifecycleState(SnapshotLifecycleState),
    /// Repeated, recipient-private prize/loadout authority. This repairs a
    /// lost pickup acknowledgement and replaces local prize prediction with
    /// the server's exact life/revision state.
    PrizeState(SnapshotPrizeState),
    /// Repeated server-owned observer combat-view schedule. The browser may
    /// consume this directive but cannot author its generation or delay.
    ObserverTimeline(crate::combat_timeline::ObserverTimelineDirectiveV1),
    /// Repeated, recipient-private input acknowledgement that lets the client
    /// stop resending inputs the server already has. Older clients skip it as
    /// an unknown type and keep sending whole windows.
    InputApplied(SnapshotInputApplied),
    /// In place of an unchanged `PrizeState`: the full prize body carried by
    /// snapshot `snapshot_id`, with this current energy. The client session
    /// expands it into that `PrizeState` before anything else reads the frame.
    PrizeReference(SnapshotPrizeReference),
    /// Forward-compat sentinel: a known wire shape that this build
    /// does not yet decode. Carries the type byte + body bytes so
    /// tooling can inspect; the runtime ignores it.
    Unknown {
        event_type: u8,
        body: Vec<u8>,
    },
}

/// Body of a `SnapshotEvent::WatchDamage`. Mirrors the per-hit
/// information from the older WATCH_DAMAGE (`0x38`) reliable packet,
/// plus a `lethal` flag the snapshot client can use to align the
/// visual flash with the corresponding `PLAYER_DEATH`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotWatchDamage {
    pub(crate) victim_id: u16,
    pub(crate) shooter_id: u16,
    pub(crate) victim_life_seq: Option<u32>,
    pub(crate) shooter_life_seq: Option<u32>,
    pub(crate) weapon_data: u16,
    pub(crate) damage: u16,
    pub(crate) energy_before: Option<u16>,
    pub(crate) energy_after: u16,
    pub(crate) server_tick: Option<u32>,
    pub(crate) damage_seq: Option<u32>,
    pub(crate) projectile_id: Option<u32>,
    pub(crate) lethal: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotRocketState {
    pub(crate) life_seq: u32,
    /// Newest retained input sequence carrying a Rocket activation pulse. This
    /// is a receipt for the pulse itself, not a general input watermark.
    pub(crate) observed_input_sequence: Option<u16>,
    /// Newest advancing raw-input sequence admitted by the server. A missing
    /// Rocket pulse is only conclusively lost once this watermark has moved past
    /// the complete 64-record recovery/retention window.
    pub(crate) input_sequence_watermark: Option<u16>,
    pub(crate) activation_sequence: Option<u16>,
    pub(crate) until_tick: Option<u32>,
    pub(crate) active: bool,
    pub(crate) rockets: u8,
}

/// The server has accepted every input of `life_seq` through
/// `through_sequence` with no gap (`ClientHandle::last_position_sequence`).
/// Unlike [`SnapshotRocketState::input_sequence_watermark`], the newest
/// admitted input, which can sit above a gap, this never passes a missing
/// input, so inputs after it must keep travelling until it moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotInputApplied {
    pub(crate) life_seq: u32,
    pub(crate) through_sequence: u16,
}

/// See [`SnapshotEvent::PrizeReference`]. The server names only a snapshot
/// the client acknowledged, or one acknowledged frame's own reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotPrizeReference {
    pub(crate) snapshot_id: u16,
    pub(crate) energy: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotRepelDelivery {
    pub(crate) active_steps: u16,
    pub(crate) delivery_generation: u32,
    pub(crate) source_player_id: u16,
    pub(crate) source_life_seq: u32,
    pub(crate) origin_x: u16,
    pub(crate) origin_y: u16,
    pub(crate) source_frequency: u16,
    pub(crate) expected_velocity_x_bits: u32,
    pub(crate) expected_velocity_y_bits: u32,
    /// Exact Repel weapon word and immutable source ship captured when the
    /// server admitted the event. The private application record is therefore
    /// self-contained and never consults mutable AOI/roster state.
    pub(crate) weapon_data: u16,
    pub(crate) source_ship: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotLifecycleState {
    pub(crate) life_seq: u32,
    pub(crate) alive: bool,
    /// Absolute server tick for the current death's respawn deadline.
    pub(crate) respawn_at_tick: Option<u32>,
    /// Original server-owned repel deadline for this life. The deadline is
    /// repeated privately so delayed snapshot delivery cannot start a fresh
    /// client-side repel lifetime and diverge from canonical movement.
    pub(crate) repel_until_tick: Option<u32>,
    /// Server-authored number of future 60 Hz input records that receive
    /// RepelSpeed after this exact weapon-bearing snapshot is applied.
    pub(crate) repel_delivery: Option<SnapshotRepelDelivery>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotPrizeState {
    pub(crate) life_seq: u32,
    pub(crate) revision: u32,
    pub(crate) flags: u16,
    pub(crate) bounty: u16,
    pub(crate) max_energy: u16,
    pub(crate) rotation_bits: u32,
    pub(crate) thrust_bits: u32,
    pub(crate) top_speed_bits: u32,
    pub(crate) recharge_bits: u32,
    pub(crate) gun_level: u8,
    pub(crate) bomb_level: u8,
    pub(crate) shrapnel: u8,
    pub(crate) repels: u8,
    pub(crate) bursts: u8,
    pub(crate) decoys: u8,
    pub(crate) thors: u8,
    pub(crate) bricks: u8,
    pub(crate) portals: u8,
    pub(crate) super_until_tick: Option<u32>,
    pub(crate) shield_until_tick: Option<u32>,
    pub(crate) engine_shutdown_until_tick: Option<u32>,
    /// EMP recharge-only deadline, independent of the prize-owned movement
    /// deadline (`ShipController:244-259`).
    pub(crate) emp_until_tick: Option<u32>,
    pub(crate) damage_loss_seq: Option<u32>,
    pub(crate) damage_loss_prize_id: Option<i16>,
    /// Current authoritative energy for this recipient, appended after the v1 and v2 bodies. Carried as the same
    /// `u16` the server tracks, matching `max_energy`, because that is the quantity the relocation reports.
    ///
    /// Prize/loadout authority already carries `max_energy`, and this struct's contract is to replace local
    /// prediction with the server's exact state, so the *current* energy belongs here too. Without it there is no
    /// event at all for a server-side energy change the client did not cause: a wormhole relocation retains 20% of
    /// the ship's energy (`retain_fraction(0.2, 1.0)`) and nothing told the client, so its prediction kept its own
    /// value until some unrelated damage/prize/life event happened to resync it. In one measured run the server
    /// signed a relocation at 330 while the client predicted 911.25.
    pub(crate) energy: Option<u16>,
}

impl SnapshotPrizeState {
    /// Server-owned entry policy, retained across soft lives so a neutral
    /// respawn can name the exact energy before its successor is committed.
    pub(crate) const MAXED_SPAWN_LOADOUT: u16 = 1 << 14;
}

impl SnapshotEvent {
    pub(crate) const TYPE_WATCH_DAMAGE: u8 = 0x01;
    pub(crate) const TYPE_ROCKET_STATE: u8 = 0x02;
    pub(crate) const TYPE_LIFECYCLE_STATE: u8 = 0x03;
    pub(crate) const TYPE_PRIZE_STATE: u8 = 0x04;
    pub(crate) const TYPE_OBSERVER_TIMELINE: u8 = 0x05;
    pub(crate) const TYPE_INPUT_APPLIED: u8 = 0x06;
    pub(crate) const TYPE_PRIZE_REFERENCE: u8 = 0x07;
    pub(crate) const WATCH_DAMAGE_V1_BODY_LEN: usize = 11;
    /// Length of the `WatchDamage` body in bytes.
    pub(crate) const WATCH_DAMAGE_BODY_LEN: usize = 33;
    /// `life_seq:u32 | flags:u8 | observed_sequence:u16 |
    /// activation_sequence:u16 | until_tick:u32 | rockets:u8 |
    /// input_sequence_watermark:u16`.
    pub(crate) const ROCKET_STATE_BODY_LEN: usize = 16;
    /// `life_seq:u32 | flags:u8 | respawn_at_tick:u32 | repel_until_tick:u32 |
    /// repel_active_steps:u16 | repel_delivery_generation:u32 |
    /// repel_source_player_id:u16 | repel_source_life_seq:u32 |
    /// repel_origin_x:u16 | repel_origin_y:u16 | repel_source_frequency:u16 |
    /// repel_expected_velocity_x_bits:u32 | repel_expected_velocity_y_bits:u32 |
    /// repel_weapon_data:u16 | repel_source_ship:u8`.
    pub(crate) const LIFECYCLE_STATE_BODY_LEN: usize = 42;
    /// The lifecycle body up to `repel_until_tick`, for compact frames whose
    /// state has no Repel delivery.
    pub(crate) const LIFECYCLE_STATE_COMPACT_BODY_LEN: usize = 13;
    /// Private prize authority body, including raw-f32 movement upgrades and the current energy.
    pub(crate) const PRIZE_STATE_BODY_LEN: usize = 63;
    /// Prize body before the current-energy field, still accepted for replay/parse compatibility.
    pub(crate) const PRIZE_STATE_V2_BODY_LEN: usize = 61;
    /// Pre-EMP-deadline prize body, still accepted for replay/parse
    /// compatibility. The EMP deadline is simply absent from this shape.
    pub(crate) const PRIZE_STATE_V1_BODY_LEN: usize = 57;
    pub(crate) const OBSERVER_TIMELINE_BODY_LEN: usize =
        crate::combat_timeline::ObserverTimelineDirectiveV1::ENCODED_LEN;
    /// `life_seq:u32 | through_sequence:u16`.
    pub(crate) const INPUT_APPLIED_BODY_LEN: usize = 6;
    /// `snapshot_id:u16 | energy:u16`.
    pub(crate) const PRIZE_REFERENCE_BODY_LEN: usize = 4;
    /// Fixed event header (`event_type:u8 | body_len:u8`).
    pub(crate) const HEADER_LEN: usize = 2;

    pub(crate) fn wire_len(&self) -> usize {
        self.wire_len_at(false)
    }

    /// As [`Self::wire_len`], in a frame with [`SNAPSHOT_FLAG_COMPACT_PRIVATE`]
    /// when `compact`.
    pub(crate) fn wire_len_at(&self, compact: bool) -> usize {
        Self::HEADER_LEN
            + match self {
                Self::WatchDamage(_) => Self::WATCH_DAMAGE_BODY_LEN,
                Self::RocketState(_) => Self::ROCKET_STATE_BODY_LEN,
                Self::LifecycleState(lifecycle)
                    if compact && lifecycle.repel_delivery.is_none() =>
                {
                    Self::LIFECYCLE_STATE_COMPACT_BODY_LEN
                }
                Self::LifecycleState(_) => Self::LIFECYCLE_STATE_BODY_LEN,
                Self::PrizeState(_) => Self::PRIZE_STATE_BODY_LEN,
                Self::ObserverTimeline(_) => Self::OBSERVER_TIMELINE_BODY_LEN,
                Self::InputApplied(_) => Self::INPUT_APPLIED_BODY_LEN,
                Self::PrizeReference(_) => Self::PRIZE_REFERENCE_BODY_LEN,
                Self::Unknown { body, .. } => body.len(),
            }
    }

    pub(crate) fn encode_into(&self, bytes: &mut Vec<u8>) {
        self.encode_into_at(bytes, false);
    }

    /// Encodes the event; in a compact frame a lifecycle state without Repel
    /// delivery stops after `repel_until_tick`.
    pub(crate) fn encode_into_at(&self, bytes: &mut Vec<u8>, compact: bool) {
        match self {
            Self::WatchDamage(wd) => {
                push_u8(bytes, Self::TYPE_WATCH_DAMAGE);
                push_u8(bytes, Self::WATCH_DAMAGE_BODY_LEN as u8);
                push_u16(bytes, wd.victim_id);
                push_u16(bytes, wd.shooter_id);
                push_u16(bytes, wd.weapon_data);
                push_u16(bytes, wd.damage);
                push_u16(
                    bytes,
                    wd.energy_before
                        .unwrap_or_else(|| wd.energy_after.saturating_add(wd.damage)),
                );
                push_u16(bytes, wd.energy_after);
                push_u32(bytes, wd.server_tick.unwrap_or(0));
                push_u32(bytes, wd.damage_seq.unwrap_or(0));
                push_u8(bytes, wd.lethal as u8);
                push_u32(bytes, wd.victim_life_seq.unwrap_or(0));
                push_u32(bytes, wd.shooter_life_seq.unwrap_or(0));
                push_u32(bytes, wd.projectile_id.unwrap_or(0));
            }
            Self::RocketState(rocket) => {
                push_u8(bytes, Self::TYPE_ROCKET_STATE);
                push_u8(bytes, Self::ROCKET_STATE_BODY_LEN as u8);
                push_u32(bytes, rocket.life_seq);
                let has_activation =
                    rocket.activation_sequence.is_some() && rocket.until_tick.is_some();
                push_u8(
                    bytes,
                    u8::from(has_activation)
                        | (u8::from(rocket.active) << 1)
                        | (u8::from(rocket.observed_input_sequence.is_some()) << 2)
                        | (u8::from(rocket.input_sequence_watermark.is_some()) << 3),
                );
                push_u16(bytes, rocket.observed_input_sequence.unwrap_or(0));
                push_u16(bytes, rocket.activation_sequence.unwrap_or(0));
                push_u32(bytes, rocket.until_tick.unwrap_or(0));
                push_u8(bytes, rocket.rockets);
                push_u16(bytes, rocket.input_sequence_watermark.unwrap_or(0));
            }
            Self::LifecycleState(lifecycle) => {
                let short = compact && lifecycle.repel_delivery.is_none();
                push_u8(bytes, Self::TYPE_LIFECYCLE_STATE);
                push_u8(
                    bytes,
                    if short {
                        Self::LIFECYCLE_STATE_COMPACT_BODY_LEN
                    } else {
                        Self::LIFECYCLE_STATE_BODY_LEN
                    } as u8,
                );
                push_u32(bytes, lifecycle.life_seq);
                push_u8(
                    bytes,
                    u8::from(lifecycle.alive)
                        | (u8::from(lifecycle.respawn_at_tick.is_some()) << 1)
                        | (u8::from(lifecycle.repel_until_tick.is_some()) << 2)
                        | (u8::from(lifecycle.repel_delivery.is_some()) << 3),
                );
                push_u32(bytes, lifecycle.respawn_at_tick.unwrap_or(0));
                push_u32(bytes, lifecycle.repel_until_tick.unwrap_or(0));
                if short {
                    return;
                }
                let repel = lifecycle.repel_delivery.unwrap_or_default();
                push_u16(bytes, repel.active_steps);
                push_u32(bytes, repel.delivery_generation);
                push_u16(bytes, repel.source_player_id);
                push_u32(bytes, repel.source_life_seq);
                push_u16(bytes, repel.origin_x);
                push_u16(bytes, repel.origin_y);
                push_u16(bytes, repel.source_frequency);
                push_u32(bytes, repel.expected_velocity_x_bits);
                push_u32(bytes, repel.expected_velocity_y_bits);
                push_u16(bytes, repel.weapon_data);
                push_u8(bytes, repel.source_ship);
            }
            Self::PrizeState(prize) => {
                push_u8(bytes, Self::TYPE_PRIZE_STATE);
                push_u8(bytes, Self::PRIZE_STATE_BODY_LEN as u8);
                push_u32(bytes, prize.life_seq);
                push_u32(bytes, prize.revision);
                push_u16(bytes, prize.flags);
                push_u16(bytes, prize.bounty);
                push_u16(bytes, prize.max_energy);
                push_u32(bytes, prize.rotation_bits);
                push_u32(bytes, prize.thrust_bits);
                push_u32(bytes, prize.top_speed_bits);
                push_u32(bytes, prize.recharge_bits);
                push_u8(bytes, prize.gun_level);
                push_u8(bytes, prize.bomb_level);
                push_u8(bytes, prize.shrapnel);
                push_u8(bytes, prize.repels);
                push_u8(bytes, prize.bursts);
                push_u8(bytes, prize.decoys);
                push_u8(bytes, prize.thors);
                push_u8(bytes, prize.bricks);
                push_u8(bytes, prize.portals);
                push_u32(bytes, prize.super_until_tick.unwrap_or(0));
                push_u32(bytes, prize.shield_until_tick.unwrap_or(0));
                push_u32(bytes, prize.engine_shutdown_until_tick.unwrap_or(0));
                push_u32(bytes, prize.damage_loss_seq.unwrap_or(0));
                bytes.extend_from_slice(&prize.damage_loss_prize_id.unwrap_or(0).to_le_bytes());
                // Appended after the v1 body so existing offsets stay stable.
                push_u32(bytes, prize.emp_until_tick.unwrap_or(0));
                // ...and after the v2 body, for the same reason.
                push_u16(bytes, prize.energy.unwrap_or(0));
            }
            Self::ObserverTimeline(directive) => {
                push_u8(bytes, Self::TYPE_OBSERVER_TIMELINE);
                push_u8(bytes, Self::OBSERVER_TIMELINE_BODY_LEN as u8);
                bytes.extend_from_slice(&directive.encode());
            }
            Self::InputApplied(applied) => {
                push_u8(bytes, Self::TYPE_INPUT_APPLIED);
                push_u8(bytes, Self::INPUT_APPLIED_BODY_LEN as u8);
                push_u32(bytes, applied.life_seq);
                push_u16(bytes, applied.through_sequence);
            }
            Self::PrizeReference(reference) => {
                push_u8(bytes, Self::TYPE_PRIZE_REFERENCE);
                push_u8(bytes, Self::PRIZE_REFERENCE_BODY_LEN as u8);
                push_u16(bytes, reference.snapshot_id);
                push_u16(bytes, reference.energy);
            }
            Self::Unknown { event_type, body } => {
                push_u8(bytes, *event_type);
                push_u8(bytes, body.len() as u8);
                bytes.extend_from_slice(body);
            }
        }
    }

    pub(crate) fn parse_at(packet: &[u8], offset: usize) -> Result<(Self, usize), String> {
        if offset + Self::HEADER_LEN > packet.len() {
            return Err(format!(
                "snapshot event header truncated at offset {offset}"
            ));
        }
        let event_type = packet[offset];
        let body_len = packet[offset + 1] as usize;
        let body_start = offset + Self::HEADER_LEN;
        let body_end = body_start + body_len;
        if body_end > packet.len() {
            return Err(format!(
                "snapshot event body truncated: end {body_end} > packet {}",
                packet.len()
            ));
        }
        match event_type {
            Self::TYPE_WATCH_DAMAGE => {
                if body_len < Self::WATCH_DAMAGE_V1_BODY_LEN {
                    return Err(format!(
                        "watch_damage body too short: {body_len} < {}",
                        Self::WATCH_DAMAGE_V1_BODY_LEN
                    ));
                }
                let victim_id = read_u16(packet, body_start)?;
                let shooter_id = read_u16(packet, body_start + 2)?;
                let weapon_data = read_u16(packet, body_start + 4)?;
                let damage = read_u16(packet, body_start + 6)?;
                let (
                    energy_before,
                    energy_after,
                    server_tick,
                    damage_seq,
                    projectile_id,
                    lethal,
                    victim_life_seq,
                    shooter_life_seq,
                ) = if body_len >= Self::WATCH_DAMAGE_BODY_LEN {
                    (
                        Some(read_u16(packet, body_start + 8)?),
                        read_u16(packet, body_start + 10)?,
                        Some(read_u32(packet, body_start + 12)?),
                        Some(read_u32(packet, body_start + 16)?),
                        Some(read_u32(packet, body_start + 29)?),
                        packet[body_start + 20] != 0,
                        Some(read_u32(packet, body_start + 21)?),
                        Some(read_u32(packet, body_start + 25)?),
                    )
                } else if body_len >= 29 {
                    (
                        Some(read_u16(packet, body_start + 8)?),
                        read_u16(packet, body_start + 10)?,
                        Some(read_u32(packet, body_start + 12)?),
                        Some(read_u32(packet, body_start + 16)?),
                        None,
                        packet[body_start + 20] != 0,
                        Some(read_u32(packet, body_start + 21)?),
                        Some(read_u32(packet, body_start + 25)?),
                    )
                } else if body_len >= 21 {
                    (
                        Some(read_u16(packet, body_start + 8)?),
                        read_u16(packet, body_start + 10)?,
                        Some(read_u32(packet, body_start + 12)?),
                        Some(read_u32(packet, body_start + 16)?),
                        None,
                        packet[body_start + 20] != 0,
                        None,
                        None,
                    )
                } else {
                    (
                        None,
                        read_u16(packet, body_start + 8)?,
                        None,
                        None,
                        None,
                        packet[body_start + 10] != 0,
                        None,
                        None,
                    )
                };
                let event = Self::WatchDamage(SnapshotWatchDamage {
                    victim_id,
                    shooter_id,
                    victim_life_seq,
                    shooter_life_seq,
                    weapon_data,
                    damage,
                    energy_before,
                    energy_after,
                    server_tick,
                    damage_seq,
                    projectile_id,
                    lethal,
                });
                Ok((event, body_end - offset))
            }
            Self::TYPE_ROCKET_STATE => {
                if body_len != Self::ROCKET_STATE_BODY_LEN {
                    return Err(format!(
                        "rocket_state body length mismatch: {body_len} != {}",
                        Self::ROCKET_STATE_BODY_LEN
                    ));
                }
                let life_seq = read_u32(packet, body_start)?;
                let flags = packet[body_start + 4];
                if flags & !0b1111 != 0 || flags & 0b10 != 0 && flags & 0b01 == 0 {
                    return Err(format!("invalid rocket_state flags: {flags:#04x}"));
                }
                let has_activation = flags & 0b01 != 0;
                let event = Self::RocketState(SnapshotRocketState {
                    life_seq,
                    observed_input_sequence: (flags & 0b100 != 0)
                        .then(|| read_u16(packet, body_start + 5))
                        .transpose()?,
                    input_sequence_watermark: (flags & 0b1000 != 0)
                        .then(|| read_u16(packet, body_start + 14))
                        .transpose()?,
                    activation_sequence: has_activation
                        .then(|| read_u16(packet, body_start + 7))
                        .transpose()?,
                    until_tick: has_activation
                        .then(|| read_u32(packet, body_start + 9))
                        .transpose()?,
                    active: flags & 0b10 != 0,
                    rockets: packet[body_start + 13],
                });
                Ok((event, body_end - offset))
            }
            Self::TYPE_LIFECYCLE_STATE => {
                if body_len != Self::LIFECYCLE_STATE_BODY_LEN
                    && body_len != Self::LIFECYCLE_STATE_COMPACT_BODY_LEN
                {
                    return Err(format!(
                        "lifecycle_state body length mismatch: {body_len} != {}",
                        Self::LIFECYCLE_STATE_BODY_LEN
                    ));
                }
                let life_seq = read_u32(packet, body_start)?;
                let flags = packet[body_start + 4];
                if flags & !0b1111 != 0
                    || flags & 0b01 != 0 && flags & 0b10 != 0
                    || flags & 0b01 == 0 && flags & 0b100 != 0
                    || flags & 0b1000 != 0 && flags & 0b01 == 0
                    || flags & 0b1000 != 0 && body_len != Self::LIFECYCLE_STATE_BODY_LEN
                {
                    return Err(format!("invalid lifecycle_state flags: {flags:#04x}"));
                }
                let repel_delivery = (flags & 0b1000 != 0)
                    .then(|| {
                        Ok::<_, String>(SnapshotRepelDelivery {
                            active_steps: read_u16(packet, body_start + 13)?,
                            delivery_generation: read_u32(packet, body_start + 15)?,
                            source_player_id: read_u16(packet, body_start + 19)?,
                            source_life_seq: read_u32(packet, body_start + 21)?,
                            origin_x: read_u16(packet, body_start + 25)?,
                            origin_y: read_u16(packet, body_start + 27)?,
                            source_frequency: read_u16(packet, body_start + 29)?,
                            expected_velocity_x_bits: read_u32(packet, body_start + 31)?,
                            expected_velocity_y_bits: read_u32(packet, body_start + 35)?,
                            weapon_data: read_u16(packet, body_start + 39)?,
                            source_ship: packet[body_start + 41],
                        })
                    })
                    .transpose()?;
                if repel_delivery.is_some_and(|delivery| {
                    delivery.active_steps == 0
                        || delivery.delivery_generation == 0
                        || delivery.source_life_seq == 0
                        || crate::continuum_weapon::continuum_weapon_type(delivery.weapon_data) != 5
                        || delivery.source_ship >= 8
                        || !f32::from_bits(delivery.expected_velocity_x_bits).is_finite()
                        || !f32::from_bits(delivery.expected_velocity_y_bits).is_finite()
                }) {
                    return Err("invalid lifecycle Repel delivery identity".to_owned());
                }
                let event = Self::LifecycleState(SnapshotLifecycleState {
                    life_seq,
                    alive: flags & 0b01 != 0,
                    respawn_at_tick: (flags & 0b10 != 0)
                        .then(|| read_u32(packet, body_start + 5))
                        .transpose()?,
                    repel_until_tick: (flags & 0b100 != 0)
                        .then(|| read_u32(packet, body_start + 9))
                        .transpose()?,
                    repel_delivery,
                });
                Ok((event, body_end - offset))
            }
            Self::TYPE_PRIZE_STATE => {
                if body_len != Self::PRIZE_STATE_BODY_LEN
                    && body_len != Self::PRIZE_STATE_V2_BODY_LEN
                    && body_len != Self::PRIZE_STATE_V1_BODY_LEN
                {
                    return Err(format!(
                        "prize_state body length mismatch: {body_len} != {}",
                        Self::PRIZE_STATE_BODY_LEN
                    ));
                }
                let life_seq = read_u32(packet, body_start)?;
                let revision = read_u32(packet, body_start + 4)?;
                let flags = read_u16(packet, body_start + 8)?;
                const KNOWN_FLAGS: u16 = (1 << 15) - 1;
                if life_seq == 0 || revision == 0 || flags & !KNOWN_FLAGS != 0 {
                    return Err("invalid prize_state identity or flags".to_owned());
                }
                if flags & (1 << 11) != 0 && flags & (1 << 9) == 0 {
                    return Err("pending prize_state Shutdown lacks deadline".to_owned());
                }
                let rotation_bits = read_u32(packet, body_start + 14)?;
                let thrust_bits = read_u32(packet, body_start + 18)?;
                let top_speed_bits = read_u32(packet, body_start + 22)?;
                let recharge_bits = read_u32(packet, body_start + 26)?;
                if [rotation_bits, thrust_bits, top_speed_bits, recharge_bits]
                    .into_iter()
                    .any(|bits| !f32::from_bits(bits).is_finite())
                {
                    return Err("non-finite prize_state movement value".to_owned());
                }
                let has_damage_loss = flags & (1 << 10) != 0;
                let loss_seq = read_u32(packet, body_start + 51)?;
                let loss_id =
                    i16::from_le_bytes([packet[body_start + 55], packet[body_start + 56]]);
                if (has_damage_loss && (loss_seq == 0 || !(0..=28).contains(&loss_id)))
                    || (!has_damage_loss && (loss_seq != 0 || loss_id != 0))
                {
                    return Err("invalid prize_state damage-loss identity".to_owned());
                }
                let event = Self::PrizeState(SnapshotPrizeState {
                    life_seq,
                    revision,
                    flags,
                    bounty: read_u16(packet, body_start + 10)?,
                    max_energy: read_u16(packet, body_start + 12)?,
                    rotation_bits,
                    thrust_bits,
                    top_speed_bits,
                    recharge_bits,
                    gun_level: packet[body_start + 30],
                    bomb_level: packet[body_start + 31],
                    shrapnel: packet[body_start + 32],
                    repels: packet[body_start + 33],
                    bursts: packet[body_start + 34],
                    decoys: packet[body_start + 35],
                    thors: packet[body_start + 36],
                    bricks: packet[body_start + 37],
                    portals: packet[body_start + 38],
                    super_until_tick: (flags & (1 << 7) != 0)
                        .then(|| read_u32(packet, body_start + 39))
                        .transpose()?,
                    shield_until_tick: (flags & (1 << 8) != 0)
                        .then(|| read_u32(packet, body_start + 43))
                        .transpose()?,
                    engine_shutdown_until_tick: (flags & (1 << 9) != 0)
                        .then(|| read_u32(packet, body_start + 47))
                        .transpose()?,
                    emp_until_tick: (body_len >= Self::PRIZE_STATE_V2_BODY_LEN
                        && flags & (1 << 13) != 0)
                        .then(|| read_u32(packet, body_start + 57))
                        .transpose()?,
                    energy: (body_len >= Self::PRIZE_STATE_BODY_LEN)
                        .then(|| read_u16(packet, body_start + 61))
                        .transpose()?,
                    damage_loss_seq: has_damage_loss.then_some(loss_seq),
                    damage_loss_prize_id: has_damage_loss.then_some(loss_id),
                });
                Ok((event, body_end - offset))
            }
            Self::TYPE_OBSERVER_TIMELINE => {
                if body_len != Self::OBSERVER_TIMELINE_BODY_LEN {
                    return Err(format!(
                        "observer_timeline body length mismatch: {body_len} != {}",
                        Self::OBSERVER_TIMELINE_BODY_LEN
                    ));
                }
                let directive = crate::combat_timeline::ObserverTimelineDirectiveV1::decode(
                    &packet[body_start..body_end],
                )
                .map_err(|error| format!("invalid observer_timeline: {error:?}"))?;
                Ok((Self::ObserverTimeline(directive), body_end - offset))
            }
            Self::TYPE_PRIZE_REFERENCE => {
                if body_len != Self::PRIZE_REFERENCE_BODY_LEN {
                    return Err(format!(
                        "prize_reference body length mismatch: {body_len} != {}",
                        Self::PRIZE_REFERENCE_BODY_LEN
                    ));
                }
                let event = Self::PrizeReference(SnapshotPrizeReference {
                    snapshot_id: read_u16(packet, body_start)?,
                    energy: read_u16(packet, body_start + 2)?,
                });
                Ok((event, body_end - offset))
            }
            Self::TYPE_INPUT_APPLIED => {
                if body_len != Self::INPUT_APPLIED_BODY_LEN {
                    return Err(format!(
                        "input_applied body length mismatch: {body_len} != {}",
                        Self::INPUT_APPLIED_BODY_LEN
                    ));
                }
                let event = Self::InputApplied(SnapshotInputApplied {
                    life_seq: read_u32(packet, body_start)?,
                    through_sequence: read_u16(packet, body_start + 4)?,
                });
                Ok((event, body_end - offset))
            }
            _ => {
                // Forward-compat: skip past unknown types via body_len.
                let body = packet[body_start..body_end].to_vec();
                Ok((Self::Unknown { event_type, body }, body_end - offset))
            }
        }
    }
}

/// Full decoded snapshot frame: header metadata + per-tier entity
/// lists + per-snapshot events. Events were reserved by Phase 3e-1
/// and started carrying payload in 3e-5.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotFrame {
    pub(crate) header: SnapshotHeader,
    pub(crate) near_entities: Vec<SnapshotEntity>,
    pub(crate) mid_entities: Vec<SnapshotEntity>,
    pub(crate) events: Vec<SnapshotEvent>,
}

impl SnapshotFrame {
    /// The frame tick that compact entity sample times are relative to.
    fn compact_sample_tick(header: SnapshotHeader) -> Option<u32> {
        (header.flags & SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME != 0)
            .then_some(header.tick & crate::continuum::CONTINUUM_TICK_MASK)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let compact_tick = Self::compact_sample_tick(self.header);
        let compact_private = self.header.flags & SNAPSHOT_FLAG_COMPACT_PRIVATE != 0;
        let body_len: usize = self
            .near_entities
            .iter()
            .map(|e| e.wire_len_at(compact_tick))
            .sum::<usize>()
            + self
                .mid_entities
                .iter()
                .map(|e| e.wire_len_at(compact_tick))
                .sum::<usize>()
            + self
                .events
                .iter()
                .map(|e| e.wire_len_at(compact_private))
                .sum::<usize>();
        let mut bytes = Vec::with_capacity(SnapshotHeader::HEADER_LEN + body_len);
        let header = SnapshotHeader {
            near_count: self.near_entities.len() as u8,
            mid_count: self.mid_entities.len() as u8,
            event_count: self.events.len() as u8,
            ..self.header
        };
        header.encode_into(&mut bytes);
        for entity in &self.near_entities {
            entity.encode_into_at(&mut bytes, compact_tick);
        }
        for entity in &self.mid_entities {
            entity.encode_into_at(&mut bytes, compact_tick);
        }
        for event in &self.events {
            event.encode_into_at(&mut bytes, compact_private);
        }
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        let header = SnapshotHeader::parse(packet)?;
        let near_count = header.near_count as usize;
        let mid_count = header.mid_count as usize;
        let event_count = header.event_count as usize;
        let compact_tick = Self::compact_sample_tick(header);
        let mut offset = SnapshotHeader::HEADER_LEN;
        let mut near_entities = Vec::with_capacity(near_count);
        for _ in 0..near_count {
            let (entity, consumed) =
                SnapshotEntity::parse_at_compact(packet, offset, compact_tick)?;
            offset += consumed;
            near_entities.push(entity);
        }
        let mut mid_entities = Vec::with_capacity(mid_count);
        for _ in 0..mid_count {
            let (entity, consumed) =
                SnapshotEntity::parse_at_compact(packet, offset, compact_tick)?;
            offset += consumed;
            mid_entities.push(entity);
        }
        let mut events = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            let (event, consumed) = SnapshotEvent::parse_at(packet, offset)?;
            offset += consumed;
            events.push(event);
        }
        Ok(Self {
            header,
            near_entities,
            mid_entities,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn entity(id: u16, x: u16, y: u16) -> SnapshotEntity {
        SnapshotEntity::full(id, x, y)
    }

    #[test]
    fn empty_snapshot_round_trips() {
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: 0,
                snapshot_id: 0x1234,
                base_id: 0x1233,
                tick: 0x0001_0203,
                arena_stream_id: 7,
                near_count: 0,
                mid_count: 0,
                event_count: 0,
            },
            near_entities: vec![],
            mid_entities: vec![],
            events: vec![],
        };
        let bytes = frame.encode();
        assert_eq!(bytes.len(), SnapshotHeader::HEADER_LEN);
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert_eq!(parsed, frame);
    }

    #[test]
    fn snapshot_with_near_entities_round_trips() {
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 1,
                base_id: 0,
                tick: 99,
                arena_stream_id: 7,
                near_count: 0, // overridden by encode()
                mid_count: 0,
                event_count: 0,
            },
            near_entities: vec![entity(1, 100, 200), entity(2, 300, 400)],
            mid_entities: vec![],
            events: vec![],
        };
        let bytes = frame.encode();
        // Each x+y entity is 4 (header: id+mask) + 4 (x + y) = 8 bytes.
        assert_eq!(bytes.len(), SnapshotHeader::HEADER_LEN + 2 * 8);
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.header.near_count, 2);
        assert_eq!(parsed.header.mid_count, 0);
        assert_eq!(parsed.near_entities, frame.near_entities);
        assert_eq!(parsed.mid_entities, vec![]);
    }

    #[test]
    fn delta_entity_with_no_fields_is_four_bytes() {
        // Phase 3e-7: header widened to id:u16 + mask:u16 = 4 bytes.
        let e = SnapshotEntity::unchanged(42);
        assert_eq!(e.wire_len(), 4);
        let mut bytes = Vec::new();
        e.encode_into(&mut bytes);
        assert_eq!(bytes, vec![42, 0, 0, 0]);
        let (parsed, consumed) = SnapshotEntity::parse_at(&bytes, 0).unwrap();
        assert_eq!(parsed, e);
        assert_eq!(consumed, 4);
    }

    #[test]
    fn delta_entity_with_only_x_is_six_bytes() {
        // Phase 3e-7: header is now id(2) + mask(2) = 4 bytes. With
        // bit 0 set we add 2 bytes for x → 6 bytes total.
        let e = SnapshotEntity {
            entity_id: 7,
            field_mask: SnapshotEntity::FIELD_X,
            x: Some(0x1234),
            ..SnapshotEntity::default()
        };
        assert_eq!(e.wire_len(), 6);
        let mut bytes = Vec::new();
        e.encode_into(&mut bytes);
        let (parsed, consumed) = SnapshotEntity::parse_at(&bytes, 0).unwrap();
        assert_eq!(parsed, e);
        assert_eq!(consumed, 6);
    }

    #[test]
    fn snapshot_with_mixed_mask_entities_round_trips() {
        // Realistic delta: some entities unchanged, some moved.
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: 0,
                snapshot_id: 50,
                base_id: 45, // delta against snapshot 45
                tick: 999,
                arena_stream_id: 7,
                near_count: 0,
                mid_count: 0,
                event_count: 0,
            },
            near_entities: vec![
                SnapshotEntity::unchanged(1),
                SnapshotEntity::full(2, 100, 200),
            ],
            mid_entities: vec![SnapshotEntity::unchanged(3)],
            events: vec![],
        };
        let bytes = frame.encode();
        // 21 header + 4 (unchanged) + 8 (full x+y) + 4 (unchanged) = 37.
        assert_eq!(bytes.len(), 37);
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.near_entities, frame.near_entities);
        assert_eq!(parsed.mid_entities, frame.mid_entities);
    }

    #[test]
    fn snapshot_with_near_and_mid_entities_round_trips() {
        let frame = SnapshotFrame {
            header: SnapshotHeader::default(),
            near_entities: vec![entity(1, 10, 20), entity(2, 30, 40)],
            mid_entities: vec![entity(3, 1000, 2000)],
            events: vec![],
        };
        let bytes = frame.encode();
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.header.near_count, 2);
        assert_eq!(parsed.header.mid_count, 1);
        assert_eq!(parsed.near_entities, frame.near_entities);
        assert_eq!(parsed.mid_entities, frame.mid_entities);
    }

    #[test]
    fn snapshot_full_flag_signals_full_state() {
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 1,
                base_id: 0,
                tick: 99,
                arena_stream_id: 7,
                near_count: 0,
                mid_count: 0,
                event_count: 0,
            },
            near_entities: vec![],
            mid_entities: vec![],
            events: vec![],
        };
        let bytes = frame.encode();
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert!(parsed.header.is_full());
    }

    #[test]
    fn full_snapshot_rejects_incomplete_near_entity() {
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 1,
                ..SnapshotHeader::default()
            },
            near_entities: vec![SnapshotEntity::full(7, 100, 200)],
            ..SnapshotFrame::default()
        };

        let error = delta_apply(None, &frame).unwrap_err();
        assert!(error.contains("lacks required fields"), "{error}");
    }

    #[test]
    fn delta_snapshot_rejects_incomplete_new_near_entity() {
        let base = SnapshotEntityStateMap::new();
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 2,
                base_id: 1,
                ..SnapshotHeader::default()
            },
            near_entities: vec![SnapshotEntity::full(7, 100, 200)],
            ..SnapshotFrame::default()
        };

        let error = delta_apply(Some(&base), &frame).unwrap_err();
        assert!(error.contains("lacks required fields"), "{error}");
    }

    #[test]
    fn rejects_wrong_type_byte() {
        let mut bytes = SnapshotFrame::default().encode();
        bytes[0] = 0x71;
        assert!(SnapshotFrame::parse(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_packet() {
        let bytes = vec![0x70, 0x01, 0x00, 0x00, 0x00];
        assert!(SnapshotFrame::parse(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_body() {
        // Header claims 2 entities; truncate to only the first entity's
        // bytes (8 = 4 header + 4 payload for x+y).
        let frame = SnapshotFrame {
            header: SnapshotHeader::default(),
            near_entities: vec![entity(1, 10, 20), entity(2, 30, 40)],
            mid_entities: vec![],
            events: vec![],
        };
        let mut bytes = frame.encode();
        bytes.truncate(SnapshotHeader::HEADER_LEN + 8);
        assert!(SnapshotFrame::parse(&bytes).is_err());
    }

    #[test]
    fn snapshot_with_watch_damage_event_round_trips() {
        let event = SnapshotEvent::WatchDamage(SnapshotWatchDamage {
            victim_id: 7,
            shooter_id: 9,
            weapon_data: 0x1234,
            damage: 250,
            energy_before: Some(1350),
            energy_after: 1100,
            server_tick: Some(55),
            damage_seq: Some(66),
            projectile_id: Some(99),
            victim_life_seq: Some(3),
            shooter_life_seq: Some(4),
            lethal: false,
        });
        let frame = SnapshotFrame {
            header: SnapshotHeader::default(),
            near_entities: vec![],
            mid_entities: vec![],
            events: vec![event.clone()],
        };
        let bytes = frame.encode();
        // Generation-bound header + event header + extended damage body.
        assert_eq!(
            bytes.len(),
            SnapshotHeader::HEADER_LEN + 2 + SnapshotEvent::WATCH_DAMAGE_BODY_LEN
        );
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.header.event_count, 1);
        assert_eq!(parsed.events, vec![event]);
    }

    #[test]
    fn snapshot_with_private_rocket_state_round_trips_wrap_values() {
        let event = SnapshotEvent::RocketState(SnapshotRocketState {
            life_seq: u32::MAX,
            observed_input_sequence: Some(u16::MAX - 1),
            input_sequence_watermark: Some(1),
            activation_sequence: Some(u16::MAX),
            until_tick: Some(2),
            active: true,
            rockets: 4,
        });
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: u16::MAX,
                tick: 0x7fff_fffe,
                ..SnapshotHeader::default()
            },
            events: vec![event.clone()],
            ..SnapshotFrame::default()
        };
        let bytes = frame.encode();
        assert_eq!(
            bytes.len(),
            SnapshotHeader::HEADER_LEN
                + SnapshotEvent::HEADER_LEN
                + SnapshotEvent::ROCKET_STATE_BODY_LEN
        );
        assert_eq!(SnapshotFrame::parse(&bytes).unwrap().events, vec![event]);
    }

    #[test]
    fn observer_timeline_event_round_trips_strict_versioned_directive() {
        let directive = crate::combat_timeline::ObserverTimelineDirectiveV1 {
            generation: u32::MAX,
            issued_server_tick: u32::MAX - 5,
            effective_server_tick: 3,
            effective_from_input_sequence: u16::MAX,
            from_delay_ticks: 7,
            target_delay_ticks: 9,
            ceiling_delay_ticks: 9,
            slew_interval_ticks: crate::combat_timeline::OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
            hold_grace_ticks: crate::combat_timeline::OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
            profile: crate::combat_timeline::ObserverNetworkProfile::CleanLan,
        };
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 1,
                event_count: 1,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::ObserverTimeline(directive)],
            ..SnapshotFrame::default()
        };
        let encoded = frame.encode();
        assert_eq!(SnapshotFrame::parse(&encoded), Ok(frame));

        let event_offset = SnapshotHeader::HEADER_LEN;
        let version_offset = event_offset + SnapshotEvent::HEADER_LEN;
        let mut wrong_version = encoded;
        wrong_version[version_offset] =
            crate::combat_timeline::COMBAT_VIEW_PROTOCOL_VERSION.wrapping_add(1);
        assert!(
            SnapshotFrame::parse(&wrong_version)
                .unwrap_err()
                .contains("invalid observer_timeline")
        );
    }

    #[test]
    fn compact_frames_carry_sample_times_in_one_byte_and_escape_the_rest() {
        let mask = crate::continuum::CONTINUUM_TICK_MASK;
        // Frame tick 1 just after the 31-bit wrap.
        let frame_tick = 1;
        let sample =
            |tick: u32, third: u8| PresentationSampleTime::from_parts(tick, third).unwrap();
        let samples = [
            sample(1, 0),         // this tick
            sample(0, 2),         // one tick behind
            sample(mask - 2, 1),  // across the wrap, 4 behind
            sample(mask - 82, 2), // 84 behind, phase 2: byte 254
            sample(mask - 83, 0), // 85 behind: full form
            sample(5, 1),         // ahead of the frame: full form
        ];
        let entity = |id: u16, sample_time| SnapshotEntity {
            entity_id: id,
            field_mask: SnapshotEntity::FIELD_X | SnapshotEntity::FIELD_SAMPLE_TIME,
            x: Some(100 + id),
            sample_time: Some(sample_time),
            ..SnapshotEntity::default()
        };
        let frame = |flags| SnapshotFrame {
            header: SnapshotHeader {
                flags,
                snapshot_id: 3,
                tick: frame_tick,
                near_count: samples.len() as u8,
                ..SnapshotHeader::default()
            },
            near_entities: samples
                .iter()
                .enumerate()
                .map(|(id, sample)| entity(id as u16, *sample))
                .collect(),
            ..SnapshotFrame::default()
        };
        let plain = frame(SNAPSHOT_FLAG_FULL);
        let compact = frame(SNAPSHOT_FLAG_FULL | SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME);
        let plain_bytes = plain.encode();
        let compact_bytes = compact.encode();
        // Four samples save four bytes each; the two escapes cost one more.
        assert_eq!(plain_bytes.len() - compact_bytes.len(), 4 * 4 - 2);
        assert_eq!(SnapshotFrame::parse(&plain_bytes), Ok(plain));
        assert_eq!(SnapshotFrame::parse(&compact_bytes), Ok(compact.clone()));
        // The fourth entity's byte is 84 * 3 + 2.
        let fourth = SnapshotHeader::HEADER_LEN + 3 * (4 + 2 + 1) + 4 + 2;
        assert_eq!(compact_bytes[fourth], 254);

        // A truncated compact field is an error, not a shifted read.
        let mut truncated = compact_bytes;
        truncated.truncate(SnapshotHeader::HEADER_LEN + 4 + 2);
        assert!(SnapshotFrame::parse(&truncated).is_err());
    }

    #[test]
    fn position_offsets_shrink_moves_against_the_base_and_resolve_exactly() {
        let mut base = SnapshotEntityStateMap::new();
        let at = |x, y| SnapshotEntityState {
            x,
            y,
            ..SnapshotEntityState::default()
        };
        base.insert(1, at(1_000, 2_000));
        base.insert(2, at(1_000, 2_000));
        base.insert(3, at(0, 65_535));
        let current = vec![
            (1, at(1_127, 1_872)), // +127, -128: the extremes fit
            (2, at(1_128, 2_000)), // +128 does not fit: absolute
            (3, at(5, 65_530)),
            (4, at(9, 9)), // not in the base: absolute
        ];
        let mut entities = delta_encode_near_tier(&current, Some(&base));
        let absolute: usize = entities.iter().map(SnapshotEntity::wire_len).sum();
        offset_positions(&mut entities, &base);
        let offsets = |id: u16| {
            entities[usize::from(id - 1)].field_mask & SNAPSHOT_FIELD_POSITION_OFFSET != 0
        };
        assert!(offsets(1) && !offsets(2) && offsets(3) && !offsets(4));
        let compact: usize = entities.iter().map(SnapshotEntity::wire_len).sum();
        assert_eq!(absolute - compact, 2 * 2, "two entries save two bytes each");

        let frame = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 2,
                base_id: 1,
                near_count: 4,
                ..SnapshotHeader::default()
            },
            near_entities: entities,
            ..SnapshotFrame::default()
        };
        let parsed = SnapshotFrame::parse(&frame.encode()).unwrap();
        let applied = delta_apply(Some(&base), &parsed).unwrap();
        for (id, state) in &current {
            assert_eq!(
                (applied.entities[id].x, applied.entities[id].y),
                (state.x, state.y)
            );
        }
        // Without its base an offset cannot be resolved.
        let mut lone = SnapshotEntityStateMap::new();
        lone.insert(2, at(0, 0));
        assert!(delta_apply(Some(&lone), &parsed).is_err());
    }

    #[test]
    fn compact_private_frames_shorten_lifecycle_and_carry_prize_references() {
        let resting = SnapshotLifecycleState {
            life_seq: 9,
            alive: true,
            repel_until_tick: Some(77),
            ..SnapshotLifecycleState::default()
        };
        let repelling = SnapshotLifecycleState {
            repel_delivery: Some(SnapshotRepelDelivery {
                active_steps: 3,
                delivery_generation: 4,
                source_player_id: 5,
                source_life_seq: 6,
                weapon_data: 5,
                source_ship: 1,
                ..SnapshotRepelDelivery::default()
            }),
            ..resting
        };
        let reference = SnapshotEvent::PrizeReference(SnapshotPrizeReference {
            snapshot_id: 0xfffe,
            energy: 1_234,
        });
        let frame = |flags, lifecycle| SnapshotFrame {
            header: SnapshotHeader {
                flags,
                snapshot_id: 2,
                event_count: 2,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::LifecycleState(lifecycle), reference.clone()],
            ..SnapshotFrame::default()
        };
        let compact = SNAPSHOT_FLAG_FULL | SNAPSHOT_FLAG_COMPACT_PRIVATE;
        for (flags, lifecycle, body) in [
            (SNAPSHOT_FLAG_FULL, resting, 42),
            (compact, resting, 13),
            (compact, repelling, 42),
        ] {
            let frame = frame(flags, lifecycle);
            let bytes = frame.encode();
            assert_eq!(bytes[SnapshotHeader::HEADER_LEN + 1], body);
            assert_eq!(
                bytes.len(),
                SnapshotHeader::HEADER_LEN + 2 + body as usize + 2 + 4
            );
            assert_eq!(SnapshotFrame::parse(&bytes), Ok(frame));
        }
        // A short lifecycle body cannot claim a Repel delivery.
        let mut forged = frame(compact, resting).encode();
        forged[SnapshotHeader::HEADER_LEN + 2 + 4] |= 0b1000;
        assert!(SnapshotFrame::parse(&forged).is_err());
    }

    #[test]
    fn input_applied_event_round_trips_and_is_eight_bytes() {
        let applied = SnapshotEvent::InputApplied(SnapshotInputApplied {
            life_seq: u32::MAX,
            through_sequence: 0xfffe,
        });
        assert_eq!(applied.wire_len(), 8);
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 1,
                event_count: 1,
                ..SnapshotHeader::default()
            },
            events: vec![applied],
            ..SnapshotFrame::default()
        };
        let encoded = frame.encode();
        assert_eq!(SnapshotFrame::parse(&encoded), Ok(frame));

        let mut short = encoded;
        short[SnapshotHeader::HEADER_LEN + 1] = 5;
        short.pop();
        assert!(SnapshotFrame::parse(&short).is_err());
    }

    #[test]
    fn snapshot_with_private_lifecycle_state_round_trips_deadline_and_wrap() {
        let event = SnapshotEvent::LifecycleState(SnapshotLifecycleState {
            life_seq: u32::MAX,
            alive: false,
            respawn_at_tick: Some(7),
            repel_until_tick: None,
            repel_delivery: None,
        });
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: u16::MAX,
                base_id: 0,
                tick: u32::MAX - 2,
                ..SnapshotHeader::default()
            },
            events: vec![event.clone()],
            ..SnapshotFrame::default()
        };
        let bytes = frame.encode();
        assert_eq!(SnapshotFrame::parse(&bytes).unwrap().events, vec![event]);

        let alive = SnapshotEvent::LifecycleState(SnapshotLifecycleState {
            life_seq: 1,
            alive: true,
            respawn_at_tick: None,
            repel_until_tick: Some(u32::MAX),
            repel_delivery: Some(SnapshotRepelDelivery {
                active_steps: 135,
                delivery_generation: 7,
                source_player_id: 9,
                source_life_seq: 11,
                origin_x: 500,
                origin_y: 700,
                source_frequency: 2,
                expected_velocity_x_bits: 500.25_f32.to_bits(),
                expected_velocity_y_bits: (-17.5_f32).to_bits(),
                weapon_data: 5,
                source_ship: 3,
            }),
        });
        let frame = SnapshotFrame {
            events: vec![alive.clone()],
            ..SnapshotFrame::default()
        };
        assert_eq!(
            SnapshotFrame::parse(&frame.encode()).unwrap().events,
            vec![alive]
        );
    }

    #[test]
    fn lifecycle_state_rejects_invalid_flags_and_lengths() {
        let frame = SnapshotFrame {
            events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 9,
                alive: false,
                respawn_at_tick: Some(10),
                repel_until_tick: None,
                repel_delivery: None,
            })],
            ..SnapshotFrame::default()
        };
        let mut bytes = frame.encode();
        let event = SnapshotHeader::HEADER_LEN;
        bytes[event + 1] = (SnapshotEvent::LIFECYCLE_STATE_BODY_LEN - 1) as u8;
        assert!(SnapshotFrame::parse(&bytes).is_err());

        let mut bytes = frame.encode();
        bytes[event + SnapshotEvent::HEADER_LEN + 4] = 0b11;
        assert!(SnapshotFrame::parse(&bytes).is_err());

        let mut bytes = frame.encode();
        bytes[event + SnapshotEvent::HEADER_LEN + 4] = 0b110;
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "a dead lifecycle cannot advertise an active Repel deadline"
        );

        let mut bytes = frame.encode();
        bytes[event + SnapshotEvent::HEADER_LEN + 4] = 0b1001;
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "logical Repel steps require the matching deadline flag"
        );

        let mut bytes = frame.encode();
        bytes[event + SnapshotEvent::HEADER_LEN + 4] = 0b1101;
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "a present logical Repel interval cannot contain zero steps"
        );

        let private = SnapshotFrame {
            events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 9,
                alive: true,
                respawn_at_tick: None,
                repel_until_tick: Some(325),
                repel_delivery: Some(SnapshotRepelDelivery {
                    active_steps: 135,
                    delivery_generation: 1,
                    source_player_id: 7,
                    source_life_seq: 3,
                    origin_x: 500,
                    origin_y: 700,
                    source_frequency: 2,
                    expected_velocity_x_bits: 500.0_f32.to_bits(),
                    expected_velocity_y_bits: 0.0_f32.to_bits(),
                    weapon_data: 5,
                    source_ship: 3,
                }),
            })],
            ..SnapshotFrame::default()
        };
        let mut bytes = private.encode();
        let body = event + SnapshotEvent::HEADER_LEN;
        bytes[body + 39..body + 41].copy_from_slice(&0_u16.to_le_bytes());
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "the private lifecycle record accepts only an exact Repel weapon"
        );
        let mut bytes = private.encode();
        bytes[body + 41] = 8;
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "the immutable source ship must identify real ship tuning"
        );
    }

    #[test]
    fn rocket_state_rejects_active_without_activation_and_bad_length() {
        let mut bytes = SnapshotFrame {
            events: vec![SnapshotEvent::RocketState(SnapshotRocketState {
                life_seq: 1,
                observed_input_sequence: None,
                input_sequence_watermark: Some(64),
                activation_sequence: None,
                until_tick: None,
                active: false,
                rockets: 3,
            })],
            ..SnapshotFrame::default()
        }
        .encode();
        // event header begins immediately after the 13-byte frame header;
        // flags are four bytes into the Rocket body.
        bytes[SnapshotHeader::HEADER_LEN + SnapshotEvent::HEADER_LEN + 4] = 0b10;
        assert!(SnapshotFrame::parse(&bytes).is_err());

        let mut bytes = SnapshotFrame {
            events: vec![SnapshotEvent::RocketState(SnapshotRocketState::default())],
            ..SnapshotFrame::default()
        }
        .encode();
        bytes[SnapshotHeader::HEADER_LEN + 1] = 11;
        assert!(SnapshotFrame::parse(&bytes).is_err());
    }

    #[test]
    fn private_prize_state_round_trips_raw_stats_deadlines_and_damage_loss() {
        let prize = SnapshotEvent::PrizeState(SnapshotPrizeState {
            life_seq: u32::MAX,
            revision: u32::MAX,
            flags: 0b111_1111
                | (1 << 7)
                | (1 << 8)
                | (1 << 9)
                | (1 << 10)
                | (1 << 11)
                | (1 << 13)
                | SnapshotPrizeState::MAXED_SPAWN_LOADOUT,
            bounty: 77,
            max_energy: 1_234,
            rotation_bits: 2.5_f32.to_bits(),
            thrust_bits: 3.25_f32.to_bits(),
            top_speed_bits: 4.75_f32.to_bits(),
            recharge_bits: 5.5_f32.to_bits(),
            gun_level: 4,
            bomb_level: 3,
            shrapnel: 31,
            repels: 9,
            bursts: 8,
            decoys: 7,
            thors: 6,
            bricks: 5,
            portals: 4,
            super_until_tick: Some(3),
            shield_until_tick: Some(4),
            engine_shutdown_until_tick: Some(5),
            emp_until_tick: Some(6),
            damage_loss_seq: Some(u32::MAX),
            damage_loss_prize_id: Some(21),
            energy: Some(987),
        });
        let frame = SnapshotFrame {
            events: vec![prize.clone()],
            ..SnapshotFrame::default()
        };
        assert_eq!(
            SnapshotFrame::parse(&frame.encode()).unwrap().events,
            vec![prize]
        );
    }

    #[test]
    fn private_prize_state_rejects_invalid_identity_flags_and_nonfinite_stats() {
        let valid = SnapshotFrame {
            events: vec![SnapshotEvent::PrizeState(SnapshotPrizeState {
                life_seq: 1,
                revision: 1,
                rotation_bits: 1.0_f32.to_bits(),
                thrust_bits: 1.0_f32.to_bits(),
                top_speed_bits: 1.0_f32.to_bits(),
                recharge_bits: 1.0_f32.to_bits(),
                ..SnapshotPrizeState::default()
            })],
            ..SnapshotFrame::default()
        };
        let body = SnapshotHeader::HEADER_LEN + SnapshotEvent::HEADER_LEN;
        let mut bytes = valid.encode();
        bytes[body..body + 4].copy_from_slice(&0_u32.to_le_bytes());
        assert!(SnapshotFrame::parse(&bytes).is_err());

        let mut bytes = valid.encode();
        bytes[body + 8..body + 10].copy_from_slice(&(1_u16 << 15).to_le_bytes());
        assert!(SnapshotFrame::parse(&bytes).is_err());

        let mut bytes = valid.encode();
        bytes[body + 14..body + 18].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(SnapshotFrame::parse(&bytes).is_err());

        let mut bytes = valid.encode();
        bytes[body + 8..body + 10].copy_from_slice(&(1_u16 << 10).to_le_bytes());
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "a present damage-loss marker requires a nonzero sequence"
        );

        let mut bytes = valid.encode();
        bytes[body + 8..body + 10].copy_from_slice(&(1_u16 << 11).to_le_bytes());
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "a pending Shutdown source requires an explicitly present deadline"
        );

        let mut bytes = valid.encode();
        bytes[body + 8..body + 10].copy_from_slice(&(1_u16 << 10).to_le_bytes());
        bytes[body + 51..body + 55].copy_from_slice(&1_u32.to_le_bytes());
        bytes[body + 55..body + 57].copy_from_slice(&29_i16.to_le_bytes());
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "damage-loss ids are the exact canonical 0..=28 range"
        );

        let mut bytes = valid.encode();
        bytes[body + 51..body + 55].copy_from_slice(&1_u32.to_le_bytes());
        assert!(
            SnapshotFrame::parse(&bytes).is_err(),
            "an absent marker cannot hide nonzero damage-loss identity bytes"
        );

        let mut bytes = valid.encode();
        bytes[body + 8..body + 10].copy_from_slice(&(1_u16 << 10).to_le_bytes());
        bytes[body + 51..body + 55].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            SnapshotFrame::parse(&bytes).is_ok(),
            "None(0) is a canonical loss id and sequence wrap keeps u32::MAX valid"
        );
    }

    #[test]
    fn source_time_revision_round_trips_and_rejects_missing_invalid_or_truncated_time() {
        let sample_time =
            PresentationSampleTime::from_parts(crate::continuum::CONTINUUM_TICK_MASK, 2).unwrap();
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 1,
                ..SnapshotHeader::default()
            },
            near_entities: vec![SnapshotEntity::near_full(
                7,
                SnapshotEntityState {
                    life_seq: 3,
                    sample_time,
                    ..SnapshotEntityState::default()
                },
            )],
            mid_entities: vec![],
            events: vec![],
        };
        let bytes = frame.encode();
        assert_eq!(bytes[1], SNAPSHOT_FLAG_FULL | SNAPSHOT_WIRE_REVISIONS);
        assert_eq!(
            SnapshotFrame::parse(&bytes).unwrap().near_entities[0].sample_time,
            Some(sample_time)
        );
        let mut old_revision = bytes.clone();
        old_revision[1] &= !SNAPSHOT_FLAG_SOURCE_TIME;
        assert!(
            SnapshotFrame::parse(&old_revision)
                .unwrap_err()
                .contains("revision")
        );
        let mut invalid_phase = bytes.clone();
        // Source time precedes the coupled physical/relocation u32 pair.
        let phase_offset = invalid_phase.len() - 8 - 1;
        invalid_phase[phase_offset] = 3;
        assert!(
            SnapshotFrame::parse(&invalid_phase)
                .unwrap_err()
                .contains("phase")
        );
        let mut invalid_tick = bytes.clone();
        let tick_offset = phase_offset - 4;
        invalid_tick[tick_offset..tick_offset + 4].copy_from_slice(&(1_u32 << 31).to_le_bytes());
        assert!(
            SnapshotFrame::parse(&invalid_tick)
                .unwrap_err()
                .contains("tick")
        );
        for removed in 1..=13 {
            assert!(SnapshotFrame::parse(&bytes[..bytes.len() - removed]).is_err());
        }
    }

    #[test]
    fn motion_segment_survives_lost_snapshot_and_acknowledged_base_deltas() {
        let old = SnapshotEntityState {
            x: 100,
            life_seq: 7,
            motion_segment: 1,
            ..SnapshotEntityState::default()
        };
        let base = [(768, old)].into_iter().collect();
        let moved = SnapshotEntityState {
            x: 130,
            motion_segment: 2,
            ..old
        };
        // Snapshot 2 was lost. Snapshot 3 still references acknowledged base 1,
        // so its absolute generation must repeat without a timed Flash lease.
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 3,
                base_id: 1,
                ..SnapshotHeader::default()
            },
            near_entities: delta_encode_near_tier(&[(768, moved)], Some(&base)),
            mid_entities: vec![],
            events: vec![],
        };
        assert_eq!(frame.near_entities[0].motion_segment, Some(2));
        let bytes = frame.encode();
        assert_ne!(bytes[1] & SNAPSHOT_FLAG_MOTION_SEGMENT, 0);
        let applied = delta_apply(Some(&base), &SnapshotFrame::parse(&bytes).unwrap()).unwrap();
        assert_eq!(applied.frame.near_entities[0].motion_segment, Some(2));
        let next = delta_encode_near_tier(&[(768, moved)], Some(&applied.entities));
        assert_eq!(next[0].motion_segment, None);
        let unchanged = SnapshotFrame {
            near_entities: next,
            ..frame
        };
        assert_eq!(
            delta_apply(Some(&applied.entities), &unchanged)
                .unwrap()
                .frame
                .near_entities[0]
                .motion_segment,
            Some(2)
        );
        for removed in 1..=8 {
            assert!(SnapshotFrame::parse(&bytes[..bytes.len() - removed]).is_err());
        }
        let mut old_revision = bytes;
        old_revision[1] &= !SNAPSHOT_FLAG_MOTION_SEGMENT;
        assert!(
            SnapshotFrame::parse(&old_revision)
                .unwrap_err()
                .contains("motion-segment")
        );
    }

    #[test]
    fn lost_relocation_then_impulse_preserves_both_generations_on_wire() {
        let original = SnapshotEntityState {
            x: 100,
            life_seq: 7,
            motion_segment: 1,
            relocation_generation: 1,
            ..SnapshotEntityState::default()
        };
        let base = [(768, original)].into_iter().collect();
        let repelled_after_lost_relocation = SnapshotEntityState {
            x: 130,
            motion_segment: 3,
            relocation_generation: 2,
            ..original
        };
        let frame = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 3,
                base_id: 1,
                ..SnapshotHeader::default()
            },
            near_entities: delta_encode_near_tier(
                &[(768, repelled_after_lost_relocation)],
                Some(&base),
            ),
            ..SnapshotFrame::default()
        };
        let bytes = frame.encode();
        assert_ne!(bytes[1] & SNAPSHOT_FLAG_RELOCATION_GENERATION, 0);
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        let applied = delta_apply(Some(&base), &parsed).unwrap();
        assert_eq!(applied.entities[&768], repelled_after_lost_relocation);
        assert_eq!(
            applied.frame.near_entities[0].relocation_generation,
            Some(2)
        );
        let unchanged = delta_encode_near_tier(
            &[(768, repelled_after_lost_relocation)],
            Some(&applied.entities),
        );
        assert_eq!(unchanged[0].motion_segment, None);
        assert_eq!(unchanged[0].relocation_generation, None);
        let mut old_revision = bytes.clone();
        old_revision[1] &= !SNAPSHOT_FLAG_RELOCATION_GENERATION;
        assert!(
            SnapshotFrame::parse(&old_revision)
                .unwrap_err()
                .contains("relocation-generation")
        );
        for tail in 1..=8 {
            assert!(SnapshotFrame::parse(&bytes[..bytes.len() - tail]).is_err());
        }
    }

    #[test]
    fn unknown_event_type_is_preserved_with_body_for_forward_compat() {
        // Hand-craft a frame with one entity + one unknown-type event.
        let mut bytes = vec![
            0x70,                                         // type
            SNAPSHOT_FLAG_FULL | SNAPSHOT_WIRE_REVISIONS, // flags + wire revision
            1,
            0, // snapshot_id
            0,
            0, // base_id (full)
            0,
            0,
            0,
            0, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // arena_stream_id (legacy test stream)
            0, // near_count
            0, // mid_count
            1, // event_count
        ];
        // Unknown event: type 0xAB, body_len 3, body [10, 20, 30]
        bytes.extend_from_slice(&[0xAB, 3, 10, 20, 30]);
        let frame = SnapshotFrame::parse(&bytes).unwrap();
        match &frame.events[0] {
            SnapshotEvent::Unknown { event_type, body } => {
                assert_eq!(*event_type, 0xAB);
                assert_eq!(body, &vec![10u8, 20, 30]);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_with_entities_and_events_round_trips() {
        let frame = SnapshotFrame {
            header: SnapshotHeader::default(),
            near_entities: vec![entity(1, 100, 200)],
            mid_entities: vec![],
            events: vec![SnapshotEvent::WatchDamage(SnapshotWatchDamage {
                victim_id: 1,
                shooter_id: 2,
                weapon_data: 5,
                damage: 100,
                energy_before: Some(600),
                energy_after: 500,
                server_tick: Some(77),
                damage_seq: Some(88),
                projectile_id: Some(100),
                victim_life_seq: Some(1),
                shooter_life_seq: Some(2),
                lethal: true,
            })],
        };
        let bytes = frame.encode();
        let parsed = SnapshotFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.near_entities, frame.near_entities);
        assert_eq!(parsed.events, frame.events);
    }

    #[test]
    fn rejects_truncated_event_body() {
        // Header claims 1 event but body is missing.
        let mut bytes = SnapshotFrame::default().encode();
        bytes[20] = 1; // event_count
        bytes.push(SnapshotEvent::TYPE_WATCH_DAMAGE);
        bytes.push(SnapshotEvent::WATCH_DAMAGE_BODY_LEN as u8);
        // Truncate before the body.
        assert!(SnapshotFrame::parse(&bytes).is_err());
    }

    #[test]
    fn applied_frame_reencode_uses_mask_before_following_event() {
        let base_state = SnapshotEntityState {
            x: 100,
            y: 200,
            velocity_x: 3,
            velocity_y: -4,
            direction: 7,
            energy: 1_500,
            status: 0x02,
            bounty: 9,
            ping: 11,
            flag_timer: 13,
            ..SnapshotEntityState::default()
        };
        let mut base = SnapshotEntityStateMap::new();
        base.insert(7, base_state);
        let event = SnapshotEvent::Unknown {
            event_type: 0xab,
            body: vec![10, 20, 30],
        };
        let delta = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 2,
                base_id: 1,
                ..SnapshotHeader::default()
            },
            near_entities: vec![SnapshotEntity {
                entity_id: 7,
                field_mask: SNAPSHOT_FIELD_PING,
                ping: Some(42),
                ..SnapshotEntity::default()
            }],
            events: vec![event.clone()],
            ..SnapshotFrame::default()
        };

        let applied = delta_apply(Some(&base), &delta).unwrap();
        assert_eq!(applied.frame.near_entities[0].status, Some(0x02));
        assert_eq!(
            applied.frame.near_entities[0].field_mask & SNAPSHOT_FIELD_STATUS,
            0,
            "status value is reconstructed even though it was absent on the wire"
        );

        let reparsed = SnapshotFrame::parse(&applied.frame.encode()).unwrap();
        assert_eq!(reparsed.events, vec![event]);
        assert_eq!(reparsed.near_entities[0].status, None);
        assert_eq!(
            delta_apply(Some(&base), &reparsed).unwrap().entities,
            applied.entities
        );
    }

    #[test]
    fn header_length_matches_byte_layout() {
        assert_eq!(SnapshotHeader::HEADER_LEN, 21);
        // Phase 3e-7: entity header is now id(2) + mask(2) = 4 bytes.
        assert_eq!(SnapshotEntity::HEADER_LEN, 4);
    }

    #[test]
    fn snapshot_ack_round_trips() {
        let ack = SnapshotAckPacket {
            snapshot_id: 0x1234,
        };
        let bytes = ack.encode();
        assert_eq!(bytes.len(), SnapshotAckPacket::PACKET_LEN);
        assert_eq!(bytes[0], SnapshotAckPacket::TYPE);
        let parsed = SnapshotAckPacket::parse(&bytes).unwrap();
        assert_eq!(parsed, ack);
    }

    #[test]
    fn snapshot_ack_rejects_wrong_type() {
        let bytes = vec![0x70, 0x01, 0x00];
        assert!(SnapshotAckPacket::parse(&bytes).is_err());
    }

    #[test]
    fn snapshot_ack_rejects_truncated() {
        let bytes = vec![0x71, 0x01];
        assert!(SnapshotAckPacket::parse(&bytes).is_err());
    }

    #[test]
    fn snapshot_ack_packet_length_matches_byte_layout() {
        // 1 type + 2 snapshot_id = 3 bytes.
        assert_eq!(SnapshotAckPacket::PACKET_LEN, 3);
    }

    #[test]
    fn snapshot_id_allocator_soaks_wrap_without_allocating_zero() {
        let mut id = 0;
        for _ in 0..70_000 {
            let next = next_snapshot_id(id);
            assert_ne!(next, 0);
            assert!(snapshot_id_is_forward(next, id));
            id = next;
        }
    }

    #[test]
    fn projectile_refresh_request_requires_exact_scope_shape() {
        let request = ProjectileRefreshRequestPacket {
            server_session_id: 9,
            arena_stream_id: 3,
        };
        let bytes = request.encode();
        assert_eq!(ProjectileRefreshRequestPacket::parse(&bytes), Some(request));
        assert!(SnapshotFullRequestPacket::parse(&bytes).is_err());
        for len in 0..bytes.len() {
            assert!(ProjectileRefreshRequestPacket::parse(&bytes[..len]).is_none());
        }
        let mut extended = bytes.clone();
        extended.push(0);
        assert!(ProjectileRefreshRequestPacket::parse(&extended).is_none());
        for offset in [0, 1, 2, 10] {
            let mut invalid = bytes.clone();
            invalid[offset] = 0;
            assert!(ProjectileRefreshRequestPacket::parse(&invalid).is_none());
        }
    }

    #[test]
    fn snapshot_full_request_round_trips() {
        let bytes = SnapshotFullRequestPacket.encode();
        assert_eq!(bytes, vec![SNAPSHOT_FULL_REQUEST_PACKET]);
        assert_eq!(
            SnapshotFullRequestPacket::parse(&bytes),
            Ok(SnapshotFullRequestPacket)
        );
        assert!(SnapshotFullRequestPacket::parse(&[SNAPSHOT_FULL_REQUEST_PACKET, 0]).is_err());
    }

    #[test]
    fn codec_round_trip_stays_authoritative_through_simulated_loss() {
        let mut random = 0x6d2b_79f5_u32;
        let mut snapshot_id = 0_u16;
        let mut crossed_snapshot_id_wrap = false;
        let mut applied_after_wrap = false;
        let mut received_since_ack = 0_u8;
        let mut active_base = None;
        let mut active_base_is_mid = false;
        let mut returned_near_from_mid_base = false;
        let mut server_history: VecDeque<(u16, SnapshotEntityStateMap)> = VecDeque::new();
        let mut client_history: VecDeque<(u16, SnapshotEntityStateMap)> = VecDeque::new();

        for step in 0..70_000_u32 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let previous_snapshot_id = snapshot_id;
            snapshot_id = next_snapshot_id(snapshot_id);
            crossed_snapshot_id_wrap |= snapshot_id < previous_snapshot_id;
            let current = SnapshotEntityState {
                x: 1_000 + (random as u16 % 4_000),
                y: 2_000 + ((random >> 8) as u16 % 4_000),
                velocity_x: (random >> 16) as i16,
                velocity_y: random as i16,
                direction: random as u8 % 40,
                energy: (random >> 4) as u16,
                weapon: (step % 17 == 0).then_some(0x42).unwrap_or(0),
                status: (random >> 12) as u8,
                bounty: step as u16,
                ping: (random >> 24) as u8,
                flag_timer: (step as u16).wrapping_mul(3),
                life_seq: 1 + step / 10_000,
                sample_time: PresentationSampleTime::from_parts(step, (step % 3) as u8).unwrap(),
                motion_segment: 1 + step / 250,
                relocation_generation: 1 + step / 250,
            };
            let is_near = (step / 25) % 2 == 0;
            let current_near_tier = vec![(7, current)];
            let current_mid_tier = vec![(7, current.x, current.y, current.life_seq)];
            let base = active_base.and_then(|base_id| {
                server_history
                    .iter()
                    .find(|(id, _)| *id == base_id)
                    .map(|(_, entities)| entities)
            });
            let frame = SnapshotFrame {
                header: SnapshotHeader {
                    flags: if base.is_none() {
                        SNAPSHOT_FLAG_FULL
                    } else {
                        0
                    },
                    snapshot_id,
                    base_id: active_base.unwrap_or(0),
                    tick: step,
                    arena_stream_id: 7,
                    near_count: u8::from(is_near),
                    mid_count: u8::from(!is_near),
                    event_count: 0,
                },
                near_entities: if is_near {
                    delta_encode_near_tier(&current_near_tier, base)
                } else {
                    vec![]
                },
                mid_entities: if is_near {
                    vec![]
                } else {
                    delta_encode_mid_tier(&current_mid_tier, base)
                },
                events: vec![],
            };
            let mut authoritative = SnapshotEntityStateMap::new();
            authoritative.insert(
                7,
                if is_near {
                    current
                } else {
                    SnapshotEntityState {
                        x: current.x,
                        y: current.y,
                        life_seq: current.life_seq,
                        ..SnapshotEntityState::default()
                    }
                },
            );
            server_history.push_back((snapshot_id, authoritative.clone()));
            while server_history.len() > SNAPSHOT_HISTORY_CAPACITY {
                let removable = server_history
                    .iter()
                    .position(|(id, _)| Some(*id) != active_base)
                    .unwrap();
                server_history.remove(removable);
            }

            // Deterministic 20% loss on the unreliable snapshot stream.
            if random % 5 == 0 {
                continue;
            }
            let parsed = SnapshotFrame::parse(&frame.encode()).unwrap();
            let client_base = if parsed.header.is_full() {
                None
            } else {
                client_history
                    .iter()
                    .find(|(id, _)| *id == parsed.header.base_id)
                    .map(|(_, entities)| entities)
            };
            let applied = delta_apply(client_base, &parsed).unwrap();
            assert_eq!(applied.entities, authoritative, "step {step}");
            applied_after_wrap |= crossed_snapshot_id_wrap;
            returned_near_from_mid_base |= is_near && active_base.is_some() && active_base_is_mid;
            client_history.push_back((snapshot_id, applied.entities));
            while client_history.len() > SNAPSHOT_HISTORY_CAPACITY {
                let removable = client_history
                    .iter()
                    .position(|(id, _)| Some(*id) != active_base)
                    .unwrap();
                client_history.remove(removable);
            }
            received_since_ack += 1;
            if received_since_ack == SNAPSHOT_ACK_INTERVAL {
                active_base = Some(snapshot_id);
                active_base_is_mid = !is_near;
                received_since_ack = 0;
            }
        }

        assert!(
            crossed_snapshot_id_wrap,
            "codec soak must cross u16 id wrap"
        );
        assert!(
            applied_after_wrap,
            "codec soak must reconstruct delivered frames after id wrap"
        );
        assert!(
            returned_near_from_mid_base,
            "codec soak must reconstruct a near entity against an acknowledged mid-tier base"
        );
    }
}
