#![allow(dead_code)]

/// Never-reused identity for one registered transport connection.
///
/// This stays internal: Continuum's public player id remains the wire identity,
/// while ingress uses this token to prove that a packet or cleanup callback
/// still owns the current occupant of that reusable id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PlayerSessionId(u64);

impl PlayerSessionId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// Connection-bound identity carried from transport lifecycle lookup through
/// packet dispatch and disconnect cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PlayerSessionIdentity {
    pub(crate) player_id: u16,
    pub(crate) session_id: PlayerSessionId,
}

impl PlayerSessionIdentity {
    pub(crate) const fn new(player_id: u16, session_id: PlayerSessionId) -> Self {
        Self {
            player_id,
            session_id,
        }
    }
}

/// Exact client runtime incarnation used to bind delayed outbound artifacts.
///
/// `session` prevents reconnect/id-reuse delivery, while `life` prevents an
/// artifact captured before a death, ship/frequency change, leave/rejoin, or
/// arena reset from reaching the newer runtime on the same connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PlayerRuntimeIdentity {
    pub(crate) session: PlayerSessionIdentity,
    pub(crate) life: LifeIdentity,
}

impl PlayerRuntimeIdentity {
    pub(crate) const fn new(session: PlayerSessionIdentity, life: LifeIdentity) -> Self {
        Self { session, life }
    }

    pub(crate) const fn player_id(self) -> u16 {
        self.session.player_id
    }
}

/// Server-owned identity for one player life/loadout generation.
///
/// Continuum player ids are intentionally reusable. `life_seq` is therefore
/// part of deferred gameplay authority references. The sequence wraps from
/// `u32::MAX` to 1; correctness assumes no deferred authority work survives a
/// full 2^32-generation cycle. Reconnect/ID-reuse safety is instead provided
/// by the never-reused `PlayerSessionId` at ingress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct LifeIdentity {
    pub(crate) player_id: u16,
    pub(crate) life_seq: u32,
}

impl LifeIdentity {
    pub(crate) const fn new(player_id: u16, life_seq: u32) -> Self {
        Self {
            player_id,
            life_seq,
        }
    }
}
