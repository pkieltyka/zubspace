use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaDirectoryEntry {
    pub(crate) name: String,
    pub(crate) population: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArenaDirectoryListing {
    pub(crate) arenas: Vec<ArenaDirectoryEntry>,
}

impl ArenaDirectoryListing {
    pub(crate) const TYPE: u8 = 0x2f;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.is_empty() {
            return Err("arena directory packet is empty".to_string());
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected arena directory packet type: {:#04x}",
                packet[0]
            ));
        }

        let mut arenas = Vec::new();
        let mut offset = 1;
        while offset < packet.len() {
            let (name, next_offset) = read_c_string_at(packet, offset)?;
            if next_offset + 2 > packet.len() {
                return Err("unexpected EOF reading arena population".to_string());
            }
            let population = read_u16(packet, next_offset)?;
            arenas.push(ArenaDirectoryEntry { name, population });
            offset = next_offset + 2;
        }

        Ok(Self { arenas })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneBannerAdvertisement {
    pub(crate) display_mode: u8,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) duration: u32,
    pub(crate) pixels: Vec<u8>,
}

impl ZoneBannerAdvertisement {
    pub(crate) const TYPE: u8 = 0x30;
    pub(crate) const HEADER_LEN: usize = 10;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_LEN {
            return Err(format!(
                "zone banner packet too short: {} < {}",
                packet.len(),
                Self::HEADER_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected zone banner packet type: {:#04x}",
                packet[0]
            ));
        }

        let width = read_u16(packet, 2)?;
        let height = read_u16(packet, 4)?;
        let pixel_count = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or_else(|| "zone banner dimensions overflow".to_string())?;
        let expected_len = Self::HEADER_LEN
            .checked_add(pixel_count)
            .ok_or_else(|| "zone banner packet length overflow".to_string())?;
        if packet.len() != expected_len {
            return Err(format!(
                "zone banner packet length mismatch: {} != {}",
                packet.len(),
                expected_len
            ));
        }

        Ok(Self {
            display_mode: packet[1],
            width,
            height,
            duration: read_u32(packet, 6)?,
            pixels: packet[Self::HEADER_LEN..].to_vec(),
        })
    }
}
