use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerPositionUpdate {
    pub(crate) player_id: u16,
    pub(crate) direction: u8,
    pub(crate) timestamp: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) togglables: u8,
    pub(crate) ping: u8,
    pub(crate) bounty: u16,
    pub(crate) weapon: u16,
    pub(crate) checksum: Option<u8>,
    pub(crate) energy: Option<u16>,
    pub(crate) s2c_latency: Option<u16>,
    pub(crate) flag_timer: Option<u16>,
    pub(crate) items: Option<u32>,
}

impl PlayerPositionUpdate {
    pub(crate) const LARGE_TYPE: u8 = 0x05;
    pub(crate) const SMALL_TYPE: u8 = 0x28;
    pub(crate) const BATCHED_SMALL_TYPE: u8 = 0x39;
    pub(crate) const BATCHED_LARGE_TYPE: u8 = 0x3a;
    pub(crate) const LARGE_MIN_LEN: usize = 21;
    pub(crate) const SMALL_MIN_LEN: usize = 16;
    pub(crate) const BATCHED_SMALL_ENTRY_LEN: usize = 10;
    pub(crate) const BATCHED_LARGE_ENTRY_LEN: usize = 11;

    #[allow(dead_code)]
    pub(crate) fn parse_large(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::LARGE_MIN_LEN {
            return Err(format!(
                "large position packet too short: {} < {}",
                packet.len(),
                Self::LARGE_MIN_LEN
            ));
        }
        if packet[0] != Self::LARGE_TYPE {
            return Err(format!(
                "unexpected large position packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 8)?,
            direction: packet[1],
            timestamp: read_u16(packet, 2)?,
            x: read_u16(packet, 4)?,
            y: read_u16(packet, 15)?,
            velocity_x: read_i16(packet, 10)?,
            velocity_y: read_i16(packet, 6)?,
            togglables: packet[13],
            ping: packet[14],
            bounty: read_u16(packet, 17)?,
            weapon: read_u16(packet, 19)?,
            checksum: Some(packet[12]),
            energy: read_optional_u16(packet, 21)?,
            s2c_latency: read_optional_u16(packet, 23)?,
            flag_timer: read_optional_u16(packet, 25)?,
            items: read_optional_u32(packet, 27)?,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn parse_small(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::SMALL_MIN_LEN {
            return Err(format!(
                "small position packet too short: {} < {}",
                packet.len(),
                Self::SMALL_MIN_LEN
            ));
        }
        if packet[0] != Self::SMALL_TYPE {
            return Err(format!(
                "unexpected small position packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: packet[8] as u16,
            direction: packet[1],
            timestamp: read_u16(packet, 2)?,
            x: read_u16(packet, 4)?,
            y: read_u16(packet, 12)?,
            velocity_x: read_i16(packet, 14)?,
            velocity_y: read_i16(packet, 10)?,
            togglables: packet[9],
            ping: packet[6],
            bounty: packet[7] as u16,
            weapon: 0,
            checksum: None,
            energy: read_optional_u16(packet, 16)?,
            s2c_latency: read_optional_u16(packet, 18)?,
            flag_timer: read_optional_u16(packet, 20)?,
            items: read_optional_u32(packet, 22)?,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn parse_batched_small(packet: &[u8]) -> Result<Vec<Self>, String> {
        parse_batched_positions(
            packet,
            Self::BATCHED_SMALL_TYPE,
            Self::BATCHED_SMALL_ENTRY_LEN,
            false,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn parse_batched_large(packet: &[u8]) -> Result<Vec<Self>, String> {
        parse_batched_positions(
            packet,
            Self::BATCHED_LARGE_TYPE,
            Self::BATCHED_LARGE_ENTRY_LEN,
            true,
        )
    }
}

pub(crate) fn parse_batched_positions(
    packet: &[u8],
    packet_type: u8,
    entry_len: usize,
    includes_togglables: bool,
) -> Result<Vec<PlayerPositionUpdate>, String> {
    if packet.first().copied() != Some(packet_type) {
        return Err(format!(
            "unexpected batched position packet type: {:#04x}",
            packet.first().copied().unwrap_or(0)
        ));
    }
    if packet.len() < 1 + entry_len {
        return Err(format!(
            "batched position packet too short: {} < {}",
            packet.len(),
            1 + entry_len
        ));
    }
    if !(packet.len() - 1).is_multiple_of(entry_len) {
        return Err("batched position packet has trailing bytes".to_string());
    }

    let mut updates = Vec::with_capacity((packet.len() - 1) / entry_len);
    let mut offset = 1;
    while offset < packet.len() {
        let (player_id, togglables, cursor) = if includes_togglables {
            let packed = read_u16(packet, offset)?;
            (packed & 0x03ff, (packed >> 10) as u8, offset + 2)
        } else {
            (packet[offset] as u16, 0, offset + 1)
        };
        let packed_heading = read_u16(packet, cursor)?;
        let direction = (packed_heading >> 10) as u8;
        let timestamp = packed_heading & 0x03ff;
        let packed_pos = read_u32(packet, cursor + 2)?;
        let x = (packed_pos & 0x3fff) as u16;
        let y = ((packed_pos >> 14) & 0x3fff) as u16;
        let packed_velocity = read_u16(packet, cursor + 6)?;
        let velocity_y = sign_extend_i16((packed_velocity & 0x3fff) as u16, 14);
        let multiplier = packet[cursor + 8] as i8 as i16;
        let velocity_x =
            (((packed_velocity >> 14) as i16) + multiplier * 4) * 16 + ((packed_pos >> 28) as i16);

        updates.push(PlayerPositionUpdate {
            player_id,
            direction,
            timestamp,
            x,
            y,
            velocity_x,
            velocity_y,
            togglables,
            ping: 0,
            bounty: 0,
            weapon: 0,
            checksum: None,
            energy: None,
            s2c_latency: None,
            flag_timer: None,
            items: None,
        });

        offset += entry_len;
    }

    Ok(updates)
}
