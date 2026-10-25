use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientPositionUpdate {
    pub(crate) direction: u8,
    pub(crate) timestamp: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) togglables: u8,
    pub(crate) bounty: u16,
    pub(crate) energy: u16,
    pub(crate) weapon: u16,
    pub(crate) extra: Option<ClientPositionExtra>,
    /// Mandatory current-protocol raw f32 tail. The legacy integer fields
    /// remain for relay compatibility, but authority is proven against these
    /// exact shared-simulation bits.
    pub(crate) canonical_state: crate::continuum_position::ClientPositionCanonicalState,
    /// Exact `(snapshot_id, input_sequence)` boundary for the newest hostile
    /// Repel applied by this client. The impulse occurs after movement for the
    /// named sequence and before any weapon-independent outbound publication.
    pub(crate) repel_application: Option<crate::continuum_position::ClientRepelApplicationMarker>,
    /// Redundant rolling input history delivered atomically with this
    /// position on the unordered realtime channel.
    pub(crate) input_history: Option<ClientInputRecordPacket>,
    /// Positive-Warp application identity repeated until the server's exact
    /// acceptance receipt arrives.
    pub(crate) warp_collectible_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClientPositionExtra {
    pub(crate) s2c_latency: u16,
    pub(crate) flag_timer: u16,
    pub(crate) items: u32,
}

impl ClientPositionUpdate {
    pub(crate) const TYPE: u8 = 0x03;
    pub(crate) const CHECKSUM_OFFSET: usize = 10;

    #[allow(dead_code)]
    pub(crate) fn encode(&self) -> Vec<u8> {
        let extra = Some(self.extra.unwrap_or_default());
        let input_history_len = self
            .input_history
            .as_ref()
            .map_or(0, |history| history.encode().len());
        let mut bytes = Vec::with_capacity(
            32 + crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_LEN + input_history_len,
        );
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.direction);
        push_u32(&mut bytes, self.timestamp);
        push_u16(&mut bytes, self.velocity_x as u16);
        push_u16(&mut bytes, self.y);
        push_u8(&mut bytes, 0);
        push_u8(&mut bytes, self.togglables);
        push_u16(&mut bytes, self.x);
        push_u16(&mut bytes, self.velocity_y as u16);
        push_u16(&mut bytes, self.bounty);
        push_u16(&mut bytes, self.energy);
        push_u16(&mut bytes, self.weapon);

        if let Some(extra) = extra {
            push_u16(&mut bytes, self.energy);
            push_u16(&mut bytes, extra.s2c_latency);
            push_u16(&mut bytes, extra.flag_timer);
            push_u32(&mut bytes, extra.items);
        }

        self.canonical_state.encode(&mut bytes);

        if let Some(input_history) = &self.input_history {
            bytes.extend_from_slice(&input_history.encode());
        }
        if let Some(repel) = self.repel_application {
            push_u8(
                &mut bytes,
                crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_TYPE,
            );
            push_u8(
                &mut bytes,
                crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_VERSION,
            );
            push_u16(&mut bytes, repel.snapshot_id);
            push_u16(&mut bytes, repel.application_sequence);
            push_u32(&mut bytes, repel.delivery_generation);
        }
        if let Some(collectible_id) = self.warp_collectible_id {
            push_u8(
                &mut bytes,
                crate::continuum_position::CLIENT_POSITION_WARP_MARKER_TYPE,
            );
            push_u32(&mut bytes, collectible_id);
        }

        bytes[Self::CHECKSUM_OFFSET] = weapon_checksum(&bytes);
        bytes
    }
}
