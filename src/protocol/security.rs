use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecurityPacketStats {
    pub(crate) s2c_slow_total: u32,
    pub(crate) s2c_fast_total: u32,
    pub(crate) s2c_slow_current: u16,
    pub(crate) s2c_fast_current: u16,
    pub(crate) s2c_reliable_out: u16,
    pub(crate) ping_current: u16,
    pub(crate) ping_average: u16,
    pub(crate) ping_low: u16,
    pub(crate) ping_high: u16,
    pub(crate) slow_frame: u8,
    pub(crate) timer_drift: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecurityPacket {
    pub(crate) weapons_received: u32,
    pub(crate) settings_checksum: u32,
    pub(crate) exe_checksum: u32,
    pub(crate) map_checksum: u32,
    pub(crate) stats: SecurityPacketStats,
    pub(crate) continuum_map_crc32: Option<u32>,
}

impl SecurityPacket {
    pub(crate) const TYPE: u8 = 0x1a;

    #[allow(dead_code)]
    pub(crate) fn vie(
        weapons_received: u32,
        settings_checksum: u32,
        exe_checksum: u32,
        map_checksum: u32,
        stats: SecurityPacketStats,
    ) -> Self {
        Self {
            weapons_received,
            settings_checksum,
            exe_checksum,
            map_checksum,
            stats,
            continuum_map_crc32: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn continuum(
        weapons_received: u32,
        settings_checksum: u32,
        exe_checksum: u32,
        map_checksum: u32,
        stats: SecurityPacketStats,
        continuum_map_crc32: u32,
    ) -> Self {
        Self {
            weapons_received,
            settings_checksum,
            exe_checksum,
            map_checksum,
            stats,
            continuum_map_crc32: Some(continuum_map_crc32),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(if self.continuum_map_crc32.is_some() {
            46
        } else {
            40
        });

        push_u8(&mut bytes, Self::TYPE);
        push_u32(&mut bytes, self.weapons_received);
        push_u32(&mut bytes, self.settings_checksum);
        push_u32(&mut bytes, self.exe_checksum);
        push_u32(&mut bytes, self.map_checksum);
        push_u32(&mut bytes, self.stats.s2c_slow_total);
        push_u32(&mut bytes, self.stats.s2c_fast_total);
        push_u16(&mut bytes, self.stats.s2c_slow_current);
        push_u16(&mut bytes, self.stats.s2c_fast_current);
        push_u16(&mut bytes, self.stats.s2c_reliable_out);
        push_u16(&mut bytes, self.stats.ping_current);
        push_u16(&mut bytes, self.stats.ping_average);
        push_u16(&mut bytes, self.stats.ping_low);
        push_u16(&mut bytes, self.stats.ping_high);
        push_u8(&mut bytes, self.stats.slow_frame);

        if let Some(map_crc32) = self.continuum_map_crc32 {
            push_u16(&mut bytes, self.stats.timer_drift);
            push_u32(&mut bytes, map_crc32);
        }

        bytes
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecurityChallenge {
    pub(crate) prize_seed: u32,
    pub(crate) door_seed: u32,
    pub(crate) timestamp: u32,
    pub(crate) checksum_key: u32,
}

impl SecurityChallenge {
    pub(crate) const TYPE: u8 = 0x18;
    pub(crate) const PACKET_LEN: usize = 17;

    #[allow(dead_code)]
    pub(crate) fn parse_packet(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::PACKET_LEN {
            return Err(format!(
                "security challenge packet too short: {} < {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected security challenge packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            prize_seed: read_u32(packet, 1)?,
            door_seed: read_u32(packet, 5)?,
            timestamp: read_u32(packet, 9)?,
            checksum_key: read_u32(packet, 13)?,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn should_send_security(self, map_checksum: u32) -> bool {
        self.checksum_key != 0 && map_checksum != 0
    }
}
