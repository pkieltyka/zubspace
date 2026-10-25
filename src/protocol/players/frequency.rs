use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrequencyChange {
    pub(crate) player_id: u16,
    pub(crate) frequency: u16,
}

impl FrequencyChange {
    pub(crate) const TYPE: u8 = 0x0d;
    pub(crate) const PACKET_LEN: usize = 5;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "frequency change packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected frequency change packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
            frequency: read_u16(packet, 3)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipFrequencyChange {
    pub(crate) ship: u8,
    pub(crate) player_id: u16,
    pub(crate) frequency: u16,
    /// Zubspace appends the authoritative life generation to the original
    /// six-byte roster update. This lets the recipient bind its subsequent
    /// neutral baseline and application-level acceptance receipt to one
    /// exact lifecycle boundary instead of trusting only a wrapped reliable
    /// id and position sequence.
    pub(crate) life_seq: Option<u32>,
}

impl ShipFrequencyChange {
    pub(crate) const TYPE: u8 = 0x1d;
    pub(crate) const PACKET_LEN: usize = 6;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "ship/frequency change packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet.len() != Self::PACKET_LEN && packet.len() != Self::PACKET_LEN + 4 {
            return Err(format!(
                "ship/frequency packet length mismatch: {} is neither {} nor {}",
                packet.len(),
                Self::PACKET_LEN,
                Self::PACKET_LEN + 4
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected ship/frequency change packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            ship: packet[1],
            player_id: read_u16(packet, 2)?,
            frequency: read_u16(packet, 4)?,
            life_seq: (packet.len() >= 10)
                .then(|| read_u32(packet, 6))
                .transpose()?,
        })
    }
}

/// Private zubspace acknowledgement for a frequency sequence cutover. These
/// fields are the server's canonical accepted pre-cutover physical state; the
/// client must echo them in its exact reliable post-cutover baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrequencyCutoverAck {
    pub(crate) frequency: u16,
    pub(crate) position_sequence_base: u16,
    pub(crate) life_seq: u32,
    pub(crate) direction: u8,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) togglables: u8,
    pub(crate) bounty: u16,
    pub(crate) energy: u16,
    /// Mandatory current-protocol raw state for the server-owned cutover
    /// anchor. The legacy fields remain a projection and must encode these
    /// exact bits.
    pub(crate) canonical_state: crate::continuum_position::ClientPositionCanonicalState,
}

impl FrequencyCutoverAck {
    pub(crate) const TYPE: u8 = 0x74;
    pub(crate) const LEGACY_PACKET_LEN: usize = 23;
    pub(crate) const PACKET_LEN: usize =
        Self::LEGACY_PACKET_LEN + crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_LEN;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "frequency cutover packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected frequency cutover packet type: {:#04x}",
                packet[0]
            ));
        }
        let canonical_state = crate::continuum_position::client_position_canonical_state_at(
            packet,
            Self::LEGACY_PACKET_LEN,
        )
        .ok_or_else(|| "frequency cutover missing canonical-state extension".to_string())?;
        let result = Self {
            frequency: read_u16(packet, 1)?,
            position_sequence_base: read_u16(packet, 3)?,
            life_seq: read_u32(packet, 5)?,
            direction: packet[9],
            x: read_u16(packet, 10)?,
            y: read_u16(packet, 12)?,
            velocity_x: read_i16(packet, 14)?,
            velocity_y: read_i16(packet, 16)?,
            togglables: packet[18],
            bounty: read_u16(packet, 19)?,
            energy: read_u16(packet, 21)?,
            canonical_state,
        };
        if !result.canonical_state.matches_legacy_wire(
            result.direction,
            result.x,
            result.y,
            result.velocity_x,
            result.velocity_y,
        ) {
            return Err("frequency cutover canonical state does not match legacy fields".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod frequency_cutover_tests {
    use super::*;

    fn exact_packet(state: crate::continuum_position::ClientPositionCanonicalState) -> Vec<u8> {
        let (x, y, velocity_x, velocity_y, orientation) = state.values();
        let mut packet = vec![FrequencyCutoverAck::TYPE];
        packet.extend(17_u16.to_le_bytes());
        packet.extend(u16::MAX.to_le_bytes());
        packet.extend(33_u32.to_le_bytes());
        packet.push(crate::continuum_position::protocol_direction(orientation));
        packet.extend(crate::continuum_position::protocol_position_coord(x).to_le_bytes());
        packet.extend(crate::continuum_position::protocol_position_coord(y).to_le_bytes());
        packet.extend(
            crate::continuum_position::protocol_velocity_component(velocity_x).to_le_bytes(),
        );
        packet.extend(
            crate::continuum_position::protocol_velocity_component(velocity_y).to_le_bytes(),
        );
        packet.push(0);
        packet.extend(9_u16.to_le_bytes());
        packet.extend(800_u16.to_le_bytes());
        state.encode(&mut packet);
        packet
    }

    #[test]
    fn current_frequency_cutover_requires_exact_raw_anchor_and_legacy_consistency() {
        let state = crate::continuum_position::ClientPositionCanonicalState::from_f32(
            1_234.25, 4_321.0, 19.75, -3.25, 0.15_f32,
        );
        let packet = exact_packet(state);
        let parsed = FrequencyCutoverAck::parse(&packet).expect("exact cutover ACK");
        assert_eq!(parsed.canonical_state, state);
        assert_eq!(parsed.position_sequence_base, u16::MAX);

        assert!(
            FrequencyCutoverAck::parse(&packet[..FrequencyCutoverAck::LEGACY_PACKET_LEN]).is_err()
        );
        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(FrequencyCutoverAck::parse(&trailing).is_err());

        let mut malformed_version = packet.clone();
        malformed_version[FrequencyCutoverAck::LEGACY_PACKET_LEN + 1] ^= 1;
        assert!(FrequencyCutoverAck::parse(&malformed_version).is_err());

        let mut mismatched_raw = packet.clone();
        let offset = FrequencyCutoverAck::LEGACY_PACKET_LEN + 3;
        mismatched_raw[offset..offset + 4].copy_from_slice(&1_235.0_f32.to_bits().to_le_bytes());
        assert!(FrequencyCutoverAck::parse(&mismatched_raw).is_err());

        let mut nonfinite = packet;
        nonfinite[offset..offset + 4].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(FrequencyCutoverAck::parse(&nonfinite).is_err());
    }
}
