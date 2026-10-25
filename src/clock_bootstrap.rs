// Shared by the WASM client and native server. Each binary uses the opposite
// side of the codec (`parse` vs `encode`), so per-bin dead-code warnings are
// expected without this module-level allow.
#![allow(dead_code)]

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerClockBootstrap {
    pub(crate) server_session_id: u64,
    /// Never-reused generation allocated for this exact arena join. Realtime
    /// state and event packets must match it before the client applies bytes.
    pub(crate) arena_stream_id: u64,
    pub(crate) server_tick: u32,
    pub(crate) input_sequence_base: u16,
    pub(crate) position_sequence_base: u16,
    pub(crate) weapon_sequence_base: u16,
    pub(crate) reliable_sequence_base: u16,
    pub(crate) tick_rate_hz: u16,
    pub(crate) snapshot_rate_hz: u16,
}

impl ServerClockBootstrap {
    pub(crate) const TYPE: u8 = 0x72;
    /// Version 3 binds realtime state and one-shot events to this exact arena
    /// join, independently of reusable snapshot/player ids.
    pub(crate) const VERSION: u8 = 3;
    pub(crate) const PACKET_LEN: usize = 1 + 1 + 8 + 8 + 4 + 2 + 2 + 2 + 2 + 2 + 2;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::PACKET_LEN);
        bytes.push(Self::TYPE);
        bytes.push(Self::VERSION);
        bytes.extend_from_slice(&self.server_session_id.to_le_bytes());
        bytes.extend_from_slice(&self.arena_stream_id.to_le_bytes());
        bytes.extend_from_slice(&self.server_tick.to_le_bytes());
        bytes.extend_from_slice(&self.input_sequence_base.to_le_bytes());
        bytes.extend_from_slice(&self.position_sequence_base.to_le_bytes());
        bytes.extend_from_slice(&self.weapon_sequence_base.to_le_bytes());
        bytes.extend_from_slice(&self.reliable_sequence_base.to_le_bytes());
        bytes.extend_from_slice(&self.tick_rate_hz.to_le_bytes());
        bytes.extend_from_slice(&self.snapshot_rate_hz.to_le_bytes());
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "server clock bootstrap length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected server clock bootstrap type: {:#04x}",
                packet[0]
            ));
        }
        if packet[1] != Self::VERSION {
            return Err(format!(
                "unsupported server clock bootstrap version: {}",
                packet[1]
            ));
        }

        let bootstrap = Self {
            server_session_id: read_u64(packet, 2)?,
            arena_stream_id: read_u64(packet, 10)?,
            server_tick: read_u32(packet, 18)?,
            input_sequence_base: read_u16(packet, 22)?,
            position_sequence_base: read_u16(packet, 24)?,
            weapon_sequence_base: read_u16(packet, 26)?,
            reliable_sequence_base: read_u16(packet, 28)?,
            tick_rate_hz: read_u16(packet, 30)?,
            snapshot_rate_hz: read_u16(packet, 32)?,
        };
        if bootstrap.server_session_id == 0 || bootstrap.arena_stream_id == 0 {
            return Err("server clock bootstrap identities must be nonzero".to_string());
        }
        Ok(bootstrap)
    }
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| "unexpected EOF reading u16".to_string())?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| "unexpected EOF reading u32".to_string())?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, String> {
    let bytes = data
        .get(offset..offset + 8)
        .ok_or_else(|| "unexpected EOF reading u64".to_string())?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}
