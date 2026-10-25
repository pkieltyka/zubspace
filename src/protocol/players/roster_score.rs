use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerDeath {
    pub(crate) green_id: i8,
    /// Server-issued collectible identity for the optional death green.
    /// Zero means the death did not create a collectible.
    pub(crate) green_collectible_id: u32,
    pub(crate) killer_id: u16,
    pub(crate) killed_id: u16,
    pub(crate) bounty: u16,
    pub(crate) flag_transfer: u16,
    /// Server lifecycle generation entered by this death.
    pub(crate) life_seq: u32,
    /// Absolute server tick at which the exact respawn becomes admissible.
    pub(crate) respawn_at_tick: u32,
}

impl PlayerDeath {
    pub(crate) const TYPE: u8 = 0x06;
    pub(crate) const PACKET_LEN: usize = 22;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "player death packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player death packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            green_id: packet[1] as i8,
            killer_id: read_u16(packet, 2)?,
            killed_id: read_u16(packet, 4)?,
            bounty: read_u16(packet, 6)?,
            flag_transfer: read_u16(packet, 8)?,
            green_collectible_id: read_u32(packet, 10)?,
            life_seq: read_u32(packet, 14)?,
            respawn_at_tick: read_u32(packet, 18)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRosterEntry {
    pub(crate) ship: u8,
    pub(crate) audio: u8,
    pub(crate) name: String,
    pub(crate) squad: String,
    pub(crate) kill_points: u32,
    pub(crate) flag_points: u32,
    pub(crate) player_id: u16,
    pub(crate) frequency: u16,
    pub(crate) wins: u16,
    pub(crate) losses: u16,
    pub(crate) attach_parent: u16,
    pub(crate) flags: u16,
    pub(crate) koth: u8,
}

impl PlayerRosterEntry {
    pub(crate) const TYPE: u8 = 0x03;
    pub(crate) const PACKET_LEN: usize = 64;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "player entering packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player entering packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            ship: packet[1],
            audio: packet[2],
            name: read_fixed_c_string(packet, 3, 20)?,
            squad: read_fixed_c_string(packet, 23, 20)?,
            kill_points: read_u32(packet, 43)?,
            flag_points: read_u32(packet, 47)?,
            player_id: read_u16(packet, 51)?,
            frequency: read_u16(packet, 53)?,
            wins: read_u16(packet, 55)?,
            losses: read_u16(packet, 57)?,
            attach_parent: read_u16(packet, 59)?,
            flags: read_u16(packet, 61)?,
            koth: packet[63],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreUpdate {
    pub(crate) assists: u32,
    pub(crate) player_id: u16,
    pub(crate) kill_points: u32,
    pub(crate) flag_points: u32,
    pub(crate) wins: u16,
    pub(crate) losses: u16,
}

impl ScoreUpdate {
    pub(crate) const TYPE: u8 = 0x09;
    pub(crate) const PACKET_LEN: usize = 15;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "score update packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected score update packet type: {:#04x}",
                packet[0]
            ));
        }

        let assists = if packet.len() == Self::PACKET_LEN {
            0
        } else if packet.len() == 20 && packet[15] == 1 {
            read_u32(packet, 16)?
        } else {
            return Err("unsupported score update extension".to_owned());
        };
        Ok(Self {
            assists,
            player_id: read_u16(packet, 1)?,
            kill_points: read_u32(packet, 3)?,
            flag_points: read_u32(packet, 7)?,
            wins: read_u16(packet, 11)?,
            losses: read_u16(packet, 13)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetScore {
    pub(crate) player_id: u16,
}

impl ResetScore {
    pub(crate) const TYPE: u8 = 0x1a;
    pub(crate) const PACKET_LEN: usize = 3;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "reset score packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected reset score packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
        })
    }

    pub(crate) fn applies_to(self, player_id: u16) -> bool {
        self.player_id == INVALID_PLAYER_ID || self.player_id == player_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoccerGoal {
    pub(crate) frequency: u16,
    pub(crate) team_points: u32,
}

impl SoccerGoal {
    pub(crate) const TYPE: u8 = 0x0b;
    pub(crate) const PACKET_LEN: usize = 7;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "soccer goal packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected soccer goal packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            frequency: read_u16(packet, 1)?,
            team_points: read_u32(packet, 3)?,
        })
    }
}
