use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipReset;

impl ShipReset {
    pub(crate) const TYPE: u8 = 0x1b;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive;

impl KeepAlive {
    pub(crate) const TYPE: u8 = 0x27;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet != [Self::TYPE] {
            return Err(format!(
                "keep-alive packet length mismatch: {} != 1",
                packet.len()
            ));
        }

        Ok(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostLogin;

impl PostLogin {
    pub(crate) const TYPE: u8 = 0x31;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet != [Self::TYPE] {
            return Err(format!(
                "post-login packet length mismatch: {} != 1",
                packet.len()
            ));
        }

        Ok(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetCoordinates {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

impl SetCoordinates {
    pub(crate) const TYPE: u8 = 0x32;
    pub(crate) const PACKET_LEN: usize = 5;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "set coordinates packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected set coordinates packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            x: read_u16(packet, 1)?,
            y: read_u16(packet, 3)?,
        })
    }

    pub(crate) fn position(self) -> Vec2 {
        vec2(self.x as f32 + 0.5, self.y as f32 + 0.5)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRedirectPacket {
    pub(crate) payload: Vec<u8>,
}

impl ServerRedirectPacket {
    pub(crate) const TYPE: u8 = 0x3b;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.is_empty() {
            return Err("redirect packet is empty".to_string());
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected redirect packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            payload: packet[1..].to_vec(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectBoxPacket {
    pub(crate) payload: Vec<u8>,
}

impl SelectBoxPacket {
    pub(crate) const TYPE: u8 = 0x3c;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.is_empty() {
            return Err("select-box packet is empty".to_string());
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected select-box packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            payload: packet[1..].to_vec(),
        })
    }
}
