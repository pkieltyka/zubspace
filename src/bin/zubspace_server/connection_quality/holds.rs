//! Reconnect-persistent holds. The server lock owns this registry; transport
//! sessions and arena streams deliberately do not own its lifetime.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::COOLDOWN;

const MAX_HOLDS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QualityHold {
    pub(crate) id: u64,
    until: Instant,
}

impl QualityHold {
    pub(crate) fn remaining(self, now: Instant) -> Duration {
        self.until.saturating_duration_since(now)
    }

    pub(crate) fn may_return(self, now: Instant, sustained_healthy: bool) -> bool {
        self.remaining(now).is_zero() && sustained_healthy
    }
}

#[derive(Debug, Default)]
pub(crate) struct QualityHolds {
    records: HashMap<String, QualityHold>,
    last_id: u64,
    pub(crate) forced: u64,
    pub(crate) denied: u64,
    pub(crate) returned: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HoldAllocationError {
    Capacity,
    Identity,
    Exhausted,
}

impl QualityHolds {
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
    pub(crate) fn get(&self, identity: &str) -> Option<QualityHold> {
        self.records.get(identity).copied()
    }

    /// At capacity, refuse new identities at admission rather than evicting an
    /// expired but unserved hold. Callers must check again under the server lock
    /// when allocating; no decision may force a player without a durable record.
    pub(crate) fn accepts_identity(&self, identity: &str) -> bool {
        self.records.contains_key(identity) || self.records.len() < MAX_HOLDS
    }

    /// Every admitted identity without a hold reserves one possible future
    /// record. Count those identities while holding the same ServerState lock
    /// used to publish the new login, so concurrent logins cannot oversubscribe.
    pub(crate) fn can_admit(&self, identity: &str, other_unheld_identities: usize) -> bool {
        !identity.is_empty()
            && identity.len() <= 128
            && (self.records.contains_key(identity)
                || (self.last_id < u64::MAX
                    && self.records.len().saturating_add(other_unheld_identities) < MAX_HOLDS))
    }

    /// Idempotent for an existing identity, including after the deadline. A
    /// reconnect or repeated degradation must never restart the one-minute deadline.
    pub(crate) fn begin(
        &mut self,
        identity: &str,
        now: Instant,
    ) -> Result<QualityHold, HoldAllocationError> {
        let hold = self.prepare(identity, now)?;
        if !self.records.contains_key(identity) {
            self.records.insert(identity.to_owned(), hold);
            self.last_id = hold.id;
        }
        Ok(hold)
    }

    /// Preflight under the server transaction lock. Only commit with `begin`
    /// after the roster transition succeeds; deferred cleanup starts no timer.
    pub(crate) fn prepare(
        &self,
        identity: &str,
        now: Instant,
    ) -> Result<QualityHold, HoldAllocationError> {
        // Login identities are bounded hexadecimal strings. Keep this module
        // bounded even if a future caller forgets the login parser's check.
        if identity.is_empty() || identity.len() > 128 {
            return Err(HoldAllocationError::Identity);
        }
        if let Some(hold) = self.get(identity) {
            return Ok(hold);
        }
        if !self.accepts_identity(identity) {
            return Err(HoldAllocationError::Capacity);
        }
        let id = self
            .last_id
            .checked_add(1)
            .ok_or(HoldAllocationError::Exhausted)?;
        Ok(QualityHold {
            id,
            until: now + COOLDOWN,
        })
    }

    /// Only the transaction committing a healthy return may remove a hold.
    /// Checking the exact id makes a delayed return unable to clear a new hold.
    pub(crate) fn finish(
        &mut self,
        identity: &str,
        hold_id: u64,
        now: Instant,
        sustained_healthy: bool,
    ) -> bool {
        if self
            .get(identity)
            .is_none_or(|hold| hold.id != hold_id || !hold.may_return(now, sustained_healthy))
        {
            return false;
        }
        self.records.remove(identity);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_hold_survives_reconnect_without_extending_or_automatic_expiry() {
        let base = Instant::now();
        let mut holds = QualityHolds::default();
        let hold = holds.begin("same-browser-identity", base).unwrap();
        for seconds in [1, 30, 59, 60, 61, 3600] {
            let now = base + Duration::from_secs(seconds);
            assert_eq!(holds.begin("same-browser-identity", now), Ok(hold));
            assert_eq!(holds.get("same-browser-identity"), Some(hold));
            assert!(!holds.finish("same-browser-identity", hold.id, now, false));
            assert_eq!(hold.may_return(now, true), seconds >= 60);
        }
        assert!(!holds.finish(
            "same-browser-identity",
            hold.id,
            base + COOLDOWN - Duration::from_nanos(1),
            true
        ));
        assert!(holds.finish("same-browser-identity", hold.id, base + COOLDOWN, true));
        assert!(holds.get("same-browser-identity").is_none());
        let replacement = holds
            .begin("same-browser-identity", base + COOLDOWN)
            .unwrap();
        assert!(replacement.id > hold.id);
        assert!(!holds.finish("same-browser-identity", hold.id, base + COOLDOWN * 2, true));
        assert_eq!(holds.get("same-browser-identity"), Some(replacement));
    }

    #[test]
    fn quality_hold_capacity_never_discards_an_unserved_identity() {
        let base = Instant::now();
        let mut holds = QualityHolds::default();
        for i in 0..MAX_HOLDS {
            holds.begin(&format!("identity-{i}"), base).unwrap();
        }
        assert!(!holds.accepts_identity("new-identity"));
        assert_eq!(
            holds.begin("new-identity", base + COOLDOWN * 10),
            Err(HoldAllocationError::Capacity)
        );
        let existing = holds.get("identity-0").unwrap();
        assert!(holds.accepts_identity("identity-0"));
        assert_eq!(
            holds.begin("identity-0", base + COOLDOWN * 10),
            Ok(existing)
        );
        assert!(holds.finish("identity-0", existing.id, base + COOLDOWN, true));
        assert!(holds.accepts_identity("new-identity"));
        assert!(holds.begin("new-identity", base + COOLDOWN).unwrap().id > existing.id);
        assert_eq!(holds.records.len(), MAX_HOLDS);
    }

    #[test]
    fn quality_hold_rejects_unbounded_keys_and_id_exhaustion() {
        let base = Instant::now();
        let mut holds = QualityHolds::default();
        for identity in ["", &"x".repeat(129)] {
            assert_eq!(
                holds.begin(identity, base),
                Err(HoldAllocationError::Identity)
            );
        }
        let existing = holds.begin("existing", base).unwrap();
        holds.last_id = u64::MAX;
        assert_eq!(
            holds.begin("new", base),
            Err(HoldAllocationError::Exhausted)
        );
        assert_eq!(holds.begin("existing", base), Ok(existing));
        assert_eq!(holds.records.len(), 1);
    }

    #[test]
    fn quality_hold_admission_reserves_space_for_every_unheld_login() {
        let base = Instant::now();
        let mut holds = QualityHolds::default();
        assert!(holds.can_admit("new", MAX_HOLDS - 1));
        assert!(!holds.can_admit("new", MAX_HOLDS));
        assert!(!holds.can_admit("new", usize::MAX));
        holds.begin("held", base).unwrap();
        assert!(holds.can_admit("held", MAX_HOLDS));
        assert!(!holds.can_admit("new", MAX_HOLDS - 1));
        assert!(holds.can_admit("new", MAX_HOLDS - 2));
        assert!(!holds.can_admit("", 0));
        holds.last_id = u64::MAX;
        assert!(!holds.can_admit("new", 0));
        assert!(holds.can_admit("held", 0));
    }
}
