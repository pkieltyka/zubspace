use super::*;

/// Private application-level receipt for the exact lifecycle baseline that
/// the authoritative position admission path committed. The client reliable
/// delivery id is session-scoped and 32-bit; life and position sequence keep
/// the receipt unambiguous across lifecycle and u16 gameplay-sequence wrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleBaselineAccepted {
    pub(crate) client_reliable_delivery_id: u32,
    pub(crate) life_seq: u32,
    pub(crate) position_sequence: u16,
    /// Exact authoritative tick at which the server committed the neutral
    /// lifecycle baseline. The browser may have extrapolated its deterministic
    /// door clock while the server or reliable lane was stalled; this sample
    /// lets the accepted lifecycle transaction rebase that clock before any
    /// successor-life rolling input is emitted.
    pub(crate) accepted_server_tick: u32,
    /// Server-selected location committed by this exact baseline transaction.
    /// Version 2 receipts retain the original neutral map-spawn location.
    pub(crate) accepted_position: Option<(u16, u16)>,
}

impl LifecycleBaselineAccepted {
    pub(crate) const TYPE: u8 = 0x75;
    pub(crate) const VERSION: u8 = 3;
    pub(crate) const PACKET_LEN: usize = 20;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        let legacy = packet.len() == 16 && packet.get(1) == Some(&2);
        if !legacy && packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "lifecycle baseline acceptance packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE || (!legacy && packet[1] != Self::VERSION) {
            return Err(format!(
                "unexpected lifecycle baseline acceptance packet type/version: {:#04x}/{:#04x}",
                packet[0], packet[1]
            ));
        }
        let accepted_position = if legacy {
            None
        } else {
            let point = (read_u16(packet, 16)?, read_u16(packet, 18)?);
            if point.0 >= 16_384 || point.1 >= 16_384 {
                return Err("lifecycle baseline acceptance outside map bounds".into());
            }
            Some(point)
        };
        Ok(Self {
            client_reliable_delivery_id: read_u32(packet, 2)?,
            life_seq: read_u32(packet, 6)?,
            position_sequence: read_u16(packet, 10)?,
            accepted_server_tick: read_u32(packet, 12)?,
            accepted_position,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarpRelocationAccepted {
    pub(crate) collectible_id: u32,
    pub(crate) life_seq: u32,
    pub(crate) position_sequence: u16,
}

/// Reliable application receipt for one target-private hostile Repel.
/// Snapshot ids and input sequences both wrap; the server-session id, target
/// life, and monotonic delivery generation keep the acknowledgement exact
/// across reconnect, lifecycle replacement, duplication, and reordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepelApplicationAccepted {
    pub(crate) server_session_id: u64,
    pub(crate) life_seq: u32,
    pub(crate) delivery_generation: u32,
    pub(crate) snapshot_id: u16,
    pub(crate) application_sequence: u16,
    /// Last input sequence for which RepelSpeed is authoritative. This is
    /// derived only from the server tick and observed marker tail.
    pub(crate) accepted_end_sequence: u16,
    pub(crate) active_steps: u16,
}

impl RepelApplicationAccepted {
    pub(crate) const TYPE: u8 = 0x7a;
    pub(crate) const VERSION: u8 = 3;
    pub(crate) const PACKET_LEN: usize = 26;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN || packet[0] != Self::TYPE || packet[1] != Self::VERSION
        {
            return Err("invalid Repel application acceptance packet".to_string());
        }
        let receipt = Self {
            server_session_id: u64::from_le_bytes(
                packet[2..10]
                    .try_into()
                    .map_err(|_| "invalid Repel receipt session")?,
            ),
            life_seq: read_u32(packet, 10)?,
            delivery_generation: read_u32(packet, 14)?,
            snapshot_id: read_u16(packet, 18)?,
            application_sequence: read_u16(packet, 20)?,
            accepted_end_sequence: read_u16(packet, 22)?,
            active_steps: read_u16(packet, 24)?,
        };
        if receipt.server_session_id == 0
            || receipt.life_seq == 0
            || receipt.delivery_generation == 0
            || receipt.active_steps == 0
            || receipt.accepted_end_sequence
                != receipt
                    .application_sequence
                    .wrapping_add(receipt.active_steps)
        {
            return Err("invalid Repel receipt identity or suffix".to_string());
        }
        Ok(receipt)
    }
}

impl WarpRelocationAccepted {
    pub(crate) const TYPE: u8 = 0x77;
    pub(crate) const PACKET_LEN: usize = 11;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN || packet[0] != Self::TYPE {
            return Err("invalid Warp relocation acceptance packet".to_string());
        }
        Ok(Self {
            collectible_id: read_u32(packet, 1)?,
            life_seq: read_u32(packet, 5)?,
            position_sequence: read_u16(packet, 9)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_session_life_and_sequence_receipt() {
        let mut packet = vec![
            LifecycleBaselineAccepted::TYPE,
            LifecycleBaselineAccepted::VERSION,
        ];
        packet.extend(0xffff_fffe_u32.to_le_bytes());
        packet.extend(41_u32.to_le_bytes());
        packet.extend(u16::MAX.to_le_bytes());
        packet.extend(0x7fff_fffe_u32.to_le_bytes());
        packet.extend(8104_u16.to_le_bytes());
        packet.extend(8312_u16.to_le_bytes());
        assert_eq!(
            LifecycleBaselineAccepted::parse(&packet),
            Ok(LifecycleBaselineAccepted {
                client_reliable_delivery_id: 0xffff_fffe,
                life_seq: 41,
                position_sequence: u16::MAX,
                accepted_server_tick: 0x7fff_fffe,
                accepted_position: Some((8104, 8312)),
            })
        );
        let mut legacy = packet.clone();
        legacy.remove(1);
        legacy.truncate(11);
        assert!(LifecycleBaselineAccepted::parse(&legacy).is_err());
    }

    #[test]
    fn lifecycle_receipt_preserves_legacy_and_rejects_bad_spawn_frames() {
        let mut packet = vec![LifecycleBaselineAccepted::TYPE, 2];
        packet.extend(7_u32.to_le_bytes());
        packet.extend(9_u32.to_le_bytes());
        packet.extend(11_u16.to_le_bytes());
        packet.extend(13_u32.to_le_bytes());
        assert_eq!(
            LifecycleBaselineAccepted::parse(&packet)
                .unwrap()
                .accepted_position,
            None
        );
        packet[1] = LifecycleBaselineAccepted::VERSION;
        assert!(LifecycleBaselineAccepted::parse(&packet).is_err());
        packet.extend(0_u16.to_le_bytes());
        packet.extend(16_383_u16.to_le_bytes());
        assert_eq!(
            LifecycleBaselineAccepted::parse(&packet)
                .unwrap()
                .accepted_position,
            Some((0, 16_383))
        );
        for end in 0..packet.len() {
            assert!(LifecycleBaselineAccepted::parse(&packet[..end]).is_err());
        }
        for (x, y) in [(16_384_u16, 0_u16), (0, 16_384), (u16::MAX, u16::MAX)] {
            packet[16..18].copy_from_slice(&x.to_le_bytes());
            packet[18..20].copy_from_slice(&y.to_le_bytes());
            assert!(LifecycleBaselineAccepted::parse(&packet).is_err());
        }
        packet[1] = 2;
        assert!(
            LifecycleBaselineAccepted::parse(&packet).is_err(),
            "a legacy receipt cannot smuggle an appended relocation"
        );
    }

    #[test]
    fn repel_receipt_is_versioned_exact_wrap_safe_and_rejects_malformed_identity() {
        let mut packet = vec![
            RepelApplicationAccepted::TYPE,
            RepelApplicationAccepted::VERSION,
        ];
        packet.extend(77_u64.to_le_bytes());
        packet.extend(9_u32.to_le_bytes());
        packet.extend(u32::MAX.to_le_bytes());
        packet.extend(u16::MAX.to_le_bytes());
        packet.extend(0_u16.to_le_bytes());
        packet.extend(135_u16.to_le_bytes());
        packet.extend(135_u16.to_le_bytes());
        assert_eq!(
            RepelApplicationAccepted::parse(&packet),
            Ok(RepelApplicationAccepted {
                server_session_id: 77,
                life_seq: 9,
                delivery_generation: u32::MAX,
                snapshot_id: u16::MAX,
                application_sequence: 0,
                accepted_end_sequence: 135,
                active_steps: 135,
            })
        );
        let mut wrong_version = packet.clone();
        wrong_version[1] = 1;
        assert!(RepelApplicationAccepted::parse(&wrong_version).is_err());
        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(RepelApplicationAccepted::parse(&trailing).is_err());
        let mut zero_generation = packet;
        zero_generation[14..18].copy_from_slice(&0_u32.to_le_bytes());
        assert!(RepelApplicationAccepted::parse(&zero_generation).is_err());
        let mut mismatched_end = vec![
            RepelApplicationAccepted::TYPE,
            RepelApplicationAccepted::VERSION,
        ];
        mismatched_end.extend(77_u64.to_le_bytes());
        mismatched_end.extend(9_u32.to_le_bytes());
        mismatched_end.extend(1_u32.to_le_bytes());
        mismatched_end.extend(7_u16.to_le_bytes());
        mismatched_end.extend(u16::MAX.to_le_bytes());
        mismatched_end.extend(1_u16.to_le_bytes());
        mismatched_end.extend(1_u16.to_le_bytes());
        assert!(RepelApplicationAccepted::parse(&mismatched_end).is_err());
    }
}

/// Terminal refusal of one exact ship request, without replacing its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipChangeRejected {
    pub(crate) ship: u8,
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) position_sequence: u16,
}

impl ShipChangeRejected {
    pub(crate) const TYPE: u8 = 0x7f;
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != 27
            || packet[0] != Self::TYPE
            || packet[1] != 1
            || packet[2] > 8
            || read_u32(packet, 21)? == 0
            || u64::from_le_bytes(packet[3..11].try_into().unwrap()) == 0
            || u64::from_le_bytes(packet[11..19].try_into().unwrap()) == 0
        {
            return Err("invalid ship-change rejection".into());
        }
        Ok(Self {
            ship: packet[2],
            server_session_id: u64::from_le_bytes(packet[3..11].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(packet[11..19].try_into().unwrap()),
            player_id: read_u16(packet, 19)?,
            life_seq: read_u32(packet, 21)?,
            position_sequence: read_u16(packet, 25)?,
        })
    }
}
