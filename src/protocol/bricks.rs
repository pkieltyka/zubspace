use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrickDrop {
    pub(crate) x1: u16,
    pub(crate) y1: u16,
    pub(crate) x2: u16,
    pub(crate) y2: u16,
    pub(crate) team: u16,
    pub(crate) brick_id: u16,
    pub(crate) timestamp: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrickDropped {
    Clear,
    Drop(BrickDrop),
}

impl BrickDropped {
    pub(crate) const TYPE: u8 = 0x21;
    pub(crate) const PACKET_LEN: usize = 17;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(Self::TYPE) {
            return Err(format!(
                "unexpected brick dropped packet type: {:#04x}",
                packet.first().copied().unwrap_or(0)
            ));
        }
        if packet.len() < Self::PACKET_LEN {
            return Ok(Self::Clear);
        }

        Ok(Self::Drop(BrickDrop {
            x1: read_u16(packet, 1)?,
            y1: read_u16(packet, 3)?,
            x2: read_u16(packet, 5)?,
            y2: read_u16(packet, 7)?,
            team: read_u16(packet, 9)?,
            brick_id: read_u16(packet, 11)?,
            timestamp: read_u32(packet, 13)?,
        }))
    }
}
