//! A scoped, read-only observer view carried on the existing reliable spectate lane.
//! Camera coordinates never represent a player's authoritative position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpectatorViewRequest {
    pub(crate) target: u16,
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) life_seq: u32,
    pub(crate) sequence: u16,
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// Read-only presentation activation hint. Never a movement/input witness.
    pub(crate) next_input_sequence: Option<u16>,
}

#[allow(dead_code)]
impl SpectatorViewRequest {
    pub(crate) const LEGACY_LEN: usize = 34;
    pub(crate) const LEN: usize = 36;
    pub(crate) fn encode(self) -> Vec<u8> {
        let mut out = vec![0x08];
        out.extend(self.target.to_le_bytes());
        out.push(if self.next_input_sequence.is_some() {
            2
        } else {
            1
        });
        out.extend(self.server_session_id.to_le_bytes());
        out.extend(self.arena_stream_id.to_le_bytes());
        out.extend(self.life_seq.to_le_bytes());
        out.extend(self.sequence.to_le_bytes());
        out.extend(self.x.to_le_bytes());
        out.extend(self.y.to_le_bytes());
        if let Some(sequence) = self.next_input_sequence {
            out.extend(sequence.to_le_bytes());
        }
        out
    }
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        if !matches!(bytes.len(), Self::LEGACY_LEN | Self::LEN)
            || bytes[0] != 0x08
            || !matches!(
                (bytes[3], bytes.len()),
                (1, Self::LEGACY_LEN) | (2, Self::LEN)
            )
        {
            return None;
        }
        let view = Self {
            target: u16::from_le_bytes(bytes[1..3].try_into().ok()?),
            server_session_id: u64::from_le_bytes(bytes[4..12].try_into().ok()?),
            arena_stream_id: u64::from_le_bytes(bytes[12..20].try_into().ok()?),
            life_seq: u32::from_le_bytes(bytes[20..24].try_into().ok()?),
            sequence: u16::from_le_bytes(bytes[24..26].try_into().ok()?),
            x: f32::from_le_bytes(bytes[26..30].try_into().ok()?),
            y: f32::from_le_bytes(bytes[30..34].try_into().ok()?),
            next_input_sequence: (bytes[3] == 2)
                .then(|| u16::from_le_bytes(bytes[34..36].try_into().expect("versioned length"))),
        };
        (view.x.is_finite() && view.y.is_finite() && view.life_seq != 0).then_some(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observer_view_rejects_noncanonical_framing_and_nonfinite_coordinates() {
        let view = SpectatorViewRequest {
            target: u16::MAX,
            server_session_id: 1,
            arena_stream_id: 2,
            life_seq: 3,
            sequence: u16::MAX,
            x: 100.5,
            y: 200.25,
            next_input_sequence: None,
        };
        let bytes = view.encode();
        assert_eq!(SpectatorViewRequest::decode(&bytes), Some(view));
        let modern = SpectatorViewRequest {
            next_input_sequence: Some(u16::MAX),
            ..view
        };
        let modern_bytes = modern.encode();
        assert_eq!(modern_bytes.len(), SpectatorViewRequest::LEN);
        assert_eq!(SpectatorViewRequest::decode(&modern_bytes), Some(modern));
        for len in 0..modern_bytes.len() {
            assert!(SpectatorViewRequest::decode(&modern_bytes[..len]).is_none());
        }
        let mut wrong_version = modern_bytes.clone();
        wrong_version[3] = 1;
        assert!(SpectatorViewRequest::decode(&wrong_version).is_none());
        let mut wrong_legacy = bytes.clone();
        wrong_legacy[3] = 2;
        assert!(SpectatorViewRequest::decode(&wrong_legacy).is_none());
        for len in 0..bytes.len() {
            assert!(SpectatorViewRequest::decode(&bytes[..len]).is_none());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(SpectatorViewRequest::decode(&extra).is_none());
        for x in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                SpectatorViewRequest::decode(&SpectatorViewRequest { x, ..view }.encode())
                    .is_none()
            );
        }
        assert!(
            SpectatorViewRequest::decode(
                &SpectatorViewRequest {
                    life_seq: 0,
                    ..view
                }
                .encode()
            )
            .is_none()
        );
    }
}
