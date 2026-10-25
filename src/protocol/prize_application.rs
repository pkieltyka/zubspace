// This shared packet module is compiled into both binaries. Engine Shutdown
// and movement-upgrade prizes change movement semantics, so a reliable prize
// delivery needs an exact gameplay-sequence application boundary rather than
// a wall-clock guess.
#![allow(dead_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrizeEffectApplied {
    pub(crate) server_session_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) collectible_id: u32,
    pub(crate) prize_id: i16,
    pub(crate) prize_state_revision: u32,
    /// First input sequence generated after the browser applied the effect.
    pub(crate) application_sequence: u16,
}

impl PrizeEffectApplied {
    pub(crate) const TYPE: u8 = 0x64;
    pub(crate) const VERSION: u8 = 1;
    pub(crate) const PACKET_LEN: usize = 28;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.collectible_id.to_le_bytes());
        packet.extend(self.prize_id.to_le_bytes());
        packet.extend(self.prize_state_revision.to_le_bytes());
        packet.extend(self.application_sequence.to_le_bytes());
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet.first().copied() != Some(Self::TYPE)
            || packet.get(1).copied() != Some(Self::VERSION)
        {
            return Err("invalid prize effect application header or length".to_string());
        }
        let application = Self {
            server_session_id: u64::from_le_bytes(packet[2..10].try_into().unwrap()),
            player_id: u16::from_le_bytes(packet[10..12].try_into().unwrap()),
            life_seq: u32::from_le_bytes(packet[12..16].try_into().unwrap()),
            collectible_id: u32::from_le_bytes(packet[16..20].try_into().unwrap()),
            prize_id: i16::from_le_bytes(packet[20..22].try_into().unwrap()),
            prize_state_revision: u32::from_le_bytes(packet[22..26].try_into().unwrap()),
            application_sequence: u16::from_le_bytes(packet[26..28].try_into().unwrap()),
        };
        if application.server_session_id == 0
            || application.player_id == u16::MAX
            || application.life_seq == 0
            || application.collectible_id == 0
            || application.prize_state_revision == 0
            || !prize_effect_requires_application_boundary(application.prize_id)
        {
            return Err("invalid prize effect application identity".to_string());
        }
        Ok(application)
    }
}

pub(crate) fn prize_effect_requires_application_boundary(prize_id: i16) -> bool {
    matches!(prize_id.checked_abs(), Some(1 | 3 | 11 | 12 | 14)) || prize_id == 25
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrizeEffectAccepted {
    pub(crate) server_session_id: u64,
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
    pub(crate) collectible_id: u32,
    pub(crate) prize_id: i16,
    pub(crate) prize_state_revision: u32,
    /// Exact client marker this terminal answers. A REBASE creates a new
    /// marker at `application_sequence`; it must never make this terminal a
    /// tombstone for that new marker.
    pub(crate) requested_application_sequence: u16,
    /// Applied boundary, or the next boundary requested by REBASE.
    pub(crate) application_sequence: u16,
    pub(crate) outcome: u8,
}

impl PrizeEffectAccepted {
    pub(crate) const TYPE: u8 = 0x76;
    pub(crate) const VERSION: u8 = 1;
    pub(crate) const PACKET_LEN: usize = PrizeEffectApplied::PACKET_LEN + 3;
    pub(crate) const OUTCOME_APPLIED: u8 = 1;
    pub(crate) const OUTCOME_REBASE: u8 = 2;
    pub(crate) const OUTCOME_CANCELLED: u8 = 3;

    pub(crate) fn encode(self) -> Vec<u8> {
        let application = PrizeEffectApplied {
            server_session_id: self.server_session_id,
            player_id: self.player_id,
            life_seq: self.life_seq,
            collectible_id: self.collectible_id,
            prize_id: self.prize_id,
            prize_state_revision: self.prize_state_revision,
            application_sequence: self.requested_application_sequence,
        };
        let mut packet = application.encode();
        packet[0] = Self::TYPE;
        packet.extend(self.application_sequence.to_le_bytes());
        packet.push(self.outcome);
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.first().copied() != Some(Self::TYPE) {
            return Err("invalid prize effect acceptance type".to_string());
        }
        if packet.len() != Self::PACKET_LEN
            || !matches!(
                packet[30],
                Self::OUTCOME_APPLIED | Self::OUTCOME_REBASE | Self::OUTCOME_CANCELLED
            )
        {
            return Err("invalid prize effect acceptance outcome or length".to_string());
        }
        let mut application_packet = packet[..PrizeEffectApplied::PACKET_LEN].to_vec();
        application_packet[0] = PrizeEffectApplied::TYPE;
        let application = PrizeEffectApplied::parse(&application_packet)?;
        Ok(Self {
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            collectible_id: application.collectible_id,
            prize_id: application.prize_id,
            prize_state_revision: application.prize_state_revision,
            requested_application_sequence: application.application_sequence,
            application_sequence: u16::from_le_bytes(packet[28..30].try_into().unwrap()),
            outcome: packet[30],
        })
    }
}

impl From<PrizeEffectApplied> for PrizeEffectAccepted {
    fn from(application: PrizeEffectApplied) -> Self {
        Self {
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            collectible_id: application.collectible_id,
            prize_id: application.prize_id,
            prize_state_revision: application.prize_state_revision,
            requested_application_sequence: application.application_sequence,
            application_sequence: application.application_sequence,
            outcome: Self::OUTCOME_APPLIED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_and_acceptance_round_trip_exact_identity_across_sequence_wrap() {
        let application = PrizeEffectApplied {
            server_session_id: 0x0102_0304_0506_0708,
            player_id: 42,
            life_seq: 9,
            collectible_id: 77,
            prize_id: -14,
            prize_state_revision: u32::MAX,
            application_sequence: 0,
        };
        assert_eq!(
            PrizeEffectApplied::parse(&application.encode()),
            Ok(application)
        );
        let accepted = PrizeEffectAccepted::from(application);
        for outcome in [
            PrizeEffectAccepted::OUTCOME_APPLIED,
            PrizeEffectAccepted::OUTCOME_REBASE,
            PrizeEffectAccepted::OUTCOME_CANCELLED,
        ] {
            let terminal = PrizeEffectAccepted {
                outcome,
                application_sequence: if outcome == PrizeEffectAccepted::OUTCOME_REBASE {
                    u16::MAX
                } else {
                    accepted.application_sequence
                },
                ..accepted
            };
            assert_eq!(PrizeEffectAccepted::parse(&terminal.encode()), Ok(terminal));
        }

        let mut malformed = application.encode();
        malformed[20..22].copy_from_slice(&13_i16.to_le_bytes());
        assert!(PrizeEffectApplied::parse(&malformed).is_err());
        assert!(PrizeEffectAccepted::parse(&accepted.encode()[..28]).is_err());
        let mut invalid_outcome = accepted.encode();
        invalid_outcome[30] = 0;
        assert!(PrizeEffectAccepted::parse(&invalid_outcome).is_err());
    }

    #[test]
    fn movement_prize_applications_share_the_exact_boundary_contract() {
        for prize_id in [1, -1, 3, -3, 11, -11, 12, -12, 14, -14, 25] {
            let application = PrizeEffectApplied {
                server_session_id: 1,
                player_id: 2,
                life_seq: 3,
                collectible_id: 4,
                prize_id,
                prize_state_revision: 5,
                application_sequence: 6,
            };
            assert_eq!(
                PrizeEffectApplied::parse(&application.encode()),
                Ok(application)
            );
        }
        assert!(!prize_effect_requires_application_boundary(-25));
    }
}
