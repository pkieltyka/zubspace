//! Bounded server-owned source-life presentation readiness.
//!
//! This is a fairness clock, not client authority. A successful snapshot send
//! records that one exact actor runtime was emitted to one exact recipient. A
//! separately verified application receipt may open harm after the conservative
//! lead; withholding every receipt still opens at the hard server deadline.

use crate::authority_identity::PlayerRuntimeIdentity;
use std::collections::{BTreeMap, VecDeque};

pub(crate) const THREAT_CONSERVATIVE_LEAD_TICKS: u32 = 28;
pub(crate) const THREAT_HARD_OPEN_TICKS: u32 = 50;
pub(crate) const MAX_THREAT_SOURCE_LIVES_PER_RECIPIENT: usize = 512;
/// A successfully emitted weapon/group fact or exact active-projectile state
/// receives this server-owned presentation lead before that projectile may be
/// harmful to the recipient. No client acknowledgement participates, so a
/// withheld receipt cannot extend immunity.
pub(crate) const PROJECTILE_PRESENTATION_LEAD_TICKS: u32 =
    crate::combat_timeline::PROJECTILE_PRESENTATION_MIN_LEAD_TICKS;
/// In the worst case one recipient can retain both the original shot-group
/// carrier and one exact late-AOI state for every globally active projectile.
/// Size both populations explicitly; live contracts are never silently
/// evicted to make room for a newer threat.
pub(crate) const MAX_PROJECTILE_THREATS_PER_RECIPIENT: usize =
    crate::authority_projectiles::MAX_PENDING_HITS_TOTAL * 2;
pub(crate) const PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_CONTRACT: usize =
    std::mem::size_of::<ProjectilePresentationIdentity>()
        + std::mem::size_of::<ProjectilePresentationContract>();
pub(crate) const MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_RECIPIENT: usize =
    MAX_PROJECTILE_THREATS_PER_RECIPIENT * PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_CONTRACT;
pub(crate) const MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_AT_200_PLAYERS: usize =
    MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_RECIPIENT * 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThreatPresentationContract {
    pub(crate) source: PlayerRuntimeIdentity,
    pub(crate) first_snapshot_id: u16,
    pub(crate) first_emitted_tick: u32,
    pub(crate) latest_snapshot_id: u16,
    pub(crate) latest_emitted_tick: u32,
    pub(crate) conservative_open_tick: u32,
    pub(crate) hard_open_tick: u32,
    pub(crate) application_snapshot_id: Option<u16>,
    pub(crate) application_observed_tick: Option<u32>,
}

/// Stable identity shared by the original weapon carrier and later
/// self-contained active-projectile state. `projectile_id == 0` names the
/// complete shot group reconstructed from the weapon event; a nonzero id names
/// one exact child that entered a recipient's threat AOI later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProjectilePresentationIdentity {
    pub(crate) source: PlayerRuntimeIdentity,
    pub(crate) shot_group_id: u64,
    pub(crate) projectile_id: u32,
}

impl ProjectilePresentationIdentity {
    pub(crate) fn shot_group(source: PlayerRuntimeIdentity, shot_group_id: u64) -> Option<Self> {
        (shot_group_id != 0).then_some(Self {
            source,
            shot_group_id,
            projectile_id: 0,
        })
    }

    pub(crate) fn projectile(
        source: PlayerRuntimeIdentity,
        shot_group_id: u64,
        projectile_id: u32,
    ) -> Option<Self> {
        (shot_group_id != 0 && projectile_id != 0).then_some(Self {
            source,
            shot_group_id,
            projectile_id,
        })
    }

    pub(crate) const fn group(self) -> Self {
        Self {
            projectile_id: 0,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectilePresentationContract {
    pub(crate) identity: ProjectilePresentationIdentity,
    pub(crate) queued_event_id: u32,
    pub(crate) queued_tick: u32,
    pub(crate) first_emitted_tick: Option<u32>,
    pub(crate) harmful_open_tick: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectileThreatReadinessReason {
    ProjectileStateNotQueued,
    ProjectileStateQueued,
    AwaitingProjectileLead,
    ProjectileLeadElapsed,
}

impl ProjectileThreatReadinessReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectileStateNotQueued => "projectile_state_not_queued",
            Self::ProjectileStateQueued => "projectile_state_queued",
            Self::AwaitingProjectileLead => "awaiting_projectile_lead",
            Self::ProjectileLeadElapsed => "projectile_lead_elapsed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectileThreatReadinessDecision {
    pub(crate) harmful: bool,
    pub(crate) reason: ProjectileThreatReadinessReason,
    pub(crate) contract: Option<ProjectilePresentationContract>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThreatReadinessReason {
    SourceLifeNotEmitted,
    AwaitingConservativeLead,
    AwaitingApplicationOrHardDeadline,
    VerifiedApplicationAfterLead,
    HardServerDeadline,
}

impl ThreatReadinessReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SourceLifeNotEmitted => "source_life_not_emitted",
            Self::AwaitingConservativeLead => "awaiting_conservative_lead",
            Self::AwaitingApplicationOrHardDeadline => "awaiting_application_or_hard_deadline",
            Self::VerifiedApplicationAfterLead => "verified_application_after_lead",
            Self::HardServerDeadline => "hard_server_deadline",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThreatReadinessDecision {
    pub(crate) harmful: bool,
    pub(crate) reason: ThreatReadinessReason,
    pub(crate) contract: Option<ThreatPresentationContract>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreatPresentationLedger {
    contracts: BTreeMap<PlayerRuntimeIdentity, ThreatPresentationContract>,
    insertion_order: VecDeque<PlayerRuntimeIdentity>,
    projectile_contracts: BTreeMap<ProjectilePresentationIdentity, ProjectilePresentationContract>,
    projectile_insertion_order: VecDeque<ProjectilePresentationIdentity>,
    pub(crate) capacity_evictions: u64,
    pub(crate) stale_runtime_evictions: u64,
    pub(crate) projectile_capacity_rejections: u64,
    pub(crate) projectile_stale_runtime_evictions: u64,
    pub(crate) projectile_peak: usize,
    pub(crate) projectile_terminal_cleanups: u64,
}

impl ThreatPresentationLedger {
    pub(crate) fn can_queue_new_projectiles(&self, count: usize) -> bool {
        self.projectile_contracts.len().saturating_add(count)
            <= MAX_PROJECTILE_THREATS_PER_RECIPIENT
    }

    pub(crate) fn clear(&mut self) {
        self.contracts.clear();
        self.insertion_order.clear();
        self.projectile_contracts.clear();
        self.projectile_insertion_order.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.contracts.len()
    }

    pub(crate) fn projectile_len(&self) -> usize {
        self.projectile_contracts.len()
    }

    pub(crate) fn projectile_retained_payload_bytes(&self) -> usize {
        self.projectile_contracts
            .len()
            .saturating_mul(PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_CONTRACT)
    }

    pub(crate) fn observe_emitted(
        &mut self,
        source: PlayerRuntimeIdentity,
        snapshot_id: u16,
        emitted_tick: u32,
    ) -> ThreatPresentationContract {
        // This owner admits at most one incarnation per numeric slot. An
        // exact repeat therefore cannot coexist with a stale alias: update
        // only its latest emission, preserving FIFO age, first application
        // and both opening deadlines. Avoid a full ledger scan per actor in
        // every recipient snapshot; replacement still takes the path below.
        if let Some(contract) = self.contracts.get_mut(&source) {
            contract.latest_snapshot_id = snapshot_id;
            contract.latest_emitted_tick = emitted_tick;
            return *contract;
        }

        let stale = self
            .contracts
            .keys()
            .copied()
            .filter(|runtime| runtime.player_id() == source.player_id() && *runtime != source)
            .collect::<Vec<_>>();
        for runtime in stale {
            self.contracts.remove(&runtime);
            self.insertion_order
                .retain(|candidate| *candidate != runtime);
            self.stale_runtime_evictions = self.stale_runtime_evictions.saturating_add(1);
            // A projectile from the predecessor life can remain globally
            // authoritative after its source respawns. Exact self-contained
            // projectile state must survive that actor-slot cutover; terminal
            // registry cleanup below owns its lifetime instead.
        }

        while self.contracts.len() >= MAX_THREAT_SOURCE_LIVES_PER_RECIPIENT {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            if self.contracts.remove(&oldest).is_some() {
                self.capacity_evictions = self.capacity_evictions.saturating_add(1);
            }
        }
        let contract = ThreatPresentationContract {
            source,
            first_snapshot_id: snapshot_id,
            first_emitted_tick: emitted_tick,
            latest_snapshot_id: snapshot_id,
            latest_emitted_tick: emitted_tick,
            conservative_open_tick: crate::continuum::continuum_tick_add(
                emitted_tick,
                THREAT_CONSERVATIVE_LEAD_TICKS,
            ),
            hard_open_tick: crate::continuum::continuum_tick_add(
                emitted_tick,
                THREAT_HARD_OPEN_TICKS,
            ),
            application_snapshot_id: None,
            application_observed_tick: None,
        };
        self.contracts.insert(source, contract);
        self.insertion_order.push_back(source);
        contract
    }

    /// Record an application fact only after the caller has verified that the
    /// named emitted snapshot belonged to this recipient and contained the
    /// exact source runtime. This method cannot lengthen either server deadline.
    pub(crate) fn observe_verified_application(
        &mut self,
        source: PlayerRuntimeIdentity,
        snapshot_id: u16,
        observed_tick: u32,
    ) -> bool {
        let Some(contract) = self.contracts.get_mut(&source) else {
            return false;
        };
        if snapshot_id != contract.first_snapshot_id && snapshot_id != contract.latest_snapshot_id {
            return false;
        }
        if !crate::continuum::continuum_tick_reached(observed_tick, contract.first_emitted_tick) {
            return false;
        }
        if contract.application_observed_tick.is_none() {
            contract.application_snapshot_id = Some(snapshot_id);
            contract.application_observed_tick = Some(observed_tick);
        }
        true
    }

    pub(crate) fn readiness(
        &self,
        source: PlayerRuntimeIdentity,
        tick: u32,
    ) -> ThreatReadinessDecision {
        let Some(contract) = self.contracts.get(&source).copied() else {
            return ThreatReadinessDecision {
                harmful: false,
                reason: ThreatReadinessReason::SourceLifeNotEmitted,
                contract: None,
            };
        };
        if crate::continuum::continuum_tick_reached(tick, contract.hard_open_tick) {
            return ThreatReadinessDecision {
                harmful: true,
                reason: ThreatReadinessReason::HardServerDeadline,
                contract: Some(contract),
            };
        }
        if !crate::continuum::continuum_tick_reached(tick, contract.conservative_open_tick) {
            return ThreatReadinessDecision {
                harmful: false,
                reason: ThreatReadinessReason::AwaitingConservativeLead,
                contract: Some(contract),
            };
        }
        if contract.application_observed_tick.is_some() {
            return ThreatReadinessDecision {
                harmful: true,
                reason: ThreatReadinessReason::VerifiedApplicationAfterLead,
                contract: Some(contract),
            };
        }
        ThreatReadinessDecision {
            harmful: false,
            reason: ThreatReadinessReason::AwaitingApplicationOrHardDeadline,
            contract: Some(contract),
        }
    }

    /// Reserve one exact presentation identity when its immutable realtime
    /// event enters the recipient's bounded server queue. Repeated snapshot
    /// scans are idempotent while that event waits for RTC send capacity.
    pub(crate) fn observe_projectile_queued(
        &mut self,
        identity: ProjectilePresentationIdentity,
        event_id: u32,
        queued_tick: u32,
    ) -> bool {
        if identity.shot_group_id == 0 || event_id == 0 {
            return false;
        }
        if self.projectile_contracts.contains_key(&identity) {
            return true;
        }
        if self.projectile_contracts.len() >= MAX_PROJECTILE_THREATS_PER_RECIPIENT {
            self.projectile_capacity_rejections =
                self.projectile_capacity_rejections.saturating_add(1);
            return false;
        }
        self.projectile_contracts.insert(
            identity,
            ProjectilePresentationContract {
                identity,
                queued_event_id: event_id,
                queued_tick,
                first_emitted_tick: None,
                harmful_open_tick: None,
            },
        );
        self.projectile_insertion_order.push_back(identity);
        self.projectile_peak = self.projectile_peak.max(self.projectile_contracts.len());
        true
    }

    /// Mark the exact queued event as successfully handed to the RTC edge.
    /// The deadline derives only from this server fact and cannot be delayed
    /// or accelerated by a client receipt.
    pub(crate) fn observe_projectile_emitted(
        &mut self,
        identity: ProjectilePresentationIdentity,
        event_id: u32,
        emitted_tick: u32,
    ) -> bool {
        let Some(contract) = self.projectile_contracts.get_mut(&identity) else {
            return false;
        };
        if contract.queued_event_id != event_id {
            return false;
        }
        if contract.first_emitted_tick.is_none() {
            contract.first_emitted_tick = Some(emitted_tick);
            contract.harmful_open_tick = Some(crate::continuum::continuum_tick_add(
                emitted_tick,
                PROJECTILE_PRESENTATION_LEAD_TICKS,
            ));
        }
        true
    }

    pub(crate) fn projectile_known(&self, exact: ProjectilePresentationIdentity) -> bool {
        self.projectile_contracts.contains_key(&exact)
            || self.projectile_contracts.contains_key(&exact.group())
    }

    pub(crate) fn projectile_group_known(&self, identity: ProjectilePresentationIdentity) -> bool {
        let first = identity.group();
        let last = ProjectilePresentationIdentity {
            projectile_id: u32::MAX,
            ..first
        };
        self.projectile_contracts
            .range(first..=last)
            .next()
            .is_some()
    }

    pub(crate) fn exact_projectile_known(&self, exact: ProjectilePresentationIdentity) -> bool {
        self.projectile_contracts.contains_key(&exact)
    }

    pub(crate) fn projectile_readiness(
        &self,
        exact: ProjectilePresentationIdentity,
        tick: u32,
    ) -> ProjectileThreatReadinessDecision {
        let contract = self
            .projectile_contracts
            .get(&exact)
            .or_else(|| self.projectile_contracts.get(&exact.group()))
            .copied();
        projectile_contract_readiness(contract, tick)
    }

    pub(crate) fn exact_projectile_readiness(
        &self,
        exact: ProjectilePresentationIdentity,
        tick: u32,
    ) -> ProjectileThreatReadinessDecision {
        projectile_contract_readiness(self.projectile_contracts.get(&exact).copied(), tick)
    }

    pub(crate) fn shot_group_readiness(
        &self,
        exact: ProjectilePresentationIdentity,
        tick: u32,
    ) -> ProjectileThreatReadinessDecision {
        projectile_contract_readiness(self.projectile_contracts.get(&exact.group()).copied(), tick)
    }

    /// Remove terminal projectile/group identities after the authoritative
    /// registry no longer contains them. This keeps ordinary sessions close
    /// to active load while preserving the hard capacity proof above.
    pub(crate) fn retain_projectiles(
        &mut self,
        mut keep: impl FnMut(ProjectilePresentationIdentity) -> bool,
    ) {
        let before = self.projectile_contracts.len();
        self.projectile_contracts
            .retain(|identity, _| keep(*identity));
        self.projectile_insertion_order
            .retain(|identity| self.projectile_contracts.contains_key(identity));
        self.projectile_terminal_cleanups = self
            .projectile_terminal_cleanups
            .saturating_add(before.saturating_sub(self.projectile_contracts.len()) as u64);
    }
}

fn projectile_contract_readiness(
    contract: Option<ProjectilePresentationContract>,
    tick: u32,
) -> ProjectileThreatReadinessDecision {
    let Some(contract) = contract else {
        return ProjectileThreatReadinessDecision {
            harmful: false,
            reason: ProjectileThreatReadinessReason::ProjectileStateNotQueued,
            contract: None,
        };
    };
    let Some(open_tick) = contract.harmful_open_tick else {
        return ProjectileThreatReadinessDecision {
            harmful: false,
            reason: ProjectileThreatReadinessReason::ProjectileStateQueued,
            contract: Some(contract),
        };
    };
    if crate::continuum::continuum_tick_reached(tick, open_tick) {
        ProjectileThreatReadinessDecision {
            harmful: true,
            reason: ProjectileThreatReadinessReason::ProjectileLeadElapsed,
            contract: Some(contract),
        }
    } else {
        ProjectileThreatReadinessDecision {
            harmful: false,
            reason: ProjectileThreatReadinessReason::AwaitingProjectileLead,
            contract: Some(contract),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority_identity::{LifeIdentity, PlayerSessionId, PlayerSessionIdentity};

    fn runtime(player_id: u16, session_id: u64, life_seq: u32) -> PlayerRuntimeIdentity {
        PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(player_id, PlayerSessionId::new(session_id)),
            LifeIdentity::new(player_id, life_seq),
        )
    }

    #[test]
    fn repeated_emission_matches_linear_slot_lifecycle_model() {
        // Independent insertion-ordered model: one actor incarnation per
        // numeric slot, FIFO capacity, immutable opening deadlines. In
        // particular, repeated emission must NOT refresh insertion order or
        // discard the first application fact.
        let mut ledger = ThreatPresentationLedger::default();
        let mut model: Vec<ThreatPresentationContract> = Vec::new();
        let mut stale = 0_u64;
        let mut capacity = 0_u64;
        for step in 0_u32..8000 {
            if step == 4100 {
                ledger.clear();
                model.clear();
            }
            let slot = ((step / 3) % 601) as u16;
            let source = runtime(slot, u64::from(step / 2400 + 1), step / 1900 + 1);
            let snapshot = (65500_u32 + step) as u16;
            let tick = (0x7fff_fff0_u32 + step) & 0x7fff_ffff;
            let old_len = model.len();
            model.retain(|row| row.source.player_id() != slot || row.source == source);
            stale += (old_len - model.len()) as u64;
            if let Some(row) = model.iter_mut().find(|row| row.source == source) {
                row.latest_snapshot_id = snapshot;
                row.latest_emitted_tick = tick;
            } else {
                if model.len() == MAX_THREAT_SOURCE_LIVES_PER_RECIPIENT {
                    model.remove(0);
                    capacity += 1;
                }
                model.push(ThreatPresentationContract {
                    source,
                    first_snapshot_id: snapshot,
                    first_emitted_tick: tick,
                    latest_snapshot_id: snapshot,
                    latest_emitted_tick: tick,
                    conservative_open_tick: (tick + THREAT_CONSERVATIVE_LEAD_TICKS) & 0x7fff_ffff,
                    hard_open_tick: (tick + THREAT_HARD_OPEN_TICKS) & 0x7fff_ffff,
                    application_snapshot_id: None,
                    application_observed_tick: None,
                });
            }
            let expected = model.iter_mut().find(|row| row.source == source).unwrap();
            assert_eq!(ledger.observe_emitted(source, snapshot, tick), *expected);
            if step % 3 == 0 {
                assert!(ledger.observe_verified_application(source, snapshot, tick));
                if expected.application_observed_tick.is_none() {
                    expected.application_snapshot_id = Some(snapshot);
                    expected.application_observed_tick = Some(tick);
                }
            }
            assert_eq!(ledger.capacity_evictions, capacity);
            assert_eq!(ledger.stale_runtime_evictions, stale);
            assert_eq!(ledger.contracts.len(), model.len());
            assert!(
                ledger
                    .insertion_order
                    .iter()
                    .copied()
                    .eq(model.iter().map(|row| row.source))
            );
            for row in &model {
                assert_eq!(ledger.contracts.get(&row.source), Some(row));
                assert!(ledger.readiness(row.source, row.hard_open_tick).harmful);
            }
        }
        assert!(
            stale > 0 && capacity > 0,
            "exercise both replacement and capacity"
        );
    }

    #[test]
    fn receipt_tiers_cannot_open_before_emission_or_conservative_lead() {
        let source = runtime(7, 11, 3);
        let mut ledger = ThreatPresentationLedger::default();
        assert_eq!(
            ledger.readiness(source, 100).reason,
            ThreatReadinessReason::SourceLifeNotEmitted
        );
        let contract = ledger.observe_emitted(source, 19, 100);
        assert!(
            !ledger
                .readiness(source, contract.conservative_open_tick - 1)
                .harmful
        );
        assert!(ledger.observe_verified_application(source, 19, 101));
        assert!(
            !ledger
                .readiness(source, contract.conservative_open_tick - 1)
                .harmful
        );
        assert_eq!(
            ledger
                .readiness(source, contract.conservative_open_tick)
                .reason,
            ThreatReadinessReason::VerifiedApplicationAfterLead
        );
    }

    #[test]
    fn forged_application_identity_or_pre_emission_time_cannot_open_readiness() {
        let source = runtime(7, 11, 3);
        let forged_source = runtime(8, 12, 1);
        let mut ledger = ThreatPresentationLedger::default();
        let contract = ledger.observe_emitted(source, 19, 100);
        assert!(!ledger.observe_verified_application(forged_source, 19, 101));
        assert!(!ledger.observe_verified_application(source, 18, 101));
        assert!(!ledger.observe_verified_application(source, 19, 99));
        assert_eq!(
            ledger
                .readiness(source, contract.conservative_open_tick)
                .reason,
            ThreatReadinessReason::AwaitingApplicationOrHardDeadline
        );
        assert!(ledger.observe_verified_application(source, 19, 101));
        assert_eq!(
            ledger
                .readiness(source, contract.conservative_open_tick)
                .reason,
            ThreatReadinessReason::VerifiedApplicationAfterLead
        );
    }

    #[test]
    fn withheld_application_opens_only_at_the_hard_server_deadline() {
        let source = runtime(7, 11, 3);
        let mut ledger = ThreatPresentationLedger::default();
        let contract = ledger.observe_emitted(source, 19, 100);
        assert_eq!(
            ledger
                .readiness(source, contract.conservative_open_tick)
                .reason,
            ThreatReadinessReason::AwaitingApplicationOrHardDeadline
        );
        assert!(
            !ledger
                .readiness(source, contract.hard_open_tick - 1)
                .harmful
        );
        assert_eq!(
            ledger.readiness(source, contract.hard_open_tick).reason,
            ThreatReadinessReason::HardServerDeadline
        );
    }

    #[test]
    fn exact_life_and_session_replacement_evicts_numeric_id_aliases() {
        let old = runtime(7, 11, 3);
        let respawn = runtime(7, 11, 4);
        let reconnect = runtime(7, 12, 1);
        let mut ledger = ThreatPresentationLedger::default();
        ledger.observe_emitted(old, 19, 100);
        ledger.observe_emitted(respawn, 20, 101);
        assert_eq!(ledger.len(), 1);
        assert_eq!(
            ledger.readiness(old, 10_000).reason,
            ThreatReadinessReason::SourceLifeNotEmitted
        );
        ledger.observe_emitted(reconnect, 21, 102);
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.stale_runtime_evictions, 2);
        assert_eq!(
            ledger.readiness(respawn, 10_000).reason,
            ThreatReadinessReason::SourceLifeNotEmitted
        );
    }

    #[test]
    fn deadlines_remain_wrap_safe_and_capacity_is_explicit() {
        let source = runtime(7, 11, 3);
        let mut ledger = ThreatPresentationLedger::default();
        let emitted = u32::MAX - 20;
        let contract = ledger.observe_emitted(source, 19, emitted);
        assert!(
            !ledger
                .readiness(source, contract.hard_open_tick.wrapping_sub(1))
                .harmful
        );
        assert!(ledger.readiness(source, contract.hard_open_tick).harmful);
        for index in 0..=MAX_THREAT_SOURCE_LIVES_PER_RECIPIENT {
            ledger.observe_emitted(runtime(index as u16 + 100, index as u64 + 100, 1), 1, 1);
        }
        assert_eq!(ledger.len(), MAX_THREAT_SOURCE_LIVES_PER_RECIPIENT);
        assert!(ledger.capacity_evictions > 0);
    }

    #[test]
    fn projectile_group_and_exact_state_converge_across_arrival_orders() {
        let source = runtime(7, 11, 3);
        let group = ProjectilePresentationIdentity::shot_group(source, 41).expect("group");
        let exact = ProjectilePresentationIdentity::projectile(source, 41, 99).expect("exact");
        for (queued, queried) in [(group, exact), (exact, exact)] {
            let mut ledger = ThreatPresentationLedger::default();
            assert!(ledger.observe_projectile_queued(queued, 70, 100));
            assert!(ledger.projectile_known(exact));
            assert_eq!(
                ledger.projectile_readiness(queried, 10_000).reason,
                ProjectileThreatReadinessReason::ProjectileStateQueued,
                "queue admission is not wire emission",
            );
            assert!(ledger.observe_projectile_emitted(queued, 70, 105));
            let open_tick =
                crate::continuum::continuum_tick_add(105, PROJECTILE_PRESENTATION_LEAD_TICKS);
            assert_eq!(
                ledger
                    .projectile_readiness(queried, open_tick.wrapping_sub(1))
                    .reason,
                ProjectileThreatReadinessReason::AwaitingProjectileLead,
            );
            assert_eq!(
                ledger.projectile_readiness(queried, open_tick).reason,
                ProjectileThreatReadinessReason::ProjectileLeadElapsed,
            );
        }
    }

    #[test]
    fn projectile_deadline_is_server_owned_wrap_safe_and_event_exact() {
        let source = runtime(7, 11, 3);
        let exact = ProjectilePresentationIdentity::projectile(source, 41, 99).expect("exact");
        let mut ledger = ThreatPresentationLedger::default();
        let emitted = u32::MAX - 10;
        assert!(ledger.observe_projectile_queued(exact, 70, emitted - 1));
        assert!(!ledger.observe_projectile_emitted(exact, 71, emitted));
        assert_eq!(
            ledger.projectile_readiness(exact, u32::MAX).reason,
            ProjectileThreatReadinessReason::ProjectileStateQueued,
        );
        assert!(ledger.observe_projectile_emitted(exact, 70, emitted));
        let open_tick =
            crate::continuum::continuum_tick_add(emitted, PROJECTILE_PRESENTATION_LEAD_TICKS);
        assert!(
            !ledger
                .projectile_readiness(exact, open_tick.wrapping_sub(1))
                .harmful
        );
        assert!(ledger.projectile_readiness(exact, open_tick).harmful);
    }

    #[test]
    fn projectile_survives_source_respawn_until_exact_terminal_cleanup() {
        let old = runtime(7, 11, 3);
        let successor = runtime(7, 11, 4);
        let old_exact = ProjectilePresentationIdentity::projectile(old, 41, 99).expect("old exact");
        let successor_exact =
            ProjectilePresentationIdentity::projectile(successor, 42, 100).expect("successor");
        let mut ledger = ThreatPresentationLedger::default();
        assert!(ledger.observe_projectile_queued(old_exact, 70, 100));
        assert!(ledger.observe_projectile_emitted(old_exact, 70, 101));
        ledger.observe_emitted(old, 1, 100);
        ledger.observe_emitted(successor, 2, 102);
        assert_eq!(ledger.projectile_len(), 1);
        assert_eq!(ledger.projectile_stale_runtime_evictions, 0);
        assert!(ledger.projectile_readiness(old_exact, 10_000).harmful);

        assert!(ledger.observe_projectile_queued(successor_exact, 71, 103));
        ledger.retain_projectiles(|identity| identity == old_exact || identity == successor_exact);
        assert_eq!(ledger.projectile_len(), 2);
        assert_eq!(ledger.projectile_peak, 2);
        assert_eq!(ledger.projectile_terminal_cleanups, 0);
        ledger.retain_projectiles(|identity| identity == successor_exact);
        assert_eq!(ledger.projectile_len(), 1);
        assert_eq!(ledger.projectile_terminal_cleanups, 1);
        ledger.retain_projectiles(|_| false);
        assert_eq!(ledger.projectile_len(), 0);
        assert_eq!(ledger.projectile_terminal_cleanups, 2);
    }

    #[test]
    fn projectile_capacity_rejects_without_evicting_a_live_contract() {
        let mut ledger = ThreatPresentationLedger::default();
        for index in 0..MAX_PROJECTILE_THREATS_PER_RECIPIENT {
            let source = runtime(
                (index % 700) as u16 + 1,
                index as u64 + 1,
                (index / 700) as u32 + 1,
            );
            let identity = ProjectilePresentationIdentity::projectile(
                source,
                index as u64 + 1,
                index as u32 + 1,
            )
            .expect("bounded identity");
            assert!(ledger.observe_projectile_queued(identity, index as u32 + 1, 1));
        }
        let first = *ledger
            .projectile_contracts
            .keys()
            .next()
            .expect("first retained identity");
        let overflow =
            ProjectilePresentationIdentity::projectile(runtime(701, 9_999, 1), 9_999, 9_999)
                .expect("overflow identity");
        assert!(!ledger.observe_projectile_queued(overflow, 9_999, 1));
        assert_eq!(
            ledger.projectile_len(),
            MAX_PROJECTILE_THREATS_PER_RECIPIENT
        );
        assert!(ledger.projectile_contracts.contains_key(&first));
        assert_eq!(ledger.projectile_capacity_rejections, 1);
        assert_eq!(ledger.projectile_peak, MAX_PROJECTILE_THREATS_PER_RECIPIENT);
        assert_eq!(
            ledger.projectile_retained_payload_bytes(),
            MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_RECIPIENT,
        );
        assert!(
            MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_AT_200_PLAYERS <= 128 * 1024 * 1024,
            "presentation ledger payload budget is {} bytes at 200 players",
            MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_AT_200_PLAYERS,
        );
    }
}
