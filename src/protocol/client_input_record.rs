// `parse` and `MAX_INPUTS` are only exercised by the round-trip
// tests at the bottom of this file. The production server uses its
// own decoder in `bin/zubspace_server/connection.rs` (constants
// duplicated there for the same reason). The client only encodes
// (constructor + `encode` via session::send_input_record). Module
// allow dead-code so per-bin compilation doesn't warn on the
// test-only / cross-bin items.
#![allow(dead_code)]

//! Client → server input record (zubspace extension, type `0x62`).
//!
//! Clients send a compact intent history inside every position packet,
//! and as a standalone record for steps that publish no position, so the
//! server simulates from input rather than from reported positions. The
//! same `base_seq` is mirrored into zubspace's extended position tail so
//! movement and weapon frames can be deduped without client time.
//!
//! Wire layout matches `docs/authoritative_server.md` §4:
//!
//! ```text
//! type:u8 = 0x62
//! base_seq:u16    // sequence number of the first input in the ring
//! count:u8        // number of inputs in this packet (1..=32)
//! [bits:u16, direction:u8, door_seed:u8,
//!  combat_view_receipt_v1:14 bytes] × count
//! ```
//!
//! `base_seq` lets the server reassemble dropped/reordered inputs
//! deterministically and reject duplicate/replayed gameplay frames.

use super::*;
use std::collections::VecDeque;

/// One input applied during one client tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayerInputRecord {
    /// Bit-packed `PlayerInput` (see `gameplay::PlayerInput::bits`).
    pub(crate) bits: u16,
    /// Wire-format direction at the time the input was applied
    /// (Continuum 0..39 scale). Lets the server cross-check that the
    /// client's heading matches what the input actions imply.
    pub(crate) direction: u8,
    /// Exact dynamic-door seed the browser simulated for this logical input.
    /// Doors advance on wall/render time rather than logical-input time, so
    /// the server validates this against its security-seed phase window before
    /// using it to disambiguate collision replay.
    pub(crate) door_seed: u8,
    /// Bounded proof of the server-owned observer schedule and latest
    /// Game-applied snapshot for this exact logical input. Generation zero is
    /// the explicit fail-closed pre-directive placeholder.
    pub(crate) combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1,
}

/// One inbound packet carrying up to `MAX_INPUTS` consecutive inputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientInputRecordPacket {
    pub(crate) base_seq: u16,
    pub(crate) inputs: Vec<PlayerInputRecord>,
}

impl ClientInputRecordPacket {
    pub(crate) const TYPE: u8 = 0x62;
    pub(crate) const HEADER_LEN: usize = 4;
    pub(crate) const ENTRY_LEN: usize =
        4 + crate::combat_timeline::CombatViewReceiptV1::ENCODED_LEN;
    pub(crate) const MAX_INPUTS: usize = 32;
    /// Inputs every packet carries even when the server has applied them,
    /// so a stale acknowledgement costs bytes, never an input.
    pub(crate) const MIN_RESENT_INPUTS: usize = 4;

    /// Drops the inputs the server reports applied through `applied`, which it
    /// will never need again, keeping every later input and at least
    /// [`Self::MIN_RESENT_INPUTS`]. Under loss the server's contiguous
    /// applied point stops at the first gap, so the window grows back towards
    /// 32 by itself. An acknowledgement outside this window (another life, or
    /// none yet) keeps the whole window.
    pub(crate) fn without_applied_inputs(mut self, applied: Option<u16>) -> Self {
        let Some(applied) = applied else {
            return self;
        };
        let applied_offset = usize::from(applied.wrapping_sub(self.base_seq));
        if applied_offset >= self.inputs.len() {
            return self;
        }
        let dropped =
            (applied_offset + 1).min(self.inputs.len().saturating_sub(Self::MIN_RESENT_INPUTS));
        self.inputs.drain(..dropped);
        self.base_seq = self.base_seq.wrapping_add(dropped as u16);
        self
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::HEADER_LEN + self.inputs.len() * Self::ENTRY_LEN);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.base_seq);
        push_u8(&mut bytes, self.inputs.len() as u8);
        for input in &self.inputs {
            push_u16(&mut bytes, input.bits);
            push_u8(&mut bytes, input.direction);
            push_u8(&mut bytes, input.door_seed);
            bytes.extend_from_slice(&input.combat_view_receipt.encode());
        }
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_LEN {
            return Err(format!(
                "client input packet too short: {} < {}",
                packet.len(),
                Self::HEADER_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected client input packet type: {:#04x}",
                packet[0]
            ));
        }
        let base_seq = read_u16(packet, 1)?;
        let count = packet[3] as usize;
        if count == 0 || count > Self::MAX_INPUTS {
            return Err(format!(
                "input count {count} exceeds max {}",
                Self::MAX_INPUTS
            ));
        }
        let expected_len = Self::HEADER_LEN + count * Self::ENTRY_LEN;
        if packet.len() != expected_len {
            return Err(format!(
                "client input packet length mismatch: {} != {expected_len}",
                packet.len()
            ));
        }
        let mut inputs = Vec::with_capacity(count);
        for i in 0..count {
            let off = Self::HEADER_LEN + i * Self::ENTRY_LEN;
            let sequence = base_seq.wrapping_add(i as u16);
            let combat_view_receipt = crate::combat_timeline::CombatViewReceiptV1::decode(
                &packet[off + 4..off + Self::ENTRY_LEN],
            )
            .map_err(|error| format!("invalid combat-view receipt: {error:?}"))?;
            if combat_view_receipt.input_sequence != sequence {
                return Err(format!(
                    "combat-view receipt sequence mismatch: {} != {sequence}",
                    combat_view_receipt.input_sequence
                ));
            }
            inputs.push(PlayerInputRecord {
                bits: read_u16(packet, off)?,
                direction: packet[off + 2],
                door_seed: packet[off + 3],
                combat_view_receipt,
            });
        }
        Ok(Self { base_seq, inputs })
    }
}

/// Rolling redundancy window for the unordered realtime channel. Each new
/// packet repeats the prior 31 input ticks, allowing loss, reordering, and
/// several coalesced render batches within the bounded window to be
/// reconstructed without a reliable-channel head-of-line stall.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientInputRing {
    entries: VecDeque<(u16, PlayerInputRecord)>,
}

impl ClientInputRing {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Start a new authoritative life with exactly one neutral record. The
    /// sequence remains session-monotonic (including `u16::MAX -> 0`), while
    /// no movement bit or old-life door phase can cross the boundary through
    /// rolling redundancy.
    pub(crate) fn record_neutral_life_boundary(
        &mut self,
        sequence: u16,
        door_seed: u8,
        combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1,
    ) -> ClientInputRecordPacket {
        self.clear();
        self.record(
            sequence,
            PlayerInputRecord {
                bits: 0,
                direction: 0,
                door_seed,
                combat_view_receipt,
            },
        )
    }

    pub(crate) fn record(
        &mut self,
        sequence: u16,
        mut input: PlayerInputRecord,
    ) -> ClientInputRecordPacket {
        if self
            .entries
            .back()
            .is_some_and(|(previous, _)| previous.wrapping_add(1) != sequence)
        {
            self.entries.clear();
        }
        input.combat_view_receipt.input_sequence = sequence;
        self.entries.push_back((sequence, input));
        while self.entries.len() > ClientInputRecordPacket::MAX_INPUTS {
            self.entries.pop_front();
        }
        ClientInputRecordPacket {
            base_seq: self.entries.front().map_or(sequence, |(seq, _)| *seq),
            inputs: self.entries.iter().map(|(_, input)| *input).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u16, bits: u16, direction: u8, door_seed: u8) -> PlayerInputRecord {
        PlayerInputRecord {
            bits,
            direction,
            door_seed,
            combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence),
        }
    }

    #[test]
    fn empty_packet_is_rejected() {
        let packet = ClientInputRecordPacket {
            base_seq: 0,
            inputs: vec![],
        };
        let bytes = packet.encode();
        assert_eq!(bytes, vec![0x62, 0x00, 0x00, 0x00]);
        assert!(ClientInputRecordPacket::parse(&bytes).is_err());
    }

    #[test]
    fn single_input_packet_round_trips() {
        let packet = ClientInputRecordPacket {
            base_seq: 0x1234,
            inputs: vec![record(0x1234, 0x1f, 20, 1)],
        };
        let bytes = packet.encode();
        assert_eq!(
            bytes.len(),
            ClientInputRecordPacket::HEADER_LEN + ClientInputRecordPacket::ENTRY_LEN
        );
        let parsed = ClientInputRecordPacket::parse(&bytes).unwrap();
        assert_eq!(parsed, packet);
    }

    #[test]
    fn combat_view_receipt_version_and_exact_input_sequence_are_structural() {
        let packet = ClientInputRecordPacket {
            base_seq: 77,
            inputs: vec![record(77, 0x01, 4, 9)],
        }
        .encode();
        let receipt_offset = ClientInputRecordPacket::HEADER_LEN + 4;

        let mut wrong_version = packet.clone();
        wrong_version[receipt_offset] =
            crate::combat_timeline::COMBAT_VIEW_PROTOCOL_VERSION.wrapping_add(1);
        assert!(
            ClientInputRecordPacket::parse(&wrong_version)
                .unwrap_err()
                .contains("invalid combat-view receipt")
        );

        let mut wrong_sequence = packet;
        wrong_sequence[receipt_offset + 6..receipt_offset + 8]
            .copy_from_slice(&78_u16.to_le_bytes());
        assert!(
            ClientInputRecordPacket::parse(&wrong_sequence)
                .unwrap_err()
                .contains("receipt sequence mismatch")
        );
    }

    #[test]
    fn neutral_life_boundary_discards_old_inputs_and_continues_across_sequence_wrap() {
        let mut ring = ClientInputRing::default();
        let _ = ring.record(u16::MAX - 1, record(u16::MAX - 1, 0x1f, 39, 236));
        let _ = ring.record(u16::MAX, record(u16::MAX, 0x03, 0, 238));

        let baseline = ring.record_neutral_life_boundary(
            0,
            189,
            crate::combat_timeline::CombatViewReceiptV1::unavailable(0),
        );
        assert_eq!(baseline.base_seq, 0);
        assert_eq!(baseline.inputs, vec![record(0, 0, 0, 189)]);
        assert_eq!(ring.len(), 1);

        let next = ring.record(1, record(1, 0x01, 0, 219));
        assert_eq!(next.base_seq, 0);
        assert_eq!(
            next.inputs,
            vec![record(0, 0, 0, 189), record(1, 0x01, 0, 219)]
        );
    }

    #[test]
    fn full_ring_round_trips() {
        let inputs: Vec<_> = (0..ClientInputRecordPacket::MAX_INPUTS as u8)
            .map(|i| {
                let sequence = 0xfffe_u16.wrapping_add(u16::from(i));
                record(sequence, 0x1f, i, i)
            })
            .collect();
        let packet = ClientInputRecordPacket {
            base_seq: 0xfffe,
            inputs,
        };
        let bytes = packet.encode();
        assert_eq!(
            bytes.len(),
            ClientInputRecordPacket::HEADER_LEN
                + ClientInputRecordPacket::MAX_INPUTS * ClientInputRecordPacket::ENTRY_LEN
        );
        let parsed = ClientInputRecordPacket::parse(&bytes).unwrap();
        assert_eq!(parsed, packet);
    }

    #[test]
    fn rejects_wrong_type_byte() {
        let bad = vec![0x00, 0x00, 0x00, 0x00];
        assert!(ClientInputRecordPacket::parse(&bad).is_err());
    }

    #[test]
    fn rejects_truncated_packet() {
        let truncated = vec![0x62, 0x00, 0x00, 0x02, 0x01]; // count=2, only 1 byte
        assert!(ClientInputRecordPacket::parse(&truncated).is_err());
    }

    #[test]
    fn rejects_count_above_max() {
        let mut packet = vec![
            0x62,
            0x00,
            0x00,
            (ClientInputRecordPacket::MAX_INPUTS + 1) as u8,
        ];
        packet.extend(
            std::iter::repeat(0).take(
                (ClientInputRecordPacket::MAX_INPUTS + 1) * ClientInputRecordPacket::ENTRY_LEN,
            ),
        );
        assert!(ClientInputRecordPacket::parse(&packet).is_err());
    }

    #[test]
    fn parsed_input_bits_round_trip_through_player_input_struct() {
        // Sanity: any wire `bits` value reconstructs a `PlayerInput`
        // whose `bits()` matches (after dropping unknown high bits).
        let packet = ClientInputRecordPacket {
            base_seq: 1,
            inputs: vec![record(1, 0x1f, 0, 1)],
        };
        let bytes = packet.encode();
        let parsed = ClientInputRecordPacket::parse(&bytes).unwrap();
        let bits = parsed.inputs[0].bits;
        let player_input = crate::gameplay::PlayerInput::from_bits(bits);
        assert_eq!(player_input.bits(), bits);
    }

    #[test]
    fn rolling_ring_repeats_recent_inputs_and_survives_sequence_wrap() {
        let mut ring = ClientInputRing::default();
        let sequences = [0xfffc, 0xfffd, 0xfffe, 0xffff, 0, 1, 2, 3, 4, 5];
        let mut latest = None;
        for (index, sequence) in sequences.into_iter().enumerate() {
            latest = Some(ring.record(sequence, record(sequence, index as u16, index as u8, 0)));
        }
        let packet = latest.expect("ring packet");
        assert_eq!(packet.base_seq, 0xfffc);
        assert_eq!(packet.inputs.len(), sequences.len());
        assert_eq!(packet.inputs.first().map(|input| input.bits), Some(0));
        assert_eq!(packet.inputs.last().map(|input| input.bits), Some(9));
        assert_eq!(ClientInputRecordPacket::parse(&packet.encode()), Ok(packet));
    }

    #[test]
    fn applied_inputs_leave_the_window_down_to_a_floor_of_four() {
        let mut ring = ClientInputRing::default();
        let mut packet = None;
        // Thirty-two inputs ending across the u16 wrap: 0xfff0 ..= 0x000f.
        for offset in 0..32_u16 {
            let sequence = 0xfff0_u16.wrapping_add(offset);
            packet = Some(ring.record(sequence, record(sequence, offset, 0, 0)));
        }
        let packet = packet.expect("ring packet");
        let tail = packet.base_seq.wrapping_add(31);
        let sequences = |trimmed: &ClientInputRecordPacket| {
            (0..trimmed.inputs.len())
                .map(|index| trimmed.base_seq.wrapping_add(index as u16))
                .collect::<Vec<_>>()
        };

        // Applied through 0x0005: only 0x0006 ..= 0x000f still travel.
        let trimmed = packet.clone().without_applied_inputs(Some(0x0005));
        assert_eq!(trimmed.base_seq, 0x0006);
        assert_eq!(sequences(&trimmed).last(), Some(&tail));
        assert_eq!(trimmed.inputs.len(), 10);
        assert_eq!(trimmed.inputs[0], packet.inputs[22]);
        let encoded = trimmed.encode();
        assert_eq!(ClientInputRecordPacket::parse(&encoded), Ok(trimmed));

        // Applied through the tail: the last four still travel.
        let floor = packet.clone().without_applied_inputs(Some(tail));
        assert_eq!(
            floor.inputs.len(),
            ClientInputRecordPacket::MIN_RESENT_INPUTS
        );
        assert_eq!(sequences(&floor).last(), Some(&tail));

        // No acknowledgement, one older than the window, or one ahead of it
        // (another life): the whole window.
        for applied in [None, Some(0xffef), Some(tail.wrapping_add(1)), Some(0x8000)] {
            assert_eq!(packet.clone().without_applied_inputs(applied), packet);
        }
        // A short window never drops below the floor.
        let short = ClientInputRecordPacket {
            base_seq: 7,
            inputs: packet.inputs[..3].to_vec(),
        };
        assert_eq!(short.clone().without_applied_inputs(Some(8)), short);
    }

    #[test]
    fn rolling_ring_resets_on_nonconsecutive_sequence_base() {
        let mut ring = ClientInputRing::default();
        ring.record(10, record(10, 1, 0, 0));
        ring.record(11, record(11, 2, 0, 0));
        let packet = ring.record(30, record(30, 3, 0, 0));
        assert_eq!(packet.base_seq, 30);
        assert_eq!(packet.inputs, vec![record(30, 3, 0, 0)]);
    }
}
