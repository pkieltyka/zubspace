use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LvzObjectToggle {
    pub(crate) object_id: u16,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LvzTogglePacket {
    pub(crate) toggles: Vec<LvzObjectToggle>,
}

impl LvzTogglePacket {
    pub(crate) const TYPE: u8 = 0x35;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + self.toggles.len() * 2);
        push_u8(&mut bytes, Self::TYPE);
        for toggle in &self.toggles {
            let mut bits = toggle.object_id & 0x7fff;
            if !toggle.enabled {
                bits |= 0x8000;
            }
            push_u16(&mut bytes, bits);
        }
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(Self::TYPE) {
            return Err("not an LVZ toggle packet".to_string());
        }
        if packet.len().is_multiple_of(2) {
            return Err("LVZ toggle packet has trailing byte".to_string());
        }

        let mut toggles = Vec::with_capacity((packet.len() - 1) / 2);
        let mut offset = 1;
        while offset < packet.len() {
            let bits = read_u16(packet, offset)?;
            toggles.push(LvzObjectToggle {
                object_id: bits & 0x7fff,
                enabled: bits & 0x8000 == 0,
            });
            offset += 2;
        }

        Ok(Self { toggles })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LvzModificationFlags {
    pub(crate) xy: bool,
    pub(crate) image: bool,
    pub(crate) layer: bool,
    pub(crate) time: bool,
    pub(crate) mode: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LvzModifyPacket {
    pub(crate) changed: LvzModificationFlags,
    pub(crate) map_object: bool,
    pub(crate) object_id: u16,
    pub(crate) map_x: i16,
    pub(crate) map_y: i16,
    pub(crate) screen_x: i16,
    pub(crate) screen_y: i16,
    pub(crate) x_type: u8,
    pub(crate) y_type: u8,
    pub(crate) image_id: u8,
    pub(crate) layer: u8,
    pub(crate) display_time: u16,
    pub(crate) display_mode: u8,
}

impl LvzModifyPacket {
    pub(crate) const TYPE: u8 = 0x36;
    pub(crate) const PACKET_SIZE: usize = 12;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::PACKET_SIZE);
        push_u8(&mut bytes, Self::TYPE);
        let mut flags = 0;
        if self.changed.xy {
            flags |= 0x01;
        }
        if self.changed.image {
            flags |= 0x02;
        }
        if self.changed.layer {
            flags |= 0x04;
        }
        if self.changed.time {
            flags |= 0x08;
        }
        if self.changed.mode {
            flags |= 0x10;
        }
        push_u8(&mut bytes, flags);
        push_u16(
            &mut bytes,
            (self.object_id << 1) | u16::from(self.map_object),
        );
        if self.map_object {
            push_u16(&mut bytes, self.map_x as u16);
            push_u16(&mut bytes, self.map_y as u16);
        } else {
            push_u16(
                &mut bytes,
                (((self.screen_x as u16) & 0x0fff) << 4) | u16::from(self.x_type & 0x0f),
            );
            push_u16(
                &mut bytes,
                (((self.screen_y as u16) & 0x0fff) << 4) | u16::from(self.y_type & 0x0f),
            );
        }
        push_u8(&mut bytes, self.image_id);
        push_u8(&mut bytes, self.layer);
        push_u16(
            &mut bytes,
            (self.display_time & 0x0fff) | (u16::from(self.display_mode & 0x0f) << 12),
        );
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(Self::TYPE) {
            return Err("not an LVZ modify packet".to_string());
        }
        if packet.len() != Self::PACKET_SIZE {
            return Err("LVZ modify packet has invalid length".to_string());
        }

        let flags = packet[1];
        let id_bits = read_u16(packet, 2)?;
        let x_bits = read_u16(packet, 4)?;
        let y_bits = read_u16(packet, 6)?;
        let display_bits = read_u16(packet, 10)?;

        Ok(Self {
            changed: LvzModificationFlags {
                xy: flags & 0x01 != 0,
                image: flags & 0x02 != 0,
                layer: flags & 0x04 != 0,
                time: flags & 0x08 != 0,
                mode: flags & 0x10 != 0,
            },
            map_object: id_bits & 1 != 0,
            object_id: id_bits >> 1,
            map_x: x_bits as i16,
            map_y: y_bits as i16,
            screen_x: sign_extend_i16(x_bits >> 4, 12),
            screen_y: sign_extend_i16(y_bits >> 4, 12),
            x_type: (x_bits & 0x000f) as u8,
            y_type: (y_bits & 0x000f) as u8,
            image_id: packet[8],
            layer: packet[9],
            display_time: display_bits & 0x0fff,
            display_mode: (display_bits >> 12) as u8,
        })
    }
}
