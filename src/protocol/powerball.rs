use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerballPositionUpdate {
    pub(crate) ball_id: u8,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) owner_id: u16,
    pub(crate) timestamp: u32,
}

impl PowerballPositionUpdate {
    pub(crate) const TYPE: u8 = 0x2e;
    pub(crate) const PACKET_LEN: usize = 16;
    pub(crate) const INVALID_OWNER: u16 = 0xffff;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "powerball position packet has invalid length: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err("not a powerball position packet".to_string());
        }

        Ok(Self {
            ball_id: packet[1],
            x: read_u16(packet, 2)?,
            y: read_u16(packet, 4)?,
            velocity_x: read_i16(packet, 6)?,
            velocity_y: read_i16(packet, 8)?,
            owner_id: read_u16(packet, 10)?,
            timestamp: read_u32(packet, 12)? & 0x7fff_ffff,
        })
    }

    pub(crate) fn owner(self) -> Option<u16> {
        (self.owner_id != Self::INVALID_OWNER).then_some(self.owner_id)
    }
}
