use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateTurretLink {
    pub(crate) requester_id: u16,
    pub(crate) destination_id: Option<u16>,
}

impl CreateTurretLink {
    pub(crate) const TYPE: u8 = 0x0e;
    pub(crate) const MIN_PACKET_LEN: usize = 3;
    pub(crate) const FULL_PACKET_LEN: usize = 5;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::MIN_PACKET_LEN {
            return Err(format!(
                "create turret packet too short: {} < {}",
                packet.len(),
                Self::MIN_PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected create turret packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            requester_id: read_u16(packet, 1)?,
            destination_id: if packet.len() >= Self::FULL_PACKET_LEN {
                Some(read_u16(packet, 3)?)
            } else {
                None
            },
        })
    }

    pub(crate) fn attached_destination(self) -> Option<u16> {
        self.destination_id
            .filter(|destination_id| *destination_id != INVALID_PLAYER_ID)
    }

    pub(crate) fn detach_target(self, self_player_id: Option<u16>) -> Option<u16> {
        match self.destination_id {
            Some(INVALID_PLAYER_ID) => Some(self.requester_id),
            None => self_player_id.or(Some(self.requester_id)),
            Some(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DestroyTurretLink {
    pub(crate) player_id: u16,
}

impl DestroyTurretLink {
    pub(crate) const TYPE: u8 = 0x15;
    pub(crate) const PACKET_LEN: usize = 3;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "destroy turret packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected destroy turret packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectatePacket {
    pub(crate) extra_position_info: bool,
}

impl SpectatePacket {
    pub(crate) const TYPE: u8 = 0x1c;
    pub(crate) const PACKET_LEN: usize = 2;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "spectate packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected spectate packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            extra_position_info: packet[1] != 0,
        })
    }
}
