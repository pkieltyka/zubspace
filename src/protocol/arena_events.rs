use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleUfoPacket {
    pub(crate) available: bool,
}

impl ToggleUfoPacket {
    pub(crate) const TYPE: u8 = 0x25;
    pub(crate) const PACKET_LEN: usize = 2;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "toggle ufo packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected toggle ufo packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            available: packet[1] != 0,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetPersonalKothTimer {
    pub(crate) timer: u32,
}

impl SetPersonalKothTimer {
    pub(crate) const TYPE: u8 = 0x2b;
    pub(crate) const PACKET_LEN: usize = 5;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "personal koth timer packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected personal koth timer packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            timer: read_u32(packet, 1)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KothGameReset {
    pub(crate) adding_timer: bool,
    pub(crate) timer: u32,
    pub(crate) player_id: u16,
}

impl KothGameReset {
    pub(crate) const TYPE: u8 = 0x2c;
    pub(crate) const PACKET_LEN: usize = 8;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "koth game reset packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected koth game reset packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            adding_timer: packet[1] != 0,
            timer: read_u32(packet, 2)?,
            player_id: read_u16(packet, 6)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddKothTime {
    pub(crate) additional_time: u32,
}

impl AddKothTime {
    pub(crate) const TYPE: u8 = 0x2d;
    pub(crate) const PACKET_LEN: usize = 5;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "add koth time packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected add koth time packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            additional_time: read_u32(packet, 1)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeedGameOver {
    pub(crate) best: bool,
    pub(crate) rank: u16,
    pub(crate) score: u32,
    pub(crate) player_scores: [u32; 5],
    pub(crate) player_ids: [u16; 5],
}

impl SpeedGameOver {
    pub(crate) const TYPE: u8 = 0x24;
    pub(crate) const PACKET_LEN: usize = 38;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "speed game over packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected speed game over packet type: {:#04x}",
                packet[0]
            ));
        }

        let mut player_scores = [0; 5];
        for (index, score) in player_scores.iter_mut().enumerate() {
            *score = read_u32(packet, 8 + index * 4)?;
        }

        let mut player_ids = [0; 5];
        for (index, player_id) in player_ids.iter_mut().enumerate() {
            *player_id = read_u16(packet, 28 + index * 2)?;
        }

        Ok(Self {
            best: packet[1] != 0,
            rank: read_u16(packet, 2)?,
            score: read_u32(packet, 4)?,
            player_scores,
            player_ids,
        })
    }
}
