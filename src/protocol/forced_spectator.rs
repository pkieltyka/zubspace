/// Server-owned spawn-idle transition; independent of connection-quality holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpawnIdleSpectator {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) predecessor_life_seq: u32,
    pub(crate) life_seq: u32,
}

impl SpawnIdleSpectator {
    pub(crate) const TYPE: u8 = 0x8d;
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.server_session_id == 0
            || self.arena_stream_id == 0
            || self.predecessor_life_seq == 0
            || self.life_seq != self.predecessor_life_seq.wrapping_add(1).max(1)
        {
            return Err("invalid spawn-idle spectator scope".into());
        }
        Ok(())
    }
    pub(crate) fn encode(self) -> Vec<u8> {
        let mut bytes = vec![Self::TYPE, 1];
        bytes.extend(self.server_session_id.to_le_bytes());
        bytes.extend(self.arena_stream_id.to_le_bytes());
        bytes.extend(self.player_id.to_le_bytes());
        bytes.extend(self.predecessor_life_seq.to_le_bytes());
        bytes.extend(self.life_seq.to_le_bytes());
        bytes
    }
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 28 || bytes[0] != Self::TYPE || bytes[1] != 1 {
            return Err("invalid spawn-idle spectator packet".into());
        }
        let fact = Self {
            server_session_id: u64::from_le_bytes(bytes[2..10].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(bytes[10..18].try_into().unwrap()),
            player_id: u16::from_le_bytes(bytes[18..20].try_into().unwrap()),
            predecessor_life_seq: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            life_seq: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        };
        fact.validate()?;
        Ok(fact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spawn_idle_spectator_packet_is_exact_and_validates_scope_and_wrapped_life() {
        for predecessor in [1, u32::MAX] {
            let fact = SpawnIdleSpectator {
                server_session_id: 1,
                arena_stream_id: 2,
                player_id: 3,
                predecessor_life_seq: predecessor,
                life_seq: predecessor.wrapping_add(1).max(1),
            };
            let bytes = fact.encode();
            assert_eq!(SpawnIdleSpectator::parse(&bytes), Ok(fact));
            for len in 0..bytes.len() {
                assert!(SpawnIdleSpectator::parse(&bytes[..len]).is_err());
            }
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(SpawnIdleSpectator::parse(&longer).is_err());
            assert!(
                SpawnIdleSpectator {
                    arena_stream_id: 0,
                    ..fact
                }
                .validate()
                .is_err()
            );
            assert!(
                SpawnIdleSpectator {
                    life_seq: predecessor,
                    ..fact
                }
                .validate()
                .is_err()
            );
        }
    }
}
