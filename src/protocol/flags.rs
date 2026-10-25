use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagPosition {
    pub(crate) flag_id: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) owner: u16,
}

impl FlagPosition {
    pub(crate) const TYPE: u8 = 0x12;
    pub(crate) const PACKET_LEN: usize = 9;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "flag position packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected flag position packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            flag_id: read_u16(packet, 1)?,
            x: read_u16(packet, 3)?,
            y: read_u16(packet, 5)?,
            owner: read_u16(packet, 7)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagClaim {
    pub(crate) flag_id: u16,
    pub(crate) player_id: u16,
}

impl FlagClaim {
    pub(crate) const TYPE: u8 = 0x13;
    pub(crate) const PACKET_LEN: usize = 5;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "flag claim packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected flag claim packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            flag_id: read_u16(packet, 1)?,
            player_id: read_u16(packet, 3)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagVictory {
    pub(crate) team: u16,
    pub(crate) jackpot_points: u32,
}

impl FlagVictory {
    pub(crate) const TYPE: u8 = 0x14;
    pub(crate) const PACKET_LEN: usize = 7;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "flag victory packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected flag victory packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            team: read_u16(packet, 1)?,
            jackpot_points: read_u32(packet, 3)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropFlag {
    pub(crate) player_id: u16,
}

impl DropFlag {
    pub(crate) const TYPE: u8 = 0x16;
    pub(crate) const PACKET_LEN: usize = 3;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "drop flag packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected drop flag packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagRewardGrant {
    pub(crate) team: u16,
    pub(crate) points: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagReward {
    pub(crate) grants: Vec<FlagRewardGrant>,
}

impl FlagReward {
    pub(crate) const TYPE: u8 = 0x23;
    pub(crate) const GRANT_LEN: usize = 4;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(Self::TYPE) {
            return Err(format!(
                "unexpected flag reward packet type: {:#04x}",
                packet.first().copied().unwrap_or(0)
            ));
        }
        if !(packet.len() - 1).is_multiple_of(Self::GRANT_LEN) {
            return Err("flag reward packet has trailing bytes".to_string());
        }

        let mut grants = Vec::with_capacity((packet.len() - 1) / Self::GRANT_LEN);
        let mut offset = 1;
        while offset < packet.len() {
            grants.push(FlagRewardGrant {
                team: read_u16(packet, offset)?,
                points: read_u16(packet, offset + 2)?,
            });
            offset += Self::GRANT_LEN;
        }

        Ok(Self { grants })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurfFlagUpdate {
    pub(crate) owners: Vec<u16>,
}

impl TurfFlagUpdate {
    pub(crate) const TYPE: u8 = 0x22;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(Self::TYPE) {
            return Err(format!(
                "unexpected turf flag update packet type: {:#04x}",
                packet.first().copied().unwrap_or(0)
            ));
        }
        if !(packet.len() - 1).is_multiple_of(2) {
            return Err("turf flag update packet has trailing byte".to_string());
        }

        let mut owners = Vec::with_capacity((packet.len() - 1) / 2);
        let mut offset = 1;
        while offset < packet.len() {
            owners.push(read_u16(packet, offset)?);
            offset += 2;
        }
        Ok(Self { owners })
    }
}
