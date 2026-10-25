use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatPacket {
    pub(crate) chat_type: u8,
    pub(crate) sound: u8,
    pub(crate) sender_id: u16,
    pub(crate) message: String,
}

impl ChatPacket {
    pub(crate) const TYPE: u8 = 0x07;
    pub(crate) const MIN_PACKET_LEN: usize = 5;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::MIN_PACKET_LEN {
            return Err(format!(
                "chat packet too short: {} < {}",
                packet.len(),
                Self::MIN_PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!("unexpected chat packet type: {:#04x}", packet[0]));
        }

        Ok(Self {
            chat_type: packet[1],
            sound: packet[2],
            sender_id: read_u16(packet, 3)?,
            message: read_lossy_c_string(&packet[5..]),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessageRequest {
    pub(crate) chat_type: u8,
    pub(crate) sound: u8,
    pub(crate) target: u16,
    pub(crate) message: String,
}

impl ChatMessageRequest {
    pub(crate) const TYPE: u8 = 0x06;
    #[allow(dead_code)]
    pub(crate) const PUBLIC_TYPE: u8 = 2;
    pub(crate) const TEAM_TYPE: u8 = 3;
    pub(crate) const PRIVATE_TYPE: u8 = 5;

    #[allow(dead_code)]
    pub(crate) fn public(message: impl Into<String>) -> Self {
        Self {
            chat_type: Self::PUBLIC_TYPE,
            sound: 0,
            target: 0,
            message: message.into(),
        }
    }

    pub(crate) fn team(message: impl Into<String>) -> Self {
        Self {
            chat_type: Self::TEAM_TYPE,
            sound: 0,
            target: 0,
            message: message.into(),
        }
    }

    pub(crate) fn private(target: u16, message: impl Into<String>) -> Self {
        Self {
            chat_type: Self::PRIVATE_TYPE,
            sound: 0,
            target,
            message: message.into(),
        }
    }

    pub(crate) fn with_sound(mut self, sound: u8) -> Self {
        self.sound = sound;
        self
    }

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(5 + self.message.len() + 1);
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.chat_type);
        push_u8(&mut bytes, self.sound);
        push_u16(&mut bytes, self.target);
        bytes.extend_from_slice(self.message.as_bytes());
        push_u8(&mut bytes, 0);
        bytes
    }
}
