#![allow(dead_code)]

/// Application-reliable request for one predicted Portal inventory spend.
/// The reliable wrapper orders delivery; this complete gameplay identity
/// prevents a delayed wrapper from being rebound after any authority cutover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalPlacementRequest {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) action_id: u32,
    pub(crate) position_sequence: u16,
}

impl PortalPlacementRequest {
    pub(crate) const TYPE: u8 = 0x65;
    pub(crate) const VERSION: u8 = 1;
    pub(crate) const PACKET_LEN: usize = 30;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.arena_stream_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.action_id.to_le_bytes());
        packet.extend(self.position_sequence.to_le_bytes());
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN || packet[0] != Self::TYPE || packet[1] != Self::VERSION
        {
            return Err("invalid Portal placement request framing".to_string());
        }
        let request = Self {
            server_session_id: u64::from_le_bytes(packet[2..10].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(packet[10..18].try_into().unwrap()),
            player_id: u16::from_le_bytes(packet[18..20].try_into().unwrap()),
            life_seq: u32::from_le_bytes(packet[20..24].try_into().unwrap()),
            action_id: u32::from_le_bytes(packet[24..28].try_into().unwrap()),
            position_sequence: u16::from_le_bytes(packet[28..30].try_into().unwrap()),
        };
        if request.server_session_id == 0
            || request.arena_stream_id == 0
            || request.life_seq == 0
            || request.action_id == 0
        {
            return Err("invalid Portal placement request identity".to_string());
        }
        Ok(request)
    }
}

/// Exact terminal for a Portal placement request. `portals` and `revision`
/// are the authoritative post-outcome private state, so either acceptance or
/// rejection repairs client prediction without trusting snapshot ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalPlacementAccepted {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) action_id: u32,
    pub(crate) position_sequence: u16,
    pub(crate) prize_state_revision: u32,
    pub(crate) portals: u8,
    pub(crate) outcome: u8,
    /// Exact canonical placement pose and logical lifetime authored by the
    /// server. Raw f32 bits keep the reliable terminal Eq/idempotent while
    /// preserving the subpixel position used by movement authority.
    pub(crate) position_x_bits: u32,
    pub(crate) position_y_bits: u32,
    pub(crate) placed_at_tick: u32,
    pub(crate) expires_at_tick: u32,
}

impl PortalPlacementAccepted {
    pub(crate) const TYPE: u8 = 0x66;
    pub(crate) const VERSION: u8 = 2;
    pub(crate) const PACKET_LEN: usize = 52;
    pub(crate) const OUTCOME_APPLIED: u8 = 1;
    pub(crate) const OUTCOME_REJECTED: u8 = 2;

    pub(crate) fn encode(self) -> Vec<u8> {
        let request = PortalPlacementRequest {
            server_session_id: self.server_session_id,
            arena_stream_id: self.arena_stream_id,
            player_id: self.player_id,
            life_seq: self.life_seq,
            action_id: self.action_id,
            position_sequence: self.position_sequence,
        };
        let mut packet = request.encode();
        packet[0] = Self::TYPE;
        packet[1] = Self::VERSION;
        packet.extend(self.prize_state_revision.to_le_bytes());
        packet.push(self.portals);
        packet.push(self.outcome);
        packet.extend(self.position_x_bits.to_le_bytes());
        packet.extend(self.position_y_bits.to_le_bytes());
        packet.extend(self.placed_at_tick.to_le_bytes());
        packet.extend(self.expires_at_tick.to_le_bytes());
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet[0] != Self::TYPE
            || packet[1] != Self::VERSION
            || !matches!(packet[35], Self::OUTCOME_APPLIED | Self::OUTCOME_REJECTED)
        {
            return Err("invalid Portal placement terminal framing".to_string());
        }
        let mut request_packet = packet[..PortalPlacementRequest::PACKET_LEN].to_vec();
        request_packet[0] = PortalPlacementRequest::TYPE;
        request_packet[1] = PortalPlacementRequest::VERSION;
        let request = PortalPlacementRequest::parse(&request_packet)?;
        let terminal = Self {
            server_session_id: request.server_session_id,
            arena_stream_id: request.arena_stream_id,
            player_id: request.player_id,
            life_seq: request.life_seq,
            action_id: request.action_id,
            position_sequence: request.position_sequence,
            prize_state_revision: u32::from_le_bytes(packet[30..34].try_into().unwrap()),
            portals: packet[34],
            outcome: packet[35],
            position_x_bits: u32::from_le_bytes(packet[36..40].try_into().unwrap()),
            position_y_bits: u32::from_le_bytes(packet[40..44].try_into().unwrap()),
            placed_at_tick: u32::from_le_bytes(packet[44..48].try_into().unwrap()),
            expires_at_tick: u32::from_le_bytes(packet[48..52].try_into().unwrap()),
        };
        if terminal.prize_state_revision == 0
            || !f32::from_bits(terminal.position_x_bits).is_finite()
            || !f32::from_bits(terminal.position_y_bits).is_finite()
        {
            return Err("invalid Portal placement terminal revision".to_string());
        }
        Ok(terminal)
    }

    pub(crate) fn position(self) -> (f32, f32) {
        (
            f32::from_bits(self.position_x_bits),
            f32::from_bits(self.position_y_bits),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_terminal_round_trip_exact_wrap_identities() {
        let request = PortalPlacementRequest {
            server_session_id: u64::MAX,
            arena_stream_id: u64::MAX - 1,
            player_id: u16::MAX,
            life_seq: u32::MAX,
            action_id: u32::MAX,
            position_sequence: u16::MAX,
        };
        assert_eq!(
            PortalPlacementRequest::parse(&request.encode()),
            Ok(request)
        );
        let terminal = PortalPlacementAccepted {
            server_session_id: request.server_session_id,
            arena_stream_id: request.arena_stream_id,
            player_id: request.player_id,
            life_seq: request.life_seq,
            action_id: request.action_id,
            position_sequence: request.position_sequence,
            prize_state_revision: u32::MAX,
            portals: u8::MAX,
            outcome: PortalPlacementAccepted::OUTCOME_APPLIED,
            position_x_bits: 123.25_f32.to_bits(),
            position_y_bits: 456.5_f32.to_bits(),
            placed_at_tick: 17,
            expires_at_tick: u32::MAX,
        };
        assert_eq!(
            PortalPlacementAccepted::parse(&terminal.encode()),
            Ok(terminal)
        );
    }

    #[test]
    fn malformed_or_zero_identity_packets_fail_closed() {
        let mut packet = PortalPlacementRequest {
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            life_seq: 4,
            action_id: 5,
            position_sequence: 6,
        }
        .encode();
        packet[24..28].copy_from_slice(&0_u32.to_le_bytes());
        assert!(PortalPlacementRequest::parse(&packet).is_err());
        packet.push(0);
        assert!(PortalPlacementRequest::parse(&packet).is_err());
    }
}
