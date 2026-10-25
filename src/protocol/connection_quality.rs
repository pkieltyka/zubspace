//! Independent unreliable challenge/echo probes. These carry no gameplay state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionQualityProbe {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) sequence: u32,
    pub(crate) nonce: u64,
}

impl ConnectionQualityProbe {
    pub(crate) const TYPE: u8 = 0x81;
    pub(crate) const ECHO_TYPE: u8 = 0x82;
    pub(crate) const LEN: usize = 30;

    pub(crate) fn encode(self) -> Vec<u8> {
        self.encode_type(Self::TYPE)
    }
    pub(crate) fn encode_echo(self) -> Vec<u8> {
        self.encode_type(Self::ECHO_TYPE)
    }
    fn encode_type(self, kind: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::LEN);
        bytes.extend([kind, 1]);
        bytes.extend(self.server_session_id.to_le_bytes());
        bytes.extend(self.arena_stream_id.to_le_bytes());
        bytes.extend(self.sequence.to_le_bytes());
        bytes.extend(self.nonce.to_le_bytes());
        bytes
    }
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        Self::parse_type(bytes, Self::TYPE)
    }
    pub(crate) fn parse_echo(bytes: &[u8]) -> Result<Self, String> {
        Self::parse_type(bytes, Self::ECHO_TYPE)
    }
    fn parse_type(bytes: &[u8], kind: u8) -> Result<Self, String> {
        if bytes.len() != Self::LEN || bytes[0] != kind || bytes[1] != 1 {
            return Err("invalid connection-quality probe framing".into());
        }
        let probe = Self {
            server_session_id: u64::from_le_bytes(bytes[2..10].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(bytes[10..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(bytes[18..22].try_into().unwrap()),
            nonce: u64::from_le_bytes(bytes[22..30].try_into().unwrap()),
        };
        if probe.server_session_id == 0 || probe.arena_stream_id == 0 || probe.nonce == 0 {
            return Err("invalid connection-quality probe scope or nonce".into());
        }
        Ok(probe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quality_probe_codec_preserves_scoped_challenge_and_rejects_malformed_echoes() {
        let probe = ConnectionQualityProbe {
            server_session_id: 23,
            arena_stream_id: 42,
            sequence: u32::MAX,
            nonce: 92,
        };
        assert_eq!(
            ConnectionQualityProbe::parse(&probe.encode()).unwrap(),
            probe
        );
        assert_eq!(
            ConnectionQualityProbe::parse_echo(&probe.encode_echo()).unwrap(),
            probe
        );
        assert!(ConnectionQualityProbe::parse_echo(&probe.encode()).is_err());
        for size in 0..ConnectionQualityProbe::LEN {
            assert!(ConnectionQualityProbe::parse(&probe.encode()[..size]).is_err());
        }
        for offset in [0, 1, 2, 10, 22, 30] {
            let mut bytes = probe.encode();
            match offset {
                0 | 1 => bytes[offset] = 0,
                30 => bytes.push(0),
                _ => bytes[offset..offset + 8].fill(0),
            }
            assert!(ConnectionQualityProbe::parse(&bytes).is_err());
        }
    }
}

/// Private, ordered server fact. A forced transition advances one authority
/// life; Hold/Clear describe that same spectator life. This is not a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionQualityDirectiveKind {
    Force,
    Hold,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionQualityDirective {
    pub(crate) kind: ConnectionQualityDirectiveKind,
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) predecessor_life_seq: u32,
    pub(crate) life_seq: u32,
    pub(crate) hold_id: u64,
    pub(crate) remaining_ms: u32,
    /// Exact denied user ship request, if this Hold terminates one.
    pub(crate) rejected_request: Option<(u8, u16)>,
}

impl ConnectionQualityDirective {
    pub(crate) const TYPE: u8 = 0x83;
    pub(crate) const LEN: usize = 44;
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.server_session_id == 0
            || self.arena_stream_id == 0
            || self.predecessor_life_seq == 0
            || self.life_seq == 0
            || self.hold_id == 0
            || self.remaining_ms > 120_000
        {
            return Err("invalid connection-quality directive scope or hold".into());
        }
        match self.kind {
            ConnectionQualityDirectiveKind::Force
                if self.life_seq != self.predecessor_life_seq.wrapping_add(1).max(1)
                    || self.rejected_request.is_some() =>
            {
                return Err("invalid forced spectator life transition".into());
            }
            ConnectionQualityDirectiveKind::Hold | ConnectionQualityDirectiveKind::Clear
                if self.life_seq != self.predecessor_life_seq =>
            {
                return Err("quality status cannot advance an authority life".into());
            }
            ConnectionQualityDirectiveKind::Clear
                if self.remaining_ms != 0 || self.rejected_request.is_some() =>
            {
                return Err("invalid cleared quality hold".into());
            }
            _ => {}
        }
        if self.rejected_request.is_some_and(|(ship, _)| ship >= 8) {
            return Err("quality hold can reject only a request to play".into());
        }
        Ok(())
    }
    pub(crate) fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::LEN);
        bytes.extend([
            Self::TYPE,
            1,
            match self.kind {
                ConnectionQualityDirectiveKind::Force => 0,
                ConnectionQualityDirectiveKind::Hold => 1,
                ConnectionQualityDirectiveKind::Clear => 2,
            },
        ]);
        bytes.extend(self.server_session_id.to_le_bytes());
        bytes.extend(self.arena_stream_id.to_le_bytes());
        bytes.extend(self.player_id.to_le_bytes());
        bytes.extend(self.predecessor_life_seq.to_le_bytes());
        bytes.extend(self.life_seq.to_le_bytes());
        bytes.extend(self.hold_id.to_le_bytes());
        bytes.extend(self.remaining_ms.to_le_bytes());
        bytes.push(self.rejected_request.map_or(255, |(ship, _)| ship));
        bytes.extend(
            self.rejected_request
                .map_or(0, |(_, seq)| seq)
                .to_le_bytes(),
        );
        bytes
    }
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != Self::LEN || bytes[0] != Self::TYPE || bytes[1] != 1 {
            return Err("invalid connection-quality directive framing".into());
        }
        let kind = match bytes[2] {
            0 => ConnectionQualityDirectiveKind::Force,
            1 => ConnectionQualityDirectiveKind::Hold,
            2 => ConnectionQualityDirectiveKind::Clear,
            _ => return Err("unknown connection-quality directive".into()),
        };
        let sequence = u16::from_le_bytes(bytes[42..44].try_into().unwrap());
        let rejected_request = match bytes[41] {
            255 if sequence == 0 => None,
            0..=7 => Some((bytes[41], sequence)),
            _ => return Err("noncanonical denied ship request".into()),
        };
        let fact = Self {
            kind,
            server_session_id: u64::from_le_bytes(bytes[3..11].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(bytes[11..19].try_into().unwrap()),
            player_id: u16::from_le_bytes(bytes[19..21].try_into().unwrap()),
            predecessor_life_seq: u32::from_le_bytes(bytes[21..25].try_into().unwrap()),
            life_seq: u32::from_le_bytes(bytes[25..29].try_into().unwrap()),
            hold_id: u64::from_le_bytes(bytes[29..37].try_into().unwrap()),
            remaining_ms: u32::from_le_bytes(bytes[37..41].try_into().unwrap()),
            rejected_request,
        };
        fact.validate()?;
        Ok(fact)
    }
}

#[cfg(test)]
mod directive_tests {
    use super::*;
    #[test]
    fn quality_directives_round_trip_and_fail_closed_on_noncanonical_authority() {
        let force = ConnectionQualityDirective {
            kind: ConnectionQualityDirectiveKind::Force,
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 7,
            predecessor_life_seq: u32::MAX,
            life_seq: 1,
            hold_id: 5,
            remaining_ms: 120_000,
            rejected_request: None,
        };
        for fact in [
            force,
            ConnectionQualityDirective {
                kind: ConnectionQualityDirectiveKind::Hold,
                predecessor_life_seq: 1,
                rejected_request: Some((7, 0)),
                ..force
            },
            ConnectionQualityDirective {
                kind: ConnectionQualityDirectiveKind::Clear,
                predecessor_life_seq: 1,
                remaining_ms: 0,
                ..force
            },
        ] {
            assert_eq!(
                ConnectionQualityDirective::parse(&fact.encode()).unwrap(),
                fact
            );
        }
        for size in 0..ConnectionQualityDirective::LEN {
            assert!(ConnectionQualityDirective::parse(&force.encode()[..size]).is_err());
        }
        for offset in [0, 1, 2, 3, 11, 21, 25, 29, 37, 41, 42, 44] {
            let mut bytes = force.encode();
            match offset {
                0..=2 => bytes[offset] = 255,
                3 | 11 | 29 => bytes[offset..offset + 8].fill(0),
                21 | 25 => bytes[offset..offset + 4].fill(0),
                37 => bytes[offset..offset + 4].fill(255),
                41 => bytes[offset] = 8,
                42 => bytes[offset] = 1,
                _ => bytes.push(0),
            }
            assert!(
                ConnectionQualityDirective::parse(&bytes).is_err(),
                "accepted mutation at {offset}"
            );
        }
        assert!(
            ConnectionQualityDirective {
                remaining_ms: 120_001,
                ..force
            }
            .validate()
            .is_err()
        );
        assert!(
            ConnectionQualityDirective {
                life_seq: 2,
                ..force
            }
            .validate()
            .is_err()
        );
        assert!(
            ConnectionQualityDirective {
                kind: ConnectionQualityDirectiveKind::Hold,
                ..force
            }
            .validate()
            .is_err()
        );
    }
}
