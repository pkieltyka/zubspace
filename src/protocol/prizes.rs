use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerPrize {
    pub(crate) collectible_id: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) prize_id: i16,
    pub(crate) player_id: u16,
}

impl PlayerPrize {
    pub(crate) const TYPE: u8 = 0x08;
    pub(crate) const PACKET_LEN: usize = 13;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "player prize packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player prize packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            collectible_id: read_u32(packet, 1)?,
            x: read_u16(packet, 5)?,
            y: read_u16(packet, 7)?,
            prize_id: read_i16(packet, 9)?,
            player_id: read_u16(packet, 11)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrizePickupAck {
    /// Exact server process and arena-stream generation which issued this
    /// private acknowledgement. These identities travel in the packet so an
    /// old reliable payload cannot be rebound after a new bootstrap.
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) collectible_id: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) prize_id: i16,
    /// Server-owned energy at the instant the prize effect was accepted.
    /// Positive Warp uses this as the exact relocation witness, avoiding any
    /// dependence on the client's recharge prediction while the reliable ACK
    /// is in flight.
    pub(crate) authoritative_energy: u16,
    /// Exact authoritative life which owns this collectible delivery and any
    /// positive-Warp relocation marker derived from it.
    pub(crate) life_seq: u32,
    /// PrizeState revision produced by this exact recipient-private effect.
    pub(crate) prize_state_revision: u32,
    /// Absolute authoritative deadline for Engine Shutdown. Presence is
    /// carried separately on the wire, because tick zero is a valid wrapped
    /// deadline.
    pub(crate) engine_shutdown_until_tick: Option<u32>,
    /// This delivery changes movement semantics and must complete the exact
    /// PrizeEffectApplied/Accepted barrier before gameplay resumes. The field
    /// keeps its original Shutdown-specific name for wire/source compatibility.
    pub(crate) engine_shutdown_application_required: bool,
}

impl PrizePickupAck {
    pub(crate) const TYPE: u8 = 0x61;
    pub(crate) const PACKET_LEN: usize = 44;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "prize pickup ack packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected prize pickup ack packet type: {:#04x}",
                packet[0]
            ));
        }

        let ack = Self {
            server_session_id: read_u64(packet, 26)?,
            arena_stream_id: read_u64(packet, 34)?,
            player_id: read_u16(packet, 42)?,
            collectible_id: read_u32(packet, 1)?,
            x: read_u16(packet, 5)?,
            y: read_u16(packet, 7)?,
            prize_id: read_i16(packet, 9)?,
            authoritative_energy: read_u16(packet, 11)?,
            life_seq: read_u32(packet, 13)?,
            prize_state_revision: read_u32(packet, 17)?,
            engine_shutdown_until_tick: (packet[25] & (1 << 1) != 0)
                .then(|| read_u32(packet, 21))
                .transpose()?,
            engine_shutdown_application_required: packet[25] & 1 != 0,
        };
        if ack.server_session_id == 0
            || ack.arena_stream_id == 0
            || ack.player_id == u16::MAX
            || ack.collectible_id == 0
            || ack.life_seq == 0
            || ack.prize_state_revision == 0
            || ack.engine_shutdown_until_tick.is_some() && ack.prize_id.checked_abs() != Some(14)
            || ack.engine_shutdown_application_required
                && !matches!(ack.prize_id.checked_abs(), Some(1 | 3 | 11 | 12 | 14))
                && ack.prize_id != 25
            || ack.engine_shutdown_application_required
                && ack.prize_id.checked_abs() == Some(14)
                && ack.engine_shutdown_until_tick.is_none()
            || packet[25] & !0b11 != 0
        {
            return Err("invalid prize pickup acknowledgement identity".to_string());
        }
        Ok(ack)
    }
}

fn read_u64(packet: &[u8], offset: usize) -> Result<u64, String> {
    let bytes = packet
        .get(offset..offset + 8)
        .ok_or_else(|| "unexpected EOF reading u64".to_string())?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollectedPrize {
    pub(crate) count: u16,
    pub(crate) prize_id: i16,
}

impl CollectedPrize {
    pub(crate) const TYPE: u8 = 0x20;
    pub(crate) const PACKET_LEN: usize = 5;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "collected prize packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected collected prize packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            count: read_u16(packet, 1)?,
            prize_id: read_i16(packet, 3)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HiddenPrizeSpawn {
    pub(crate) collectible_id: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) prize_id: i16,
    pub(crate) ttl_ticks: u16,
}

impl HiddenPrizeSpawn {
    pub(crate) const TYPE: u8 = 0x60;
    pub(crate) const PACKET_LEN: usize = 13;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "hidden prize spawn packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected hidden prize spawn packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            collectible_id: read_u32(packet, 1)?,
            x: read_u16(packet, 5)?,
            y: read_u16(packet, 7)?,
            prize_id: read_i16(packet, 9)?,
            ttl_ticks: read_u16(packet, 11)?,
        })
    }
}
