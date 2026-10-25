#![allow(dead_code)]

/// Application-reliable manual Warp request. The request names the exact
/// completed post-movement witness on which the real keyboard edge occurred;
/// the server alone selects and consumes the active Portal (or spawn Warp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalWarpRequest {
    pub(crate) force_random: bool,
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) action_id: u32,
    pub(crate) position_sequence: u16,
}

impl PortalWarpRequest {
    pub(crate) const TYPE: u8 = 0x6a;
    pub(crate) const VERSION: u8 = 2;
    pub(crate) const PACKET_LEN: usize = 31;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.arena_stream_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.action_id.to_le_bytes());
        packet.extend(self.position_sequence.to_le_bytes());
        packet.push(u8::from(self.force_random));
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet[0] != Self::TYPE
            || packet[1] != Self::VERSION
            || packet[30] > 1
        {
            return Err("invalid Portal Warp request framing".to_string());
        }
        let request = Self {
            force_random: packet[30] != 0,
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
            return Err("invalid Portal Warp request identity".to_string());
        }
        Ok(request)
    }
}

/// Exact terminal for a server-owned manual Warp. Raw f32 bits make the
/// terminal immutable and replayable while retaining the canonical movement
/// state that seeds the next input record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalWarpAccepted {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) action_id: u32,
    pub(crate) position_sequence: u16,
    pub(crate) server_tick: u32,
    pub(crate) position_x_bits: u32,
    pub(crate) position_y_bits: u32,
    pub(crate) velocity_x_bits: u32,
    pub(crate) velocity_y_bits: u32,
    pub(crate) orientation_bits: u32,
    pub(crate) energy: u16,
    pub(crate) used_portal: bool,
    pub(crate) outcome: u8,
}

impl PortalWarpAccepted {
    pub(crate) const TYPE: u8 = 0x6b;
    pub(crate) const VERSION: u8 = 1;
    pub(crate) const PACKET_LEN: usize = 58;
    pub(crate) const OUTCOME_APPLIED: u8 = 1;
    pub(crate) const OUTCOME_REJECTED: u8 = 2;

    pub(crate) fn encode(self) -> Vec<u8> {
        let request = PortalWarpRequest {
            force_random: false,
            server_session_id: self.server_session_id,
            arena_stream_id: self.arena_stream_id,
            player_id: self.player_id,
            life_seq: self.life_seq,
            action_id: self.action_id,
            position_sequence: self.position_sequence,
        };
        let mut packet = request.encode();
        packet.truncate(30);
        packet[0] = Self::TYPE;
        packet[1] = Self::VERSION;
        packet.extend(self.server_tick.to_le_bytes());
        packet.extend(self.position_x_bits.to_le_bytes());
        packet.extend(self.position_y_bits.to_le_bytes());
        packet.extend(self.velocity_x_bits.to_le_bytes());
        packet.extend(self.velocity_y_bits.to_le_bytes());
        packet.extend(self.orientation_bits.to_le_bytes());
        packet.extend(self.energy.to_le_bytes());
        packet.push(u8::from(self.used_portal));
        packet.push(self.outcome);
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet[0] != Self::TYPE
            || packet[1] != Self::VERSION
            || packet[56] > 1
            || !matches!(packet[57], Self::OUTCOME_APPLIED | Self::OUTCOME_REJECTED)
        {
            return Err("invalid Portal Warp terminal framing".to_string());
        }
        let mut request_packet = packet[..30].to_vec();
        request_packet[0] = PortalWarpRequest::TYPE;
        request_packet[1] = PortalWarpRequest::VERSION;
        request_packet.push(0);
        let request = PortalWarpRequest::parse(&request_packet)?;
        let accepted = Self {
            server_session_id: request.server_session_id,
            arena_stream_id: request.arena_stream_id,
            player_id: request.player_id,
            life_seq: request.life_seq,
            action_id: request.action_id,
            position_sequence: request.position_sequence,
            server_tick: u32::from_le_bytes(packet[30..34].try_into().unwrap()),
            position_x_bits: u32::from_le_bytes(packet[34..38].try_into().unwrap()),
            position_y_bits: u32::from_le_bytes(packet[38..42].try_into().unwrap()),
            velocity_x_bits: u32::from_le_bytes(packet[42..46].try_into().unwrap()),
            velocity_y_bits: u32::from_le_bytes(packet[46..50].try_into().unwrap()),
            orientation_bits: u32::from_le_bytes(packet[50..54].try_into().unwrap()),
            energy: u16::from_le_bytes(packet[54..56].try_into().unwrap()),
            used_portal: packet[56] != 0,
            outcome: packet[57],
        };
        if [
            accepted.position_x_bits,
            accepted.position_y_bits,
            accepted.velocity_x_bits,
            accepted.velocity_y_bits,
            accepted.orientation_bits,
        ]
        .into_iter()
        .any(|bits| !f32::from_bits(bits).is_finite())
        {
            return Err("invalid Portal Warp canonical state".to_string());
        }
        Ok(accepted)
    }

    pub(crate) fn canonical_state(self) -> (f32, f32, f32, f32, f32) {
        (
            f32::from_bits(self.position_x_bits),
            f32::from_bits(self.position_y_bits),
            f32::from_bits(self.velocity_x_bits),
            f32::from_bits(self.velocity_y_bits),
            f32::from_bits(self.orientation_bits),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_terminal_round_trip_exact_wrap_state() {
        let request = PortalWarpRequest {
            force_random: false,
            server_session_id: u64::MAX,
            arena_stream_id: u64::MAX - 1,
            player_id: u16::MAX,
            life_seq: u32::MAX,
            action_id: u32::MAX,
            position_sequence: u16::MAX,
        };
        assert_eq!(PortalWarpRequest::parse(&request.encode()), Ok(request));
        let terminal = PortalWarpAccepted {
            server_session_id: request.server_session_id,
            arena_stream_id: request.arena_stream_id,
            player_id: request.player_id,
            life_seq: request.life_seq,
            action_id: request.action_id,
            position_sequence: request.position_sequence,
            server_tick: u32::MAX,
            position_x_bits: 12.25_f32.to_bits(),
            position_y_bits: 34.5_f32.to_bits(),
            velocity_x_bits: (-56.75_f32).to_bits(),
            velocity_y_bits: 78.0_f32.to_bits(),
            orientation_bits: 1.25_f32.to_bits(),
            energy: u16::MAX,
            used_portal: true,
            outcome: PortalWarpAccepted::OUTCOME_APPLIED,
        };
        assert_eq!(PortalWarpAccepted::parse(&terminal.encode()), Ok(terminal));
    }

    #[test]
    fn random_warp_mode_round_trips_and_rejects_noncanonical_flag() {
        let request = PortalWarpRequest {
            force_random: true,
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            life_seq: 4,
            action_id: 5,
            position_sequence: 6,
        };
        let mut packet = request.encode();
        assert_eq!(packet.len(), 31);
        assert_eq!(packet[1], 2);
        assert_eq!(PortalWarpRequest::parse(&packet), Ok(request));
        packet[30] = 2;
        assert!(PortalWarpRequest::parse(&packet).is_err());
        packet[30] = 1;
        packet[1] = 1;
        assert!(PortalWarpRequest::parse(&packet).is_err());
    }

    #[test]
    fn malformed_identity_boolean_and_float_fail_closed() {
        let mut packet = PortalWarpRequest {
            force_random: false,
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            life_seq: 4,
            action_id: 5,
            position_sequence: 6,
        }
        .encode();
        packet[24..28].copy_from_slice(&0_u32.to_le_bytes());
        assert!(PortalWarpRequest::parse(&packet).is_err());

        let mut terminal = PortalWarpAccepted {
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            life_seq: 4,
            action_id: 5,
            position_sequence: 6,
            server_tick: 7,
            position_x_bits: 1.0_f32.to_bits(),
            position_y_bits: 2.0_f32.to_bits(),
            velocity_x_bits: 3.0_f32.to_bits(),
            velocity_y_bits: 4.0_f32.to_bits(),
            orientation_bits: 5.0_f32.to_bits(),
            energy: 6,
            used_portal: false,
            outcome: PortalWarpAccepted::OUTCOME_REJECTED,
        }
        .encode();
        terminal[56] = 2;
        assert!(PortalWarpAccepted::parse(&terminal).is_err());
        terminal[56] = 0;
        terminal[34..38].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(PortalWarpAccepted::parse(&terminal).is_err());
    }
}
