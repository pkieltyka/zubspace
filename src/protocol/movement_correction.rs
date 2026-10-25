// This shared packet module is compiled into both binaries: the server only
// encodes it and the browser client only parses it.
#![allow(dead_code)]

/// Recipient-private authoritative movement-correction proposal.
///
/// Detection does not immediately replace the server's accepted movement
/// lineage. The client pauses realtime gameplay and sends
/// [`MovementCorrectionApplied`] for its current tail without mutating local
/// movement. The server rebases that exact tail atomically and returns its
/// final raw-f32 state in [`MovementCorrectionAccepted`] before the client
/// resumes. This application barrier makes an arbitrarily delayed reliable
/// proposal safe without requiring rollback of weapons or other semantic side
/// effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementCorrection {
    pub(crate) reason: u8,
    /// Present only for the v3 Repel-window correction. The receipt barrier
    /// remains independent; this is movement metadata, not a Repel ACK.
    pub(crate) repel_window: Option<(u16, u16, u32)>,
    pub(crate) server_session_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) correction_generation: u32,
    pub(crate) position_sequence: u16,
    pub(crate) server_tick: u32,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) orientation: f32,
}

impl MovementCorrection {
    pub(crate) const TYPE: u8 = 0x7b;
    pub(crate) const VERSION: u8 = 2;
    pub(crate) const REASON_SUSTAINED_DRIFT: u8 = 2;
    /// An exact locally-predicted bomb recoil whose weapon admission lost an
    /// energy race with newer authoritative damage. The weapon stays
    /// rejected; this transaction only rebases movement to the no-recoil
    /// canonical lineage.
    pub(crate) const REASON_REJECTED_WEAPON_RECOIL: u8 = 3;
    /// The server's accepted position anchor fell behind the bounded rolling
    /// input history. Authority cannot reconstruct that suffix, so it pauses
    /// the browser and rebases the exact application tail to the last proven
    /// canonical state instead of accepting unverifiable movement or leaving
    /// the runtime permanently wedged.
    pub(crate) const REASON_INPUT_HISTORY_GAP: u8 = 4;
    /// A newly installed hostile brick overlaps the authoritative hull.
    /// Rebind the paused input tail to a server-selected clear spawn without
    /// changing life, energy or inventory.
    pub(crate) const REASON_BRICK_OVERLAP: u8 = 5;
    /// A delayed Repel receipt left the browser predicting a different
    /// server-offered suffix. Reconcile to the sealed canonical window.
    pub(crate) const REASON_REPEL_WINDOW: u8 = 6;
    pub(crate) const PACKET_LEN: usize = 47;
    pub(crate) const REPEL_WINDOW_VERSION: u8 = 3;
    pub(crate) const REPEL_WINDOW_PACKET_LEN: usize = 55;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([
            Self::TYPE,
            if self.repel_window.is_some() {
                Self::REPEL_WINDOW_VERSION
            } else {
                Self::VERSION
            },
            self.reason,
        ]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.correction_generation.to_le_bytes());
        packet.extend(self.position_sequence.to_le_bytes());
        packet.extend(self.server_tick.to_le_bytes());
        for value in [
            self.position_x,
            self.position_y,
            self.velocity_x,
            self.velocity_y,
            self.orientation,
        ] {
            packet.extend(value.to_bits().to_le_bytes());
        }
        if let Some((start, end, generation)) = self.repel_window {
            packet.extend(start.to_le_bytes());
            packet.extend(end.to_le_bytes());
            packet.extend(generation.to_le_bytes());
        }
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        let extended = packet.len() == Self::REPEL_WINDOW_PACKET_LEN
            && packet.get(1).copied() == Some(Self::REPEL_WINDOW_VERSION);
        let ordinary =
            packet.len() == Self::PACKET_LEN && packet.get(1).copied() == Some(Self::VERSION);
        if (!ordinary && !extended) || packet.first().copied() != Some(Self::TYPE) {
            return Err("invalid movement correction packet header or length".to_string());
        }
        let correction = Self {
            reason: packet[2],
            repel_window: if extended {
                Some((
                    read_u16(packet, 47)?,
                    read_u16(packet, 49)?,
                    read_u32(packet, 51)?,
                ))
            } else {
                None
            },
            server_session_id: read_u64(packet, 3)?,
            player_id: read_u16(packet, 11)?,
            life_seq: read_u32(packet, 13)?,
            correction_generation: read_u32(packet, 17)?,
            position_sequence: read_u16(packet, 21)?,
            server_tick: read_u32(packet, 23)?,
            position_x: read_f32(packet, 27)?,
            position_y: read_f32(packet, 31)?,
            velocity_x: read_f32(packet, 35)?,
            velocity_y: read_f32(packet, 39)?,
            orientation: read_f32(packet, 43)?,
        };
        if (correction.reason == Self::REASON_REPEL_WINDOW) != extended
            || correction
                .repel_window
                .is_some_and(|(start, end, generation)| {
                    let span = end.wrapping_sub(start);
                    span <= 1 || span >= (1 << 15) || generation == 0
                })
            || !matches!(
                correction.reason,
                Self::REASON_SUSTAINED_DRIFT
                    | Self::REASON_REJECTED_WEAPON_RECOIL
                    | Self::REASON_INPUT_HISTORY_GAP
                    | Self::REASON_BRICK_OVERLAP
                    | Self::REASON_REPEL_WINDOW
            )
            || correction.server_session_id == 0
            || correction.player_id == u16::MAX
            || correction.life_seq == 0
            || correction.correction_generation == 0
            || ![
                correction.position_x,
                correction.position_y,
                correction.velocity_x,
                correction.velocity_y,
                correction.orientation,
            ]
            .into_iter()
            .all(f32::is_finite)
            || !(0.0..=u16::MAX as f32).contains(&correction.position_x)
            || !(0.0..=u16::MAX as f32).contains(&correction.position_y)
            || !(0.0..=1.0).contains(&correction.orientation)
        {
            return Err("invalid movement correction identity or state".to_string());
        }
        Ok(correction)
    }
}

/// Client application marker for one exact correction proposal.
///
/// `application_sequence` is the client's last already-emitted old-lineage
/// tail when it pauses for the correction. `intent` requests either exact
/// replay/application or cancellation because a newer legal movement boundary
/// has already changed local state but is not yet server-visible. No input at
/// or after the following sequence may be emitted until the exact terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovementCorrectionApplied {
    /// Apply the proposed rebase, or cancel it because the browser has
    /// already crossed a newer same-life movement boundary that has not yet
    /// reached server authority.
    pub(crate) intent: u8,
    pub(crate) server_session_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) correction_generation: u32,
    pub(crate) detected_sequence: u16,
    pub(crate) application_sequence: u16,
}

impl MovementCorrectionApplied {
    pub(crate) const TYPE: u8 = 0x63;
    pub(crate) const VERSION: u8 = 2;
    pub(crate) const INTENT_APPLY: u8 = 1;
    pub(crate) const INTENT_CANCEL_NEWER_BOUNDARY: u8 = 2;
    pub(crate) const PACKET_LEN: usize = 25;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION, self.intent]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.correction_generation.to_le_bytes());
        packet.extend(self.detected_sequence.to_le_bytes());
        packet.extend(self.application_sequence.to_le_bytes());
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet.first().copied() != Some(Self::TYPE)
            || packet.get(1).copied() != Some(Self::VERSION)
        {
            return Err("invalid movement correction application packet".to_string());
        }
        let application = Self {
            intent: packet[2],
            server_session_id: read_u64(packet, 3)?,
            player_id: read_u16(packet, 11)?,
            life_seq: read_u32(packet, 13)?,
            correction_generation: read_u32(packet, 17)?,
            detected_sequence: read_u16(packet, 21)?,
            application_sequence: read_u16(packet, 23)?,
        };
        if !matches!(
            application.intent,
            Self::INTENT_APPLY | Self::INTENT_CANCEL_NEWER_BOUNDARY
        ) || application.server_session_id == 0
            || application.player_id == u16::MAX
            || application.life_seq == 0
            || application.correction_generation == 0
        {
            return Err("invalid movement correction application identity".to_string());
        }
        Ok(application)
    }
}

/// Server terminal outcome for one exact correction proposal.
///
/// Applied outcomes carry the exact rebase tail committed by authority.
/// Cancelled outcomes retire a proposal invalidated by a later same-life legal
/// discontinuity. The client remembers either outcome so an older in-flight
/// proposal can never reopen its gameplay barrier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementCorrectionAccepted {
    /// Server transaction tick that anchors the corrected movement and doors.
    pub(crate) server_tick: u32,
    pub(crate) outcome: u8,
    pub(crate) server_session_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) correction_generation: u32,
    pub(crate) detected_sequence: u16,
    pub(crate) application_sequence: u16,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) orientation: f32,
}

impl MovementCorrectionAccepted {
    pub(crate) const TYPE: u8 = 0x7c;
    pub(crate) const VERSION: u8 = 3;
    pub(crate) const OUTCOME_APPLIED: u8 = 1;
    pub(crate) const OUTCOME_CANCELLED: u8 = 2;
    pub(crate) const PACKET_LEN: usize = 49;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION, self.outcome]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.correction_generation.to_le_bytes());
        packet.extend(self.detected_sequence.to_le_bytes());
        packet.extend(self.application_sequence.to_le_bytes());
        for value in [
            self.position_x,
            self.position_y,
            self.velocity_x,
            self.velocity_y,
            self.orientation,
        ] {
            packet.extend(value.to_bits().to_le_bytes());
        }
        packet.extend(self.server_tick.to_le_bytes());
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet.first().copied() != Some(Self::TYPE)
            || packet.get(1).copied() != Some(Self::VERSION)
        {
            return Err("invalid movement correction terminal packet".to_string());
        }
        let receipt = Self {
            server_tick: read_u32(packet, 45)?,
            outcome: packet[2],
            server_session_id: read_u64(packet, 3)?,
            player_id: read_u16(packet, 11)?,
            life_seq: read_u32(packet, 13)?,
            correction_generation: read_u32(packet, 17)?,
            detected_sequence: read_u16(packet, 21)?,
            application_sequence: read_u16(packet, 23)?,
            position_x: read_f32(packet, 25)?,
            position_y: read_f32(packet, 29)?,
            velocity_x: read_f32(packet, 33)?,
            velocity_y: read_f32(packet, 37)?,
            orientation: read_f32(packet, 41)?,
        };
        if !matches!(
            receipt.outcome,
            Self::OUTCOME_APPLIED | Self::OUTCOME_CANCELLED
        ) || receipt.server_tick & 0x8000_0000 != 0
            || receipt.server_session_id == 0
            || receipt.player_id == u16::MAX
            || receipt.life_seq == 0
            || receipt.correction_generation == 0
            || ![
                receipt.position_x,
                receipt.position_y,
                receipt.velocity_x,
                receipt.velocity_y,
                receipt.orientation,
            ]
            .into_iter()
            .all(f32::is_finite)
            || !(0.0..=u16::MAX as f32).contains(&receipt.position_x)
            || !(0.0..=u16::MAX as f32).contains(&receipt.position_y)
            || !(0.0..=1.0).contains(&receipt.orientation)
        {
            return Err(
                "invalid movement correction terminal identity, outcome, or state".to_string(),
            );
        }
        Ok(receipt)
    }
}

fn read_u16(packet: &[u8], offset: usize) -> Result<u16, String> {
    packet
        .get(offset..offset + 2)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| "truncated movement correction u16".to_string())
}

fn read_u32(packet: &[u8], offset: usize) -> Result<u32, String> {
    packet
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "truncated movement correction u32".to_string())
}

fn read_u64(packet: &[u8], offset: usize) -> Result<u64, String> {
    packet
        .get(offset..offset + 8)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| "truncated movement correction u64".to_string())
}

fn read_f32(packet: &[u8], offset: usize) -> Result<f32, String> {
    read_u32(packet, offset).map(f32::from_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correction() -> MovementCorrection {
        MovementCorrection {
            reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
            repel_window: None,
            server_session_id: 0x0102_0304_0506_0708,
            player_id: 17,
            life_seq: u32::MAX,
            correction_generation: u32::MAX,
            position_sequence: u16::MAX,
            server_tick: 0x7fff_fffe,
            position_x: 12_345.375,
            position_y: 54_321.75,
            velocity_x: -321.125,
            velocity_y: 45.5,
            orientation: 1.0,
        }
    }

    #[test]
    fn exact_identity_and_raw_f32_state_round_trip() {
        let expected = correction();
        let packet = expected.encode();
        assert_eq!(packet.len(), MovementCorrection::PACKET_LEN);
        assert_eq!(MovementCorrection::parse(&packet), Ok(expected));

        let rejected_recoil = MovementCorrection {
            reason: MovementCorrection::REASON_REJECTED_WEAPON_RECOIL,
            repel_window: None,
            ..expected
        };
        assert_eq!(
            MovementCorrection::parse(&rejected_recoil.encode()),
            Ok(rejected_recoil),
            "the bounded rejected-recoil correction uses the same channel contract"
        );

        let input_history_gap = MovementCorrection {
            reason: MovementCorrection::REASON_INPUT_HISTORY_GAP,
            repel_window: None,
            ..expected
        };
        assert_eq!(
            MovementCorrection::parse(&input_history_gap.encode()),
            Ok(input_history_gap),
            "bounded input-history recovery uses the same scoped correction contract"
        );
    }

    #[test]
    fn repel_window_correction_requires_its_scoped_versioned_movement_metadata() {
        let expected = MovementCorrection {
            reason: MovementCorrection::REASON_REPEL_WINDOW,
            repel_window: Some((u16::MAX - 3, 9, 7)),
            ..correction()
        };
        let packet = expected.encode();
        assert_eq!(packet.len(), MovementCorrection::REPEL_WINDOW_PACKET_LEN);
        assert_eq!(packet[1], MovementCorrection::REPEL_WINDOW_VERSION);
        assert_eq!(MovementCorrection::parse(&packet), Ok(expected));
        // Every older reason retains its byte-for-byte47-byte v2 framing.
        assert_eq!(correction().encode().len(), MovementCorrection::PACKET_LEN);
        assert_eq!(correction().encode()[1], MovementCorrection::VERSION);
        for invalid in [
            MovementCorrection {
                repel_window: None,
                ..expected
            },
            MovementCorrection {
                repel_window: Some((5, 5, 7)),
                ..expected
            },
            MovementCorrection {
                repel_window: Some((5, 6, 7)),
                ..expected
            },
            MovementCorrection {
                repel_window: Some((5, 0x8005, 7)),
                ..expected
            },
            MovementCorrection {
                repel_window: Some((5, 9, 0)),
                ..expected
            },
            MovementCorrection {
                reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
                ..expected
            },
        ] {
            assert!(MovementCorrection::parse(&invalid.encode()).is_err());
        }
        for length in 0..packet.len() {
            assert!(MovementCorrection::parse(&packet[..length]).is_err());
        }
    }

    #[test]
    fn malformed_stale_or_nonfinite_correction_fails_closed() {
        let packet = correction().encode();
        for mutate in [
            |packet: &mut Vec<u8>| packet[1] = 0,
            |packet: &mut Vec<u8>| packet[2] = 0,
            |packet: &mut Vec<u8>| packet[3..11].copy_from_slice(&0_u64.to_le_bytes()),
            |packet: &mut Vec<u8>| packet[13..17].copy_from_slice(&0_u32.to_le_bytes()),
            |packet: &mut Vec<u8>| packet[17..21].copy_from_slice(&0_u32.to_le_bytes()),
            |packet: &mut Vec<u8>| {
                packet[27..31].copy_from_slice(&f32::NAN.to_bits().to_le_bytes())
            },
        ] {
            let mut malformed = packet.clone();
            mutate(&mut malformed);
            assert!(MovementCorrection::parse(&malformed).is_err());
        }
        let mut trailing = packet;
        trailing.push(0);
        assert!(MovementCorrection::parse(&trailing).is_err());
    }

    #[test]
    fn application_and_acceptance_bind_exact_wrap_safe_identity() {
        let application = MovementCorrectionApplied {
            intent: MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: 77,
            player_id: 9,
            life_seq: u32::MAX,
            correction_generation: u32::MAX,
            detected_sequence: u16::MAX,
            application_sequence: 0,
        };
        assert_eq!(
            MovementCorrectionApplied::parse(&application.encode()),
            Ok(application)
        );
        let cancelled = MovementCorrectionApplied {
            intent: MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY,
            ..application
        };
        assert_eq!(
            MovementCorrectionApplied::parse(&cancelled.encode()),
            Ok(cancelled)
        );
        let mut malformed_intent = application.encode();
        malformed_intent[2] = 0;
        assert!(MovementCorrectionApplied::parse(&malformed_intent).is_err());
        let accepted = MovementCorrectionAccepted {
            server_tick: 0x7fff_ffff,
            outcome: MovementCorrectionAccepted::OUTCOME_APPLIED,
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            correction_generation: application.correction_generation,
            detected_sequence: application.detected_sequence,
            application_sequence: application.application_sequence,
            position_x: 12_345.375,
            position_y: 54_321.75,
            velocity_x: -321.125,
            velocity_y: 45.5,
            orientation: 1.0,
        };
        assert_eq!(
            MovementCorrectionAccepted::parse(&accepted.encode()),
            Ok(accepted)
        );

        let cancelled = MovementCorrectionAccepted {
            outcome: MovementCorrectionAccepted::OUTCOME_CANCELLED,
            application_sequence: application.detected_sequence,
            ..accepted
        };
        assert_eq!(
            MovementCorrectionAccepted::parse(&cancelled.encode()),
            Ok(cancelled)
        );
        let mut invalid = cancelled.encode();
        invalid[2] = 0;
        assert!(MovementCorrectionAccepted::parse(&invalid).is_err());
        let mut nonfinite = accepted.encode();
        nonfinite[25..29].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(MovementCorrectionAccepted::parse(&nonfinite).is_err());
        let mut old_version = accepted.encode();
        old_version[1] = 2;
        assert!(MovementCorrectionAccepted::parse(&old_version).is_err());
        assert!(MovementCorrectionAccepted::parse(&accepted.encode()[..45]).is_err());
        let mut invalid_tick = accepted.encode();
        invalid_tick[45..49].copy_from_slice(&0x8000_0000_u32.to_le_bytes());
        assert!(MovementCorrectionAccepted::parse(&invalid_tick).is_err());
    }
}
