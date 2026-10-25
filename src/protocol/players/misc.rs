use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginFailurePacket {
    pub(crate) reason: String,
}

impl LoginFailurePacket {
    pub(crate) const TYPE: u8 = 0x33;
    pub(crate) const MIN_PACKET_LEN: usize = 2;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::MIN_PACKET_LEN {
            return Err(format!(
                "login failure packet too short: {} < {}",
                packet.len(),
                Self::MIN_PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected login failure packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            reason: read_lossy_c_string(&packet[1..]),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerVersionPacket {
    pub(crate) version: u16,
    pub(crate) checksum: u32,
}

impl ServerVersionPacket {
    pub(crate) const TYPE: u8 = 0x34;
    pub(crate) const PACKET_LEN: usize = 7;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "server version packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected server version packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            version: read_u16(packet, 1)?,
            checksum: read_u32(packet, 3)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalBannerChange {
    pub(crate) banner: [u8; PLAYER_BANNER_BYTES],
}

impl PersonalBannerChange {
    pub(crate) const TYPE: u8 = 0x1e;
    pub(crate) const PACKET_LEN: usize = 1 + PLAYER_BANNER_BYTES;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "personal banner packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected personal banner packet type: {:#04x}",
                packet[0]
            ));
        }

        let mut banner = [0; PLAYER_BANNER_BYTES];
        banner.copy_from_slice(&packet[1..Self::PACKET_LEN]);

        Ok(Self { banner })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerBannerChange {
    pub(crate) player_id: u16,
    pub(crate) banner: [u8; PLAYER_BANNER_BYTES],
}

impl PlayerBannerChange {
    pub(crate) const TYPE: u8 = 0x1f;
    pub(crate) const PACKET_LEN: usize = 3 + PLAYER_BANNER_BYTES;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "player banner packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player banner packet type: {:#04x}",
                packet[0]
            ));
        }

        let mut banner = [0; PLAYER_BANNER_BYTES];
        banner.copy_from_slice(&packet[3..Self::PACKET_LEN]);

        Ok(Self {
            player_id: read_u16(packet, 1)?,
            banner,
        })
    }
}
