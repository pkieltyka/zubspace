use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleSendDamagePacket {
    pub(crate) enabled: bool,
}

impl ToggleSendDamagePacket {
    pub(crate) const TYPE: u8 = 0x37;
    pub(crate) const PACKET_LEN: usize = 2;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "toggle send damage packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected toggle send damage packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            enabled: packet[1] != 0,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerDamageReportEntry {
    pub(crate) attacker_id: u16,
    pub(crate) weapon_data: u16,
    pub(crate) energy: u16,
    pub(crate) damage: u16,
    pub(crate) timestamp_offset: u8,
    /// Present for zubspace snapshot damage events. Legacy WATCH_DAMAGE
    /// packets do not carry this field.
    pub(crate) energy_before: Option<u16>,
    /// Server-owned authoritative damage event sequence. Legacy
    /// WATCH_DAMAGE packets do not carry this field.
    pub(crate) damage_seq: Option<u32>,
    /// Server-owned authoritative projectile identity. Legacy WATCH_DAMAGE
    /// packets do not carry this field.
    pub(crate) projectile_id: Option<u32>,
    /// Server-owned victim life/loadout generation. Legacy WATCH_DAMAGE
    /// packets do not carry this field.
    pub(crate) victim_life_seq: Option<u32>,
    /// Server-owned attacker life/loadout generation. Legacy WATCH_DAMAGE
    /// packets do not carry this field.
    pub(crate) shooter_life_seq: Option<u32>,
    /// Explicit authoritative lethal classification carried by a zubspace
    /// snapshot. Legacy WATCH_DAMAGE packets do not carry this bit and may
    /// never be used as lifecycle evidence.
    pub(crate) explicit_lethal: Option<bool>,
    /// Applied authoritative snapshot which carried this damage event.
    /// Lifecycle repair accepts only a strictly-forward snapshot, so a stale
    /// or cross-arena event cannot advance a reusable roster identity.
    pub(crate) source_snapshot_id: Option<u16>,
    /// Monotonic client-session arena generation assigned when JOIN_GAME
    /// starts a new authoritative snapshot stream.
    pub(crate) source_snapshot_epoch: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerDamageReport {
    pub(crate) player_id: u16,
    pub(crate) timestamp: u32,
    pub(crate) entries: Vec<ServerDamageReportEntry>,
}

impl ServerDamageReport {
    pub(crate) const TYPE: u8 = 0x38;
    pub(crate) const HEADER_LEN: usize = 7;
    pub(crate) const ENTRY_LEN: usize = 9;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_LEN {
            return Err(format!(
                "watch damage packet too short: {} < {}",
                packet.len(),
                Self::HEADER_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected watch damage packet type: {:#04x}",
                packet[0]
            ));
        }

        let entry_bytes = packet.len() - Self::HEADER_LEN;
        if !entry_bytes.is_multiple_of(Self::ENTRY_LEN) {
            return Err(format!(
                "watch damage packet has partial entry bytes: {}",
                entry_bytes
            ));
        }

        let mut entries = Vec::with_capacity(entry_bytes / Self::ENTRY_LEN);
        for chunk in packet[Self::HEADER_LEN..].chunks_exact(Self::ENTRY_LEN) {
            entries.push(ServerDamageReportEntry {
                attacker_id: read_u16(chunk, 0)?,
                weapon_data: read_u16(chunk, 2)?,
                energy: read_u16(chunk, 4)?,
                damage: read_u16(chunk, 6)?,
                timestamp_offset: chunk[8],
                energy_before: None,
                damage_seq: None,
                projectile_id: None,
                victim_life_seq: None,
                shooter_life_seq: None,
                explicit_lethal: None,
                source_snapshot_id: None,
                source_snapshot_epoch: None,
            });
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
            timestamp: read_u32(packet, 3)?,
            entries,
        })
    }
}
