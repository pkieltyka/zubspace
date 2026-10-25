use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerIdAssignment {
    pub(crate) player_id: u16,
}

impl PlayerIdAssignment {
    pub(crate) const TYPE: u8 = 0x01;
    pub(crate) const PACKET_LEN: usize = 3;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "player id packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player id packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerLeaving {
    pub(crate) player_id: u16,
}

impl PlayerLeaving {
    pub(crate) const TYPE: u8 = 0x04;
    pub(crate) const PACKET_LEN: usize = 3;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "player leaving packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player leaving packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
        })
    }
}
