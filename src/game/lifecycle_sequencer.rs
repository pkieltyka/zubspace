//! Typed client authority/lifecycle owner.
//!
//! The sequencer is the sole mutable owner of client authority scope,
//! lifecycle transitions, correction application barriers, and cross-poll
//! authority staging. Callers ingest immutable facts and consume typed views
//! or bounded drains; they do not mirror lifecycle state.

use super::*;
use std::collections::VecDeque;

const MAX_CLOSED_TRANSITIONS: usize = 256;
const MAX_TRANSITION_LEDGER: usize = 256;
const MAX_CORRECTION_TERMINALS: usize = 4096;
/// The browser bridge admits at most 4,096 packets on each of the three RTC
/// lanes. A correction may hold facts across polls, so retain at most that
/// complete cross-lane horizon before failing closed instead of silently
/// dropping an already-decoded authority fact.
const MAX_DEFERRED_AUTHORITY_ACTIONS: usize = 3 * 4_096;
const MAX_DEFERRED_AUTHORITY_PACKETS: usize = 256;
const MAX_DEFERRED_AUTHORITY_PACKET_BYTES: usize =
    MAX_DEFERRED_AUTHORITY_PACKETS * crate::protocol::ReliableSequencer::MAX_PACKET_SIZE;
/// The authoritative server can expose no more than 256 active collectibles
/// in one arena. A larger set of unique Warp ACKs in one life is impossible.
const MAX_DEFERRED_WARP_PICKUP_ACKS: usize = 256;
/// At most one application boundary can originate from each active
/// collectible. The server's authoritative registry is capped at 256, so a
/// larger same-life client queue is impossible and fails closed.
const MAX_PRIZE_EFFECT_APPLICATION_BARRIERS: usize = 256;
const MAX_PRIZE_EFFECT_TERMINALS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::game) struct ClientPrizeEffectApplication {
    pub(in crate::game) application: PrizeEffectApplied,
    pub(in crate::game) engine_shutdown_until_tick: Option<u32>,
    operation: Option<DurableOperation>,
}

impl ClientPrizeEffectApplication {
    pub(in crate::game) fn submitted(self) -> bool {
        self.operation.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClientPortalWarp {
    request: PortalWarpRequest,
    operation: DurableOperation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::game) struct ClientAuthorityReadiness {
    pub(in crate::game) joined: bool,
    pub(in crate::game) arena_epoch: Option<u32>,
    pub(in crate::game) settings: bool,
    pub(in crate::game) security: bool,
    pub(in crate::game) map: bool,
    pub(in crate::game) files: bool,
}

impl ClientAuthorityReadiness {
    fn initial_baseline_ready(self) -> bool {
        // Preserve the current gameplay boundary: authenticated map
        // publication opens gameplay while optional LVZ files may continue.
        // Files remain tracked so later ship/frequency requests can require
        // their stronger reliable-FIFO publication boundary.
        self.joined && self.settings && self.security && self.map
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)] // later explicit phases are populated by bounded migration slices
pub(in crate::game) enum ClientAuthorityPhase {
    #[default]
    Disconnected,
    Bootstrapping,
    AwaitingReadiness,
    AwaitingDurableSubmission,
    AwaitingApplicationReceipt,
    Live,
    Spectating,
    ShipFrequencyTransition,
    DeadRespawning,
    CorrectionHeld,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // realtime lifecycle facts enter in the correction slice
pub(in crate::game) enum ClientAuthoritySource {
    Local,
    Realtime,
    Event,
    Reliable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum ClientAuthorityFactKind {
    HardReplacement,
    JoinedArena,
    SettingsReady,
    SecurityReady,
    SecurityPending,
    MapPending,
    MapReady,
    FilesPending,
    FilesReady,
    PrivateLifecycle,
    LifecycleIntentQueued,
    LifecycleRequestSubmitted,
    LifecycleAcknowledged,
    LifecycleRejected,
    ConnectionQuality,
    SpawnIdle,
    DurableSubmitted,
    ApplicationReceipt,
    CorrectionProposed,
    CorrectionSubmitted,
    CorrectionTerminal,
    CorrectionCancelled,
    DeferredActionsRejected,
    DeferredActionsOverflow,
    ApplicationBarrierOverflow,
    ApplicationBarrierArmed,
    ApplicationBarrierSubmitted,
    ApplicationBarrierTerminal,
    ApplicationBarrierCancelled,
    Deadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // cancellation reason is populated by later lifecycle slices
pub(in crate::game) enum ClientAuthorityRejection {
    SegmentMismatch,
    ScopeMismatch,
    LifeMismatch,
    DeliveryMismatch,
    ImpossiblePhase,
    Cancelled,
    AdmissionMismatch,
    CapacityExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum ClientAuthorityDisposition {
    Applied,
    Replacement,
    Duplicate,
    Observed,
    TimedOut,
    /// A wait that owns no delivery to retry has expired: re-ask the server rather than failing the transition.
    Reattempt,
    Rejected(ClientAuthorityRejection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // remaining variants are activated one reviewed migration slice at a time
pub(in crate::game) enum ClientAuthorityTransitionKind {
    InitialJoin,
    Ship,
    Frequency,
    Respawn,
    Correction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum ClientAuthorityApplicationBarrierKind {
    PrizeEffect,
    PortalWarp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DurableOperation {
    delivery_id: u32,
    sequence: u16,
    elapsed_seconds: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeferredCombatViewAction {
    segment: u64,
    scope: AuthorityScopeIdentity,
    action: SemanticInputAction,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InitialJoinTransition {
    life_seq: Option<u32>,
    operation: Option<DurableOperation>,
    /// Time waited *before* the baseline was submitted. The submitted wait has its own clock inside
    /// `DurableOperation`; this one exists because an unsubmitted join used to wait forever.
    pre_submission_seconds: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct LifecycleCutoverTransition {
    intent: PendingLifecycleRequest,
    predecessor_life_seq: u32,
    request_sequence: Option<u16>,
    target_life_seq: Option<u32>,
    acknowledged: bool,
    baseline: Option<ClientPositionUpdate>,
    accepted_baseline: Option<ClientPositionUpdate>,
    operation: Option<DurableOperation>,
    elapsed_seconds: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RespawnTransition {
    target_life_seq: u32,
    respawn_at_tick: Option<u32>,
    remaining_seconds: f32,
    baseline_ready: bool,
    server_alive_observed: bool,
    operation: Option<DurableOperation>,
}

#[derive(Clone, Debug, PartialEq)]
struct CorrectionTransition {
    proposal: MovementCorrection,
    application: MovementCorrectionApplied,
    operation: Option<DurableOperation>,
    retained_actions: VecDeque<SemanticInputAction>,
}

fn correction_proposal_matches_terminal(
    proposal: MovementCorrection,
    receipt: MovementCorrectionAccepted,
) -> bool {
    proposal.server_session_id == receipt.server_session_id
        && proposal.player_id == receipt.player_id
        && proposal.life_seq == receipt.life_seq
        && proposal.correction_generation == receipt.correction_generation
        && proposal.position_sequence == receipt.detected_sequence
}

fn correction_application_matches_terminal(
    application: MovementCorrectionApplied,
    receipt: MovementCorrectionAccepted,
) -> bool {
    application.intent == MovementCorrectionApplied::INTENT_APPLY
        && receipt.outcome == MovementCorrectionAccepted::OUTCOME_APPLIED
        && application.server_session_id == receipt.server_session_id
        && application.player_id == receipt.player_id
        && application.life_seq == receipt.life_seq
        && application.correction_generation == receipt.correction_generation
        && application.detected_sequence == receipt.detected_sequence
        && application.application_sequence == receipt.application_sequence
}

pub(in crate::game) fn prize_effect_acceptance_identity_matches(
    application: PrizeEffectApplied,
    receipt: PrizeEffectAccepted,
) -> bool {
    application.server_session_id == receipt.server_session_id
        && application.player_id == receipt.player_id
        && application.life_seq == receipt.life_seq
        && application.collectible_id == receipt.collectible_id
        && application.prize_id == receipt.prize_id
        && application.prize_state_revision == receipt.prize_state_revision
        && application.application_sequence == receipt.requested_application_sequence
}

pub(in crate::game) fn prize_effect_acceptance_matches(
    application: PrizeEffectApplied,
    receipt: PrizeEffectAccepted,
) -> bool {
    prize_effect_acceptance_identity_matches(application, receipt)
        && application.application_sequence == receipt.application_sequence
}

pub(in crate::game) fn prize_effect_rebase_moves_forward(
    application: PrizeEffectApplied,
    receipt: PrizeEffectAccepted,
) -> bool {
    prize_effect_acceptance_identity_matches(application, receipt) && {
        let forward = receipt
            .application_sequence
            .wrapping_sub(application.application_sequence);
        forward != 0 && forward < (1 << 15)
    }
}

fn portal_warp_acceptance_matches(request: PortalWarpRequest, receipt: PortalWarpAccepted) -> bool {
    request.server_session_id == receipt.server_session_id
        && request.arena_stream_id == receipt.arena_stream_id
        && request.player_id == receipt.player_id
        && request.life_seq == receipt.life_seq
        && request.action_id == receipt.action_id
        && request.position_sequence == receipt.position_sequence
        && (!request.force_random || !receipt.used_portal)
}

impl LifecycleCutoverTransition {
    fn kind(&self) -> ClientAuthorityTransitionKind {
        match self.intent {
            PendingLifecycleRequest::Ship(_) => ClientAuthorityTransitionKind::Ship,
            PendingLifecycleRequest::Frequency(_) => ClientAuthorityTransitionKind::Frequency,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum PendingTransition {
    InitialJoin(InitialJoinTransition),
    Lifecycle(LifecycleCutoverTransition),
    Respawn(RespawnTransition),
    Correction(CorrectionTransition),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClosedTransition {
    segment: u64,
    scope: AuthorityScopeIdentity,
    kind: ClientAuthorityTransitionKind,
    delivery_id: u32,
    sequence: u16,
    completed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::game) struct ClientAuthorityLedgerEntry {
    pub(in crate::game) source: ClientAuthoritySource,
    pub(in crate::game) fact: ClientAuthorityFactKind,
    pub(in crate::game) scope: AuthorityScopeIdentity,
    pub(in crate::game) prior: ClientAuthorityPhase,
    pub(in crate::game) disposition: ClientAuthorityDisposition,
    pub(in crate::game) resulting: ClientAuthorityPhase,
    pub(in crate::game) delivery_id: Option<u32>,
    pub(in crate::game) application_identity: Option<u32>,
    pub(in crate::game) operation_sequence: Option<u16>,
    pub(in crate::game) deadline_elapsed_seconds: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::game) struct ClientAuthorityView {
    pub(in crate::game) segment: u64,
    pub(in crate::game) scope: AuthorityScopeIdentity,
    pub(in crate::game) readiness: ClientAuthorityReadiness,
    pub(in crate::game) phase: ClientAuthorityPhase,
    pub(in crate::game) initial_baseline_pending: bool,
    pub(in crate::game) initial_baseline_ready_to_submit: bool,
    pub(in crate::game) initial_baseline_delivery_id: Option<u32>,
    pub(in crate::game) initial_baseline_sequence: Option<u16>,
    pub(in crate::game) initial_baseline_life_seq: Option<u32>,
    pub(in crate::game) lifecycle_intent: Option<PendingLifecycleRequest>,
    pub(in crate::game) lifecycle_request_sequence: Option<u16>,
    pub(in crate::game) lifecycle_baseline_ready_to_submit: bool,
    pub(in crate::game) lifecycle_baseline_delivery_id: Option<u32>,
    pub(in crate::game) lifecycle_baseline_sequence: Option<u16>,
    pub(in crate::game) lifecycle_baseline_life_seq: Option<u32>,
    pub(in crate::game) respawn_at_tick: Option<u32>,
    pub(in crate::game) respawn_remaining_seconds: f32,
    pub(in crate::game) correction_proposal: Option<MovementCorrection>,
    pub(in crate::game) correction_application: Option<MovementCorrectionApplied>,
    pub(in crate::game) correction_delivery_id: Option<u32>,
    pub(in crate::game) correction_retained_actions: usize,
    pub(in crate::game) correction_actions_dropped: u32,
    pub(in crate::game) correction_terminal_outcomes: usize,
    pub(in crate::game) correction_receipts_seen: u32,
    pub(in crate::game) correction_receipts_matched: u32,
    pub(in crate::game) correction_receipts_unmatched: u32,
    pub(in crate::game) combat_view_retained_actions: usize,
    pub(in crate::game) combat_view_actions_dropped: u32,
    pub(in crate::game) deferred_authority_actions: usize,
    pub(in crate::game) deferred_authority_packets: usize,
    pub(in crate::game) deferred_authority_packet_bytes: usize,
    /// Largest staged queue any single deferral attempt reached, including the
    /// attempt the capacity backstop rejected. `fail_deferred_actions` empties
    /// the live queue, so without the peak a capture cannot tell which of the
    /// three caps (`events`, `packets`, `bytes`) tripped.
    pub(in crate::game) deferred_action_peak_events: usize,
    pub(in crate::game) deferred_action_peak_packets: usize,
    pub(in crate::game) deferred_action_peak_bytes: usize,
    pub(in crate::game) deferred_authority_action_overflows: u32,
    pub(in crate::game) deferred_warp_pickup_acks: usize,
    pub(in crate::game) deferred_warp_pickup_ack_overflows: u32,
    pub(in crate::game) application_barrier_overflows: u32,
    pub(in crate::game) application_barriers: usize,
    pub(in crate::game) prize_effect_application_barriers: usize,
    pub(in crate::game) portal_warp_barrier: bool,
    pub(in crate::game) application_barrier_kind: Option<ClientAuthorityApplicationBarrierKind>,
    pub(in crate::game) application_barrier_delivery_id: Option<u32>,
    pub(in crate::game) application_barrier_identity: Option<u32>,
    pub(in crate::game) application_barrier_sequence: Option<u16>,
    pub(in crate::game) prize_effect_application_receipts_seen: u32,
    pub(in crate::game) prize_effect_application_receipts_matched: u32,
    pub(in crate::game) prize_effect_application_receipts_unmatched: u32,
    pub(in crate::game) transition_ledger_entries: usize,
    pub(in crate::game) last_transition: Option<ClientAuthorityLedgerEntry>,
    /// The most recent ledger record that actually changed the phase. Ordinary
    /// frames keep appending records whose `prior` equals `resulting` - the
    /// `Deadline`/`Observed` frame tick and duplicate private-lifecycle
    /// receipts - so `last_transition` reports the latest routine frame within
    /// one tick of any real event and cannot name what ended the phase.
    /// Retained captures need the phase-changing record instead.
    pub(in crate::game) last_decisive_transition: Option<ClientAuthorityLedgerEntry>,
    pub(in crate::game) transition_kind: Option<ClientAuthorityTransitionKind>,
    pub(in crate::game) transition_pending: bool,
    pub(in crate::game) deadline_elapsed_seconds: Option<f32>,
    pub(in crate::game) lifecycle_receipts_seen: u32,
    pub(in crate::game) lifecycle_receipts_matched: u32,
    pub(in crate::game) lifecycle_receipts_unmatched: u32,
}

impl ClientAuthorityView {
    /// The exact server ship ACK authorizes a neutral replacement pose.
    /// Game must install that successor's private loadout before submitting
    /// it, since the server's entry policy can exceed authored initial energy.
    /// Ordinary gameplay still requires the baseline's application receipt.
    pub(in crate::game) fn neutral_ship_baseline_ready(self) -> bool {
        self.lifecycle_baseline_ready_to_submit
            && self.transition_kind == Some(ClientAuthorityTransitionKind::Ship)
            && self.scope.is_complete_live()
            && self.readiness.initial_baseline_ready()
    }

    pub(in crate::game) fn observer_scope(self) -> Option<AuthorityScopeIdentity> {
        (self.phase == ClientAuthorityPhase::Spectating
            && self.scope.server_session_id.is_some()
            && self.scope.arena_stream_id.is_some()
            && self.scope.player_id.is_some()
            && self.scope.life_seq.is_some()
            && self.scope.alive == Some(false)
            && self.readiness.initial_baseline_ready())
        .then_some(self.scope)
    }

    pub(in crate::game) fn live_scope(self) -> Option<AuthorityScopeIdentity> {
        (self.phase == ClientAuthorityPhase::Live
            && self.scope.is_complete_live()
            && self.readiness.initial_baseline_ready())
        .then_some(self.scope)
    }

    pub(in crate::game) fn gameplay_scope(self) -> Option<AuthorityScopeIdentity> {
        (self.application_barriers == 0)
            .then(|| self.live_scope())
            .flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::game) enum PrizeEffectReceiptOutcome {
    Applied(ClientPrizeEffectApplication),
    Rebased,
    Cancelled(ClientPrizeEffectApplication),
    Duplicate,
    Rejected(ClientAuthorityRejection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum PortalWarpReceiptOutcome {
    Applied,
    Rejected,
    Duplicate,
    RejectedAuthority(ClientAuthorityRejection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum InitialReceiptOutcome {
    Applied,
    Duplicate,
    NotInitial,
    Rejected(ClientAuthorityRejection),
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::game) enum LifecycleReceiptOutcome {
    Applied {
        kind: ClientAuthorityTransitionKind,
        accepted_baseline: Option<ClientPositionUpdate>,
    },
    Duplicate,
    NotLifecycle,
    Rejected(ClientAuthorityRejection),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::game) enum CorrectionReceiptOutcome {
    Applied {
        receipt: MovementCorrectionAccepted,
        proposal_reason: u8,
        repel_window: Option<(u16, u16, u32)>,
    },
    Cancelled,
    Duplicate,
    Rejected(ClientAuthorityRejection),
}

#[derive(Clone, Copy, Debug)]
struct ClientQualityHold {
    hold_id: u64,
    life_seq: u32,
    deadline_seconds: f64,
}

#[derive(Debug)]
pub(in crate::game) struct ClientAuthoritySequencer {
    segment: u64,
    scope: AuthorityScopeIdentity,
    readiness: ClientAuthorityReadiness,
    phase: ClientAuthorityPhase,
    initial_spectator: bool,
    quality_hold: Option<ClientQualityHold>,
    last_quality_hold_id: u64,
    pending: Option<PendingTransition>,
    closed: VecDeque<ClosedTransition>,
    ledger: VecDeque<ClientAuthorityLedgerEntry>,
    /// The last ledger record that changed the phase. The ledger is a 256-entry
    /// ring and every frame appends to it, so at 60 Hz it only covers a few
    /// seconds and a phase flip is evicted long before any capture reads it.
    last_phase_change: Option<ClientAuthorityLedgerEntry>,
    lifecycle_receipts_seen: u32,
    lifecycle_receipts_matched: u32,
    lifecycle_receipts_unmatched: u32,
    correction_terminals: VecDeque<MovementCorrectionAccepted>,
    cancelled_corrections: VecDeque<MovementCorrection>,
    released_correction_actions: VecDeque<SemanticInputAction>,
    correction_actions_dropped: u32,
    correction_receipts_seen: u32,
    correction_receipts_matched: u32,
    correction_receipts_unmatched: u32,
    deferred_combat_view_actions: VecDeque<DeferredCombatViewAction>,
    combat_view_actions_dropped: u32,
    deferred_actions: ClientProtocolActions,
    deferred_action_packet_bytes: usize,
    deferred_action_peak_events: usize,
    deferred_action_peak_packets: usize,
    deferred_action_peak_bytes: usize,
    deferred_action_overflows: u32,
    deferred_warp_pickup_acks: VecDeque<ScopedPrizePickupAck>,
    deferred_warp_pickup_ack_overflows: u32,
    application_barrier_overflows: u32,
    prize_effect_applications: VecDeque<ClientPrizeEffectApplication>,
    prize_effect_terminals: VecDeque<PrizeEffectAccepted>,
    prize_effect_application_receipts_seen: u32,
    prize_effect_application_receipts_matched: u32,
    prize_effect_application_receipts_unmatched: u32,
    portal_warp: Option<ClientPortalWarp>,
    portal_warp_terminals: VecDeque<PortalWarpAccepted>,
}

impl Default for ClientAuthoritySequencer {
    fn default() -> Self {
        Self {
            segment: 0,
            scope: AuthorityScopeIdentity::default(),
            readiness: ClientAuthorityReadiness::default(),
            phase: ClientAuthorityPhase::Disconnected,
            initial_spectator: false,
            quality_hold: None,
            last_quality_hold_id: 0,
            pending: None,
            closed: VecDeque::new(),
            ledger: VecDeque::new(),
            last_phase_change: None,
            lifecycle_receipts_seen: 0,
            lifecycle_receipts_matched: 0,
            lifecycle_receipts_unmatched: 0,
            correction_terminals: VecDeque::new(),
            cancelled_corrections: VecDeque::new(),
            released_correction_actions: VecDeque::new(),
            correction_actions_dropped: 0,
            correction_receipts_seen: 0,
            correction_receipts_matched: 0,
            correction_receipts_unmatched: 0,
            deferred_combat_view_actions: VecDeque::new(),
            combat_view_actions_dropped: 0,
            deferred_actions: ClientProtocolActions::default(),
            deferred_action_packet_bytes: 0,
            deferred_action_overflows: 0,
            deferred_warp_pickup_acks: VecDeque::new(),
            deferred_warp_pickup_ack_overflows: 0,
            deferred_action_peak_events: 0,
            deferred_action_peak_packets: 0,
            deferred_action_peak_bytes: 0,
            application_barrier_overflows: 0,
            prize_effect_applications: VecDeque::new(),
            prize_effect_terminals: VecDeque::new(),
            prize_effect_application_receipts_seen: 0,
            prize_effect_application_receipts_matched: 0,
            prize_effect_application_receipts_unmatched: 0,
            portal_warp: None,
            portal_warp_terminals: VecDeque::new(),
        }
    }
}

impl ClientAuthoritySequencer {
    pub(in crate::game) fn view(&self) -> ClientAuthorityView {
        let (initial_baseline_pending, initial) = match self.pending.as_ref() {
            Some(PendingTransition::InitialJoin(initial)) => (true, Some(*initial)),
            _ => (false, None),
        };
        let operation = initial.and_then(|initial| initial.operation);
        let lifecycle = match self.pending.as_ref() {
            Some(PendingTransition::Lifecycle(lifecycle)) => Some(lifecycle),
            _ => None,
        };
        let lifecycle_operation = lifecycle.and_then(|lifecycle| lifecycle.operation);
        let respawn = match self.pending.as_ref() {
            Some(PendingTransition::Respawn(respawn)) => Some(*respawn),
            _ => None,
        };
        let correction = match self.pending.as_ref() {
            Some(PendingTransition::Correction(correction)) => Some(correction),
            _ => None,
        };
        let baseline_operation =
            lifecycle_operation.or_else(|| respawn.and_then(|respawn| respawn.operation));
        let baseline_life_seq = lifecycle
            .filter(|lifecycle| lifecycle.acknowledged)
            .and_then(|lifecycle| lifecycle.target_life_seq)
            .or_else(|| respawn.map(|respawn| respawn.target_life_seq));
        let transition_kind = match self.pending.as_ref() {
            Some(PendingTransition::InitialJoin(_)) => {
                Some(ClientAuthorityTransitionKind::InitialJoin)
            }
            Some(PendingTransition::Lifecycle(lifecycle)) => Some(lifecycle.kind()),
            Some(PendingTransition::Respawn(_)) => Some(ClientAuthorityTransitionKind::Respawn),
            Some(PendingTransition::Correction(_)) => {
                Some(ClientAuthorityTransitionKind::Correction)
            }
            None => None,
        };
        let prize_application = self.prize_effect_applications.front().copied();
        let prize_operation = prize_application.and_then(|pending| pending.operation);
        let portal_warp = self.portal_warp;
        let application_barrier_kind = if prize_application.is_some() {
            Some(ClientAuthorityApplicationBarrierKind::PrizeEffect)
        } else if portal_warp.is_some() {
            Some(ClientAuthorityApplicationBarrierKind::PortalWarp)
        } else {
            None
        };
        let application_barrier_operation =
            prize_operation.or_else(|| portal_warp.map(|pending| pending.operation));
        let application_barrier_identity = prize_application
            .map(|pending| pending.application.collectible_id)
            .or_else(|| portal_warp.map(|pending| pending.request.action_id));
        let application_barrier_sequence = prize_application
            .map(|pending| pending.application.application_sequence)
            .or_else(|| portal_warp.map(|pending| pending.request.position_sequence));
        ClientAuthorityView {
            segment: self.segment,
            scope: self.scope,
            readiness: self.readiness,
            phase: self.phase,
            initial_baseline_pending,
            initial_baseline_ready_to_submit: initial.is_some_and(|initial| {
                initial.life_seq.is_some()
                    && initial.operation.is_none()
                    && self.readiness.initial_baseline_ready()
            }),
            initial_baseline_delivery_id: operation.map(|operation| operation.delivery_id),
            initial_baseline_sequence: operation.map(|operation| operation.sequence),
            initial_baseline_life_seq: initial.and_then(|initial| initial.life_seq),
            lifecycle_intent: lifecycle.map(|lifecycle| lifecycle.intent),
            lifecycle_request_sequence: lifecycle.and_then(|lifecycle| lifecycle.request_sequence),
            lifecycle_baseline_ready_to_submit: lifecycle.is_some_and(|lifecycle| {
                lifecycle.acknowledged
                    && lifecycle.target_life_seq.is_some()
                    && lifecycle.operation.is_none()
                    && match lifecycle.intent {
                        PendingLifecycleRequest::Ship(_) => true,
                        PendingLifecycleRequest::Frequency(_) => lifecycle.baseline.is_some(),
                    }
            }) || respawn
                .is_some_and(|respawn| respawn.baseline_ready && respawn.operation.is_none()),
            lifecycle_baseline_delivery_id: baseline_operation
                .map(|operation| operation.delivery_id),
            lifecycle_baseline_sequence: baseline_operation.map(|operation| operation.sequence),
            lifecycle_baseline_life_seq: baseline_life_seq,
            respawn_at_tick: respawn.and_then(|respawn| respawn.respawn_at_tick),
            respawn_remaining_seconds: respawn
                .map(|respawn| respawn.remaining_seconds.max(0.0))
                .unwrap_or(0.0),
            correction_proposal: correction.map(|correction| correction.proposal),
            correction_application: correction.map(|correction| correction.application),
            correction_delivery_id: correction
                .and_then(|correction| correction.operation)
                .map(|operation| operation.delivery_id),
            correction_retained_actions: correction
                .map(|correction| correction.retained_actions.len())
                .unwrap_or(0)
                .saturating_add(self.released_correction_actions.len()),
            correction_actions_dropped: self.correction_actions_dropped,
            correction_terminal_outcomes: self.correction_terminals.len(),
            correction_receipts_seen: self.correction_receipts_seen,
            correction_receipts_matched: self.correction_receipts_matched,
            correction_receipts_unmatched: self.correction_receipts_unmatched,
            combat_view_retained_actions: self.deferred_combat_view_actions.len(),
            combat_view_actions_dropped: self.combat_view_actions_dropped,
            deferred_authority_actions: self.deferred_actions.events.len(),
            deferred_authority_packets: self.deferred_actions.outbound_packets.len(),
            deferred_authority_packet_bytes: self.deferred_action_packet_bytes,
            deferred_action_peak_events: self.deferred_action_peak_events,
            deferred_action_peak_packets: self.deferred_action_peak_packets,
            deferred_action_peak_bytes: self.deferred_action_peak_bytes,
            deferred_authority_action_overflows: self.deferred_action_overflows,
            deferred_warp_pickup_acks: self.deferred_warp_pickup_acks.len(),
            deferred_warp_pickup_ack_overflows: self.deferred_warp_pickup_ack_overflows,
            application_barrier_overflows: self.application_barrier_overflows,
            application_barriers: self.prize_effect_applications.len()
                + usize::from(self.portal_warp.is_some()),
            prize_effect_application_barriers: self.prize_effect_applications.len(),
            portal_warp_barrier: self.portal_warp.is_some(),
            application_barrier_kind,
            application_barrier_delivery_id: application_barrier_operation
                .map(|operation| operation.delivery_id),
            application_barrier_identity,
            application_barrier_sequence,
            prize_effect_application_receipts_seen: self.prize_effect_application_receipts_seen,
            prize_effect_application_receipts_matched: self
                .prize_effect_application_receipts_matched,
            prize_effect_application_receipts_unmatched: self
                .prize_effect_application_receipts_unmatched,
            transition_ledger_entries: self.ledger.len(),
            last_transition: self.ledger.back().copied(),
            last_decisive_transition: self.last_phase_change,
            transition_kind,
            transition_pending: self.pending.is_some(),
            deadline_elapsed_seconds: operation
                .map(|operation| operation.elapsed_seconds)
                .or_else(|| {
                    lifecycle
                        .filter(|lifecycle| lifecycle.request_sequence.is_some())
                        .map(|lifecycle| lifecycle.elapsed_seconds)
                })
                .or_else(|| {
                    respawn
                        .and_then(|respawn| respawn.operation)
                        .map(|operation| operation.elapsed_seconds)
                })
                .or_else(|| {
                    correction
                        .and_then(|correction| correction.operation)
                        .map(|operation| operation.elapsed_seconds)
                }),
            lifecycle_receipts_seen: self.lifecycle_receipts_seen,
            lifecycle_receipts_matched: self.lifecycle_receipts_matched,
            lifecycle_receipts_unmatched: self.lifecycle_receipts_unmatched,
        }
    }

    pub(in crate::game) fn pending_prize_effect_application(
        &self,
    ) -> Option<ClientPrizeEffectApplication> {
        self.prize_effect_applications.front().copied()
    }

    pub(in crate::game) fn pending_prize_engine_shutdowns(
        &self,
    ) -> impl Iterator<Item = (u32, u32, Option<u32>)> + '_ {
        self.prize_effect_applications.iter().filter_map(|pending| {
            pending.engine_shutdown_until_tick.map(|until_tick| {
                (
                    pending.application.life_seq,
                    pending.application.prize_state_revision,
                    Some(until_tick),
                )
            })
        })
    }

    pub(in crate::game) fn arm_prize_effect_application(
        &mut self,
        application: PrizeEffectApplied,
        engine_shutdown_until_tick: Option<u32>,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let scope_matches = self.view().live_scope().is_some_and(|scope| {
            scope.server_session_id == Some(application.server_session_id)
                && scope.player_id == Some(application.player_id)
                && scope.life_seq == Some(application.life_seq)
                && scope.alive == Some(true)
        });
        let disposition = if !scope_matches {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
        } else if self
            .prize_effect_terminals
            .iter()
            .any(|terminal| prize_effect_acceptance_identity_matches(application, *terminal))
        {
            ClientAuthorityDisposition::Duplicate
        } else if self
            .prize_effect_applications
            .iter()
            .any(|pending| pending.application == application)
        {
            ClientAuthorityDisposition::Duplicate
        } else if self.prize_effect_applications.len() == MAX_PRIZE_EFFECT_APPLICATION_BARRIERS {
            self.fail_application_barriers(ClientAuthorityRejection::CapacityExceeded);
            return ClientAuthorityDisposition::Rejected(
                ClientAuthorityRejection::CapacityExceeded,
            );
        } else {
            self.prize_effect_applications
                .push_back(ClientPrizeEffectApplication {
                    application,
                    engine_shutdown_until_tick,
                    operation: None,
                });
            ClientAuthorityDisposition::Applied
        };
        self.record_operation(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::ApplicationBarrierArmed,
            prior,
            disposition,
            None,
            Some(application.collectible_id),
            Some(application.application_sequence),
        );
        disposition
    }

    pub(in crate::game) fn record_prize_effect_application_submitted(
        &mut self,
        application: PrizeEffectApplied,
        delivery_id: u32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.prize_effect_applications.front_mut() {
            Some(pending) if pending.application == application && pending.operation.is_none() => {
                pending.operation = Some(DurableOperation {
                    delivery_id,
                    sequence: application.application_sequence,
                    elapsed_seconds: 0.0,
                });
                ClientAuthorityDisposition::Applied
            }
            Some(pending)
                if pending.application == application
                    && pending
                        .operation
                        .is_some_and(|operation| operation.delivery_id == delivery_id) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            Some(_) => {
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::DeliveryMismatch)
            }
            None => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record_operation(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::ApplicationBarrierSubmitted,
            prior,
            disposition,
            Some(delivery_id),
            Some(application.collectible_id),
            Some(application.application_sequence),
        );
        disposition
    }

    pub(in crate::game) fn observe_prize_effect_terminal(
        &mut self,
        receipt: PrizeEffectAccepted,
    ) -> PrizeEffectReceiptOutcome {
        let prior = self.phase;
        self.prize_effect_application_receipts_seen =
            self.prize_effect_application_receipts_seen.wrapping_add(1);
        let scope_matches = self.view().live_scope().is_some_and(|scope| {
            scope.server_session_id == Some(receipt.server_session_id)
                && scope.player_id == Some(receipt.player_id)
                && scope.life_seq == Some(receipt.life_seq)
                && scope.alive == Some(true)
        });
        let outcome = if !scope_matches {
            PrizeEffectReceiptOutcome::Rejected(ClientAuthorityRejection::ScopeMismatch)
        } else if self
            .prize_effect_terminals
            .iter()
            .any(|terminal| *terminal == receipt)
        {
            PrizeEffectReceiptOutcome::Duplicate
        } else {
            let Some(pending) = self.prize_effect_applications.front().copied() else {
                self.prize_effect_application_receipts_unmatched = self
                    .prize_effect_application_receipts_unmatched
                    .wrapping_add(1);
                self.record_operation(
                    ClientAuthoritySource::Reliable,
                    ClientAuthorityFactKind::ApplicationBarrierTerminal,
                    prior,
                    ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
                    None,
                    Some(receipt.collectible_id),
                    Some(receipt.requested_application_sequence),
                );
                return PrizeEffectReceiptOutcome::Rejected(
                    ClientAuthorityRejection::ImpossiblePhase,
                );
            };
            if !prize_effect_acceptance_identity_matches(pending.application, receipt) {
                PrizeEffectReceiptOutcome::Rejected(ClientAuthorityRejection::ScopeMismatch)
            } else {
                match receipt.outcome {
                    PrizeEffectAccepted::OUTCOME_APPLIED
                        if prize_effect_acceptance_matches(pending.application, receipt) =>
                    {
                        self.prize_effect_applications.pop_front();
                        PrizeEffectReceiptOutcome::Applied(pending)
                    }
                    PrizeEffectAccepted::OUTCOME_REBASE
                        if prize_effect_rebase_moves_forward(pending.application, receipt) =>
                    {
                        let head = self
                            .prize_effect_applications
                            .front_mut()
                            .expect("matching prize application head exists");
                        head.application.application_sequence = receipt.application_sequence;
                        head.operation = None;
                        PrizeEffectReceiptOutcome::Rebased
                    }
                    PrizeEffectAccepted::OUTCOME_CANCELLED => {
                        self.prize_effect_applications.pop_front();
                        PrizeEffectReceiptOutcome::Cancelled(pending)
                    }
                    _ => PrizeEffectReceiptOutcome::Rejected(
                        ClientAuthorityRejection::AdmissionMismatch,
                    ),
                }
            }
        };
        let disposition = match outcome {
            PrizeEffectReceiptOutcome::Applied(_)
            | PrizeEffectReceiptOutcome::Rebased
            | PrizeEffectReceiptOutcome::Cancelled(_) => {
                self.prize_effect_application_receipts_matched = self
                    .prize_effect_application_receipts_matched
                    .wrapping_add(1);
                if self.prize_effect_terminals.len() == MAX_PRIZE_EFFECT_TERMINALS {
                    self.prize_effect_terminals.pop_front();
                }
                self.prize_effect_terminals.push_back(receipt);
                ClientAuthorityDisposition::Applied
            }
            PrizeEffectReceiptOutcome::Duplicate => ClientAuthorityDisposition::Duplicate,
            PrizeEffectReceiptOutcome::Rejected(reason) => {
                self.prize_effect_application_receipts_unmatched = self
                    .prize_effect_application_receipts_unmatched
                    .wrapping_add(1);
                ClientAuthorityDisposition::Rejected(reason)
            }
        };
        self.record_operation(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::ApplicationBarrierTerminal,
            prior,
            disposition,
            None,
            Some(receipt.collectible_id),
            Some(receipt.requested_application_sequence),
        );
        outcome
    }

    pub(in crate::game) fn clear_prize_effect_application_barriers(&mut self) {
        self.retire_prize_effect_application_barriers();
    }

    pub(in crate::game) fn arm_portal_warp(
        &mut self,
        request: PortalWarpRequest,
        delivery_id: u32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let scope_matches = self.view().live_scope().is_some_and(|scope| {
            scope.server_session_id == Some(request.server_session_id)
                && scope.arena_stream_id == Some(request.arena_stream_id)
                && scope.player_id == Some(request.player_id)
                && scope.life_seq == Some(request.life_seq)
                && scope.alive == Some(true)
        });
        let disposition = if !scope_matches {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
        } else if self
            .portal_warp_terminals
            .iter()
            .any(|terminal| portal_warp_acceptance_matches(request, *terminal))
        {
            ClientAuthorityDisposition::Duplicate
        } else if self
            .portal_warp
            .is_some_and(|pending| pending.request == request)
        {
            ClientAuthorityDisposition::Duplicate
        } else if self.portal_warp.is_some() {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        } else {
            self.portal_warp = Some(ClientPortalWarp {
                request,
                operation: DurableOperation {
                    delivery_id,
                    sequence: request.position_sequence,
                    elapsed_seconds: 0.0,
                },
            });
            ClientAuthorityDisposition::Applied
        };
        self.record_operation(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::ApplicationBarrierSubmitted,
            prior,
            disposition,
            Some(delivery_id),
            Some(request.action_id),
            Some(request.position_sequence),
        );
        disposition
    }

    pub(in crate::game) fn pending_portal_warp_request(&self) -> Option<PortalWarpRequest> {
        self.portal_warp.map(|pending| pending.request)
    }

    pub(in crate::game) fn observe_portal_warp_terminal(
        &mut self,
        receipt: PortalWarpAccepted,
    ) -> PortalWarpReceiptOutcome {
        let prior = self.phase;
        let scope_matches = self.view().live_scope().is_some_and(|scope| {
            scope.server_session_id == Some(receipt.server_session_id)
                && scope.arena_stream_id == Some(receipt.arena_stream_id)
                && scope.player_id == Some(receipt.player_id)
                && scope.life_seq == Some(receipt.life_seq)
                && scope.alive == Some(true)
        });
        let outcome = if !scope_matches {
            PortalWarpReceiptOutcome::RejectedAuthority(ClientAuthorityRejection::ScopeMismatch)
        } else if self
            .portal_warp_terminals
            .iter()
            .any(|terminal| *terminal == receipt)
        {
            PortalWarpReceiptOutcome::Duplicate
        } else if self
            .portal_warp
            .is_some_and(|pending| portal_warp_acceptance_matches(pending.request, receipt))
        {
            self.portal_warp = None;
            if self.portal_warp_terminals.len() == MAX_PRIZE_EFFECT_TERMINALS {
                self.portal_warp_terminals.pop_front();
            }
            self.portal_warp_terminals.push_back(receipt);
            if receipt.outcome == PortalWarpAccepted::OUTCOME_APPLIED {
                PortalWarpReceiptOutcome::Applied
            } else {
                PortalWarpReceiptOutcome::Rejected
            }
        } else {
            PortalWarpReceiptOutcome::RejectedAuthority(ClientAuthorityRejection::AdmissionMismatch)
        };
        let disposition = match outcome {
            PortalWarpReceiptOutcome::Applied | PortalWarpReceiptOutcome::Rejected => {
                ClientAuthorityDisposition::Applied
            }
            PortalWarpReceiptOutcome::Duplicate => ClientAuthorityDisposition::Duplicate,
            PortalWarpReceiptOutcome::RejectedAuthority(reason) => {
                ClientAuthorityDisposition::Rejected(reason)
            }
        };
        self.record_operation(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::ApplicationBarrierTerminal,
            prior,
            disposition,
            None,
            Some(receipt.action_id),
            Some(receipt.position_sequence),
        );
        outcome
    }

    pub(in crate::game) fn clear_portal_warp_barrier(&mut self) {
        self.retire_portal_warp_barrier();
    }

    /// Retain one immutable post-gameplay action batch behind the sole
    /// sequencer-owned correction/lifecycle scope. Admission vectors were
    /// computed before any action in the poll mutated state and remain aligned
    /// through this bounded append.
    pub(in crate::game) fn defer_authority_actions(
        &mut self,
        actions: ClientProtocolActions,
    ) -> Result<(), ClientAuthorityRejection> {
        let incoming_events = actions.events.len();
        let incoming_packets = actions.outbound_packets.len();
        let incoming_packet_bytes = actions
            .outbound_packets
            .iter()
            .try_fold(0usize, |total, packet| total.checked_add(packet.len()));
        let admissions_aligned =
            incoming_events == 0 || actions.authority_admission.len() == incoming_events;
        if !admissions_aligned {
            self.fail_deferred_actions(
                ClientAuthorityFactKind::DeferredActionsRejected,
                ClientAuthorityRejection::AdmissionMismatch,
                false,
            );
            return Err(ClientAuthorityRejection::AdmissionMismatch);
        }
        let attempted_events = self
            .deferred_actions
            .events
            .len()
            .saturating_add(incoming_events);
        let attempted_packets = self
            .deferred_actions
            .outbound_packets
            .len()
            .saturating_add(incoming_packets);
        let attempted_bytes = incoming_packet_bytes
            .and_then(|bytes| self.deferred_action_packet_bytes.checked_add(bytes));
        self.deferred_action_peak_events = self.deferred_action_peak_events.max(attempted_events);
        self.deferred_action_peak_packets =
            self.deferred_action_peak_packets.max(attempted_packets);
        if let Some(bytes) = attempted_bytes {
            self.deferred_action_peak_bytes = self.deferred_action_peak_bytes.max(bytes);
        }
        let within_capacity = attempted_events <= MAX_DEFERRED_AUTHORITY_ACTIONS
            && attempted_packets <= MAX_DEFERRED_AUTHORITY_PACKETS
            && attempted_bytes.is_some_and(|bytes| bytes <= MAX_DEFERRED_AUTHORITY_PACKET_BYTES);
        if !within_capacity {
            self.fail_deferred_actions(
                ClientAuthorityFactKind::DeferredActionsOverflow,
                ClientAuthorityRejection::CapacityExceeded,
                false,
            );
            return Err(ClientAuthorityRejection::CapacityExceeded);
        }
        self.deferred_action_packet_bytes += incoming_packet_bytes.unwrap_or(0);
        self.deferred_actions.append(actions);
        Ok(())
    }

    pub(in crate::game) fn take_deferred_authority_actions(&mut self) -> ClientProtocolActions {
        self.deferred_action_packet_bytes = 0;
        std::mem::take(&mut self.deferred_actions)
    }

    /// Replace a previously drained queue with the exact still-blocked
    /// suffix. The same atomic capacity check prevents a caller from restoring
    /// an oversized queue after a concurrent poll.
    pub(in crate::game) fn restore_deferred_authority_actions(
        &mut self,
        actions: ClientProtocolActions,
    ) -> Result<(), ClientAuthorityRejection> {
        debug_assert!(self.deferred_actions.events.is_empty());
        self.defer_authority_actions(actions)
    }

    pub(in crate::game) fn has_deferred_warp_pickup_ack(&self, collectible_id: u32) -> bool {
        self.deferred_warp_pickup_acks
            .iter()
            .any(|pending| pending.ack.collectible_id == collectible_id)
    }

    pub(in crate::game) fn defer_warp_pickup_ack(
        &mut self,
        scoped: ScopedPrizePickupAck,
    ) -> ClientAuthorityDisposition {
        let ack_scope = AuthorityScopeIdentity {
            server_session_id: scoped.server_session_id,
            arena_stream_id: scoped.arena_stream_id,
            player_id: scoped.player_id,
            life_seq: Some(scoped.ack.life_seq),
            alive: Some(true),
        };
        if self.view().live_scope() != Some(ack_scope)
            || scoped.ack.server_session_id != ack_scope.server_session_id.unwrap_or(0)
            || scoped.ack.arena_stream_id != ack_scope.arena_stream_id.unwrap_or(0)
            || Some(scoped.ack.player_id) != ack_scope.player_id
        {
            return ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch);
        }
        if self.has_deferred_warp_pickup_ack(scoped.ack.collectible_id) {
            return ClientAuthorityDisposition::Duplicate;
        }
        if self.deferred_warp_pickup_acks.len() == MAX_DEFERRED_WARP_PICKUP_ACKS {
            self.fail_deferred_actions(
                ClientAuthorityFactKind::ApplicationBarrierOverflow,
                ClientAuthorityRejection::CapacityExceeded,
                true,
            );
            return ClientAuthorityDisposition::Rejected(
                ClientAuthorityRejection::CapacityExceeded,
            );
        }
        self.deferred_warp_pickup_acks.push_back(scoped);
        ClientAuthorityDisposition::Applied
    }

    pub(in crate::game) fn pop_deferred_warp_pickup_ack(&mut self) -> Option<ScopedPrizePickupAck> {
        self.deferred_warp_pickup_acks.pop_front()
    }

    pub(in crate::game) fn clear_deferred_warp_pickup_acks(&mut self) {
        self.deferred_warp_pickup_acks.clear();
    }

    fn fail_deferred_actions(
        &mut self,
        fact: ClientAuthorityFactKind,
        reason: ClientAuthorityRejection,
        warp_barrier: bool,
    ) {
        let prior = self.phase;
        if warp_barrier {
            self.deferred_warp_pickup_ack_overflows =
                self.deferred_warp_pickup_ack_overflows.wrapping_add(1);
        } else {
            self.deferred_action_overflows = self.deferred_action_overflows.wrapping_add(1);
        }
        self.cancel_pending();
        self.deferred_actions = ClientProtocolActions::default();
        self.deferred_action_packet_bytes = 0;
        self.deferred_warp_pickup_acks.clear();
        self.phase = ClientAuthorityPhase::Failed;
        self.record(
            ClientAuthoritySource::Local,
            fact,
            prior,
            ClientAuthorityDisposition::Rejected(reason),
            None,
        );
    }

    fn fail_application_barriers(&mut self, reason: ClientAuthorityRejection) {
        let prior = self.phase;
        self.application_barrier_overflows = self.application_barrier_overflows.wrapping_add(1);
        self.cancel_pending();
        self.retire_life_scoped_application_barriers();
        self.phase = ClientAuthorityPhase::Failed;
        self.record_operation(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::ApplicationBarrierOverflow,
            prior,
            ClientAuthorityDisposition::Rejected(reason),
            None,
            None,
            None,
        );
    }

    fn retire_life_scoped_application_barriers(&mut self) {
        self.deferred_warp_pickup_acks.clear();
        self.retire_prize_effect_application_barriers();
        self.retire_portal_warp_barrier();
    }

    fn retire_prize_effect_application_barriers(&mut self) {
        let prior = self.phase;
        while let Some(pending) = self.prize_effect_applications.pop_front() {
            self.record_operation(
                ClientAuthoritySource::Local,
                ClientAuthorityFactKind::ApplicationBarrierCancelled,
                prior,
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::Cancelled),
                pending.operation.map(|operation| operation.delivery_id),
                Some(pending.application.collectible_id),
                Some(pending.application.application_sequence),
            );
        }
        self.prize_effect_terminals.clear();
    }

    fn retire_portal_warp_barrier(&mut self) {
        let prior = self.phase;
        if let Some(pending) = self.portal_warp.take() {
            self.record_operation(
                ClientAuthoritySource::Local,
                ClientAuthorityFactKind::ApplicationBarrierCancelled,
                prior,
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::Cancelled),
                Some(pending.operation.delivery_id),
                Some(pending.request.action_id),
                Some(pending.request.position_sequence),
            );
        }
        self.portal_warp_terminals.clear();
    }

    pub(in crate::game) fn observe_hard_replacement(
        &mut self,
        source: ClientAuthoritySource,
        segment: u64,
        replacement: Option<AuthorityScopeIdentity>,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = if segment != self.segment.wrapping_add(1) {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::SegmentMismatch)
        } else {
            self.cancel_pending();
            self.segment = segment;
            self.scope = replacement.unwrap_or_default();
            self.readiness = ClientAuthorityReadiness::default();
            self.initial_spectator = false;
            self.quality_hold = None;
            self.last_quality_hold_id = 0;
            self.pending = None;
            self.correction_terminals.clear();
            self.cancelled_corrections.clear();
            self.released_correction_actions.clear();
            self.discard_combat_view_actions();
            self.deferred_actions = ClientProtocolActions::default();
            self.deferred_action_packet_bytes = 0;
            self.retire_life_scoped_application_barriers();
            self.phase = if replacement.is_some() {
                ClientAuthorityPhase::Bootstrapping
            } else {
                ClientAuthorityPhase::Disconnected
            };
            ClientAuthorityDisposition::Replacement
        };
        self.record(
            source,
            ClientAuthorityFactKind::HardReplacement,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn observe_joined_arena(
        &mut self,
        arena_epoch: u32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = if self.scope.server_session_id.is_none()
            || self.scope.arena_stream_id.is_none()
            || self.scope.player_id.is_none()
        {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
        } else if self.readiness.joined
            && self.readiness.arena_epoch == Some(arena_epoch)
            && matches!(
                self.pending.as_ref(),
                Some(PendingTransition::InitialJoin(_))
            )
        {
            ClientAuthorityDisposition::Duplicate
        } else if self.phase != ClientAuthorityPhase::Bootstrapping {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        } else {
            self.readiness.joined = true;
            self.readiness.arena_epoch = Some(arena_epoch);
            self.pending = Some(PendingTransition::InitialJoin(InitialJoinTransition {
                life_seq: None,
                operation: None,
                pre_submission_seconds: 0.0,
            }));
            self.phase = ClientAuthorityPhase::AwaitingReadiness;
            ClientAuthorityDisposition::Applied
        };
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::JoinedArena,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn observe_settings_ready(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::SettingsReady,
            |readiness| &mut readiness.settings,
            true,
        )
    }

    pub(in crate::game) fn observe_security_ready(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::SecurityReady,
            |readiness| &mut readiness.security,
            true,
        )
    }

    pub(in crate::game) fn observe_security_pending(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::SecurityPending,
            |readiness| &mut readiness.security,
            false,
        )
    }

    pub(in crate::game) fn observe_map_pending(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::MapPending,
            |readiness| &mut readiness.map,
            false,
        )
    }

    pub(in crate::game) fn observe_map_ready(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::MapReady,
            |readiness| &mut readiness.map,
            true,
        )
    }

    pub(in crate::game) fn observe_files_ready(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::FilesReady,
            |readiness| &mut readiness.files,
            true,
        )
    }

    pub(in crate::game) fn observe_files_pending(&mut self) -> ClientAuthorityDisposition {
        self.observe_readiness(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::FilesPending,
            |readiness| &mut readiness.files,
            false,
        )
    }

    fn observe_readiness(
        &mut self,
        source: ClientAuthoritySource,
        fact: ClientAuthorityFactKind,
        field: fn(&mut ClientAuthorityReadiness) -> &mut bool,
        value: bool,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = if !self.readiness.joined {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        } else if *field(&mut self.readiness) == value {
            ClientAuthorityDisposition::Duplicate
        } else {
            *field(&mut self.readiness) = value;
            self.refresh_initial_phase();
            ClientAuthorityDisposition::Applied
        };
        self.record(source, fact, prior, disposition, None);
        disposition
    }

    pub(in crate::game) fn observe_initial_ship(&mut self, ship: u8) {
        if matches!(self.pending, Some(PendingTransition::InitialJoin(_))) {
            self.initial_spectator = ship == SPECTATOR_SHIP;
        }
    }

    pub(in crate::game) fn observe_private_lifecycle(
        &mut self,
        source: ClientAuthoritySource,
        scope: AuthorityScopeIdentity,
        respawn_at_tick: Option<u32>,
        respawn_remaining_seconds: f32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let prior_scope = self.scope;
        let same_generation = scope.server_session_id == self.scope.server_session_id
            && scope.arena_stream_id == self.scope.arena_stream_id
            && scope.player_id == self.scope.player_id;
        let disposition = if !same_generation {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
        } else if self.initial_spectator
            && matches!(self.pending, Some(PendingTransition::InitialJoin(_)))
            && scope.life_seq.is_some_and(|life| life != 0)
            && scope.alive == Some(false)
        {
            self.scope = scope;
            self.pending = None;
            self.phase = ClientAuthorityPhase::Spectating;
            ClientAuthorityDisposition::Replacement
        } else if let Some(PendingTransition::Respawn(respawn)) = self.pending.as_mut() {
            if scope.life_seq != Some(respawn.target_life_seq) {
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::LifeMismatch)
            } else if scope.alive == Some(false) {
                // Snapshot and reliable death facts can repeat. Never extend
                // the already-observed authoritative deadline when a delayed
                // copy arrives after the local countdown has advanced.
                respawn.remaining_seconds = respawn
                    .remaining_seconds
                    .min(respawn_remaining_seconds.max(0.0));
                if respawn.respawn_at_tick.is_none() {
                    respawn.respawn_at_tick = respawn_at_tick;
                }
                ClientAuthorityDisposition::Duplicate
            } else if scope.alive == Some(true) {
                // The unordered private snapshot may reveal the server's
                // prospective live state before the ordered 0x75 application
                // receipt. Record that exact fact for admission, but keep the
                // gameplay phase closed until the durable tuple completes.
                respawn.server_alive_observed = true;
                self.scope.alive = Some(true);
                ClientAuthorityDisposition::Observed
            } else {
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::LifeMismatch)
            }
        } else if matches!(
            self.pending.as_ref(),
            Some(PendingTransition::Correction(_))
        ) && scope.alive == Some(false)
            && scope.life_seq.is_some_and(|life_seq| {
                self.scope
                    .life_seq
                    .is_some_and(|current| life_seq == current.wrapping_add(1).max(1))
            })
        {
            let target_life_seq = scope.life_seq.expect("checked successor life");
            self.cancel_pending();
            self.scope.life_seq = Some(target_life_seq);
            self.scope.alive = Some(false);
            self.pending = Some(PendingTransition::Respawn(RespawnTransition {
                target_life_seq,
                respawn_at_tick,
                remaining_seconds: respawn_remaining_seconds.max(0.0),
                baseline_ready: false,
                server_alive_observed: false,
                operation: None,
            }));
            self.phase = ClientAuthorityPhase::DeadRespawning;
            ClientAuthorityDisposition::Replacement
        } else if let Some(PendingTransition::Lifecycle(lifecycle)) = self.pending.as_ref() {
            if lifecycle.intent == PendingLifecycleRequest::Ship(SPECTATOR_SHIP)
                && scope.alive == Some(false)
                && scope.life_seq.is_some_and(|life| {
                    life == lifecycle.predecessor_life_seq.wrapping_add(1).max(1)
                })
            {
                let Some(PendingTransition::Lifecycle(lifecycle)) = self.pending.as_mut() else {
                    unreachable!()
                };
                lifecycle.target_life_seq = scope.life_seq;
                self.scope.life_seq = scope.life_seq;
                self.scope.alive = Some(false);
                ClientAuthorityDisposition::Observed
            } else if lifecycle.target_life_seq == scope.life_seq && scope.alive == Some(true) {
                ClientAuthorityDisposition::Duplicate
            } else if lifecycle.target_life_seq.is_none()
                && scope.alive == Some(true)
                && scope.life_seq.is_some_and(|life_seq| {
                    life_seq == lifecycle.predecessor_life_seq.wrapping_add(1).max(1)
                })
            {
                let target_life_seq = scope.life_seq.expect("checked successor life");
                let Some(PendingTransition::Lifecycle(lifecycle)) = self.pending.as_mut() else {
                    unreachable!()
                };
                lifecycle.target_life_seq = Some(target_life_seq);
                self.scope.life_seq = Some(target_life_seq);
                self.scope.alive = Some(true);
                ClientAuthorityDisposition::Observed
            } else if lifecycle.target_life_seq.is_none()
                && scope.alive == Some(false)
                && scope.life_seq.is_some_and(|life_seq| {
                    life_seq == lifecycle.predecessor_life_seq.wrapping_add(1).max(1)
                })
            {
                let target_life_seq = scope.life_seq.expect("checked successor life");
                self.cancel_pending();
                self.scope.life_seq = Some(target_life_seq);
                self.scope.alive = Some(false);
                self.pending = Some(PendingTransition::Respawn(RespawnTransition {
                    target_life_seq,
                    respawn_at_tick,
                    remaining_seconds: respawn_remaining_seconds.max(0.0),
                    baseline_ready: false,
                    server_alive_observed: false,
                    operation: None,
                }));
                self.phase = ClientAuthorityPhase::DeadRespawning;
                ClientAuthorityDisposition::Replacement
            } else {
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
            }
        } else if !matches!(
            self.pending.as_ref(),
            Some(PendingTransition::InitialJoin(_))
        ) {
            if self.scope.life_seq == scope.life_seq && self.scope.alive == scope.alive {
                ClientAuthorityDisposition::Duplicate
            } else if self.phase == ClientAuthorityPhase::Live
                && scope.alive == Some(false)
                && scope.life_seq.is_some_and(|life_seq| {
                    self.scope
                        .life_seq
                        .is_some_and(|current| life_seq == current.wrapping_add(1).max(1))
                })
            {
                let target_life_seq = scope.life_seq.expect("checked successor life");
                self.scope.life_seq = Some(target_life_seq);
                self.scope.alive = Some(false);
                self.pending = Some(PendingTransition::Respawn(RespawnTransition {
                    target_life_seq,
                    respawn_at_tick,
                    remaining_seconds: respawn_remaining_seconds.max(0.0),
                    baseline_ready: false,
                    server_alive_observed: false,
                    operation: None,
                }));
                self.phase = ClientAuthorityPhase::DeadRespawning;
                ClientAuthorityDisposition::Replacement
            } else {
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
            }
        } else if scope.life_seq.is_none() || scope.alive != Some(true) {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::LifeMismatch)
        } else {
            let PendingTransition::InitialJoin(initial) = self.pending.as_mut().unwrap() else {
                unreachable!()
            };
            match initial.life_seq {
                Some(life_seq) if Some(life_seq) != scope.life_seq => {
                    ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::LifeMismatch)
                }
                Some(_) => ClientAuthorityDisposition::Duplicate,
                None => {
                    initial.life_seq = scope.life_seq;
                    self.scope.life_seq = scope.life_seq;
                    self.scope.alive = scope.alive;
                    self.refresh_initial_phase();
                    ClientAuthorityDisposition::Applied
                }
            }
        };
        if self.scope != prior_scope {
            self.retire_life_scoped_application_barriers();
        }
        self.record(
            source,
            ClientAuthorityFactKind::PrivateLifecycle,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn queue_lifecycle_intent(
        &mut self,
        intent: PendingLifecycleRequest,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_ref() {
            None if self.view().live_scope().is_some()
                || (self.view().observer_scope().is_some()
                    && matches!(intent, PendingLifecycleRequest::Ship(ship) if ship < SPECTATOR_SHIP)) =>
            {
                let predecessor_life_seq = self
                    .scope
                    .life_seq
                    .expect("live scope always has a life generation");
                self.pending = Some(PendingTransition::Lifecycle(LifecycleCutoverTransition {
                    intent,
                    predecessor_life_seq,
                    request_sequence: None,
                    target_life_seq: None,
                    acknowledged: false,
                    baseline: None,
                    accepted_baseline: None,
                    operation: None,
                    elapsed_seconds: 0.0,
                }));
                self.phase = ClientAuthorityPhase::ShipFrequencyTransition;
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::Lifecycle(lifecycle)) if lifecycle.intent == intent => {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::LifecycleIntentQueued,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn record_ship_request_submitted(
        &mut self,
        ship: u8,
        position_sequence: u16,
    ) -> ClientAuthorityDisposition {
        self.record_lifecycle_request_submitted(
            PendingLifecycleRequest::Ship(ship),
            position_sequence,
            None,
        )
    }

    pub(in crate::game) fn record_frequency_request_submitted(
        &mut self,
        frequency: u16,
        position_sequence: u16,
        baseline: ClientPositionUpdate,
    ) -> ClientAuthorityDisposition {
        self.record_lifecycle_request_submitted(
            PendingLifecycleRequest::Frequency(frequency),
            position_sequence,
            Some(baseline),
        )
    }

    fn record_lifecycle_request_submitted(
        &mut self,
        intent: PendingLifecycleRequest,
        position_sequence: u16,
        baseline: Option<ClientPositionUpdate>,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == intent && lifecycle.request_sequence.is_none() =>
            {
                lifecycle.request_sequence = Some(position_sequence);
                lifecycle.baseline = baseline;
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == intent
                    && lifecycle.request_sequence == Some(position_sequence) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::LifecycleRequestSubmitted,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn observe_ship_rejection(
        &mut self,
        receipt: crate::protocol::ShipChangeRejected,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let matches = matches!(self.pending.as_ref(),
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == PendingLifecycleRequest::Ship(receipt.ship)
                    && lifecycle.predecessor_life_seq == receipt.life_seq
                    && self.scope.life_seq == Some(receipt.life_seq)
                    && self.scope.server_session_id == Some(receipt.server_session_id)
                    && self.scope.arena_stream_id == Some(receipt.arena_stream_id)
                    && self.scope.player_id == Some(receipt.player_id)
                    && lifecycle.request_sequence == Some(receipt.position_sequence)
                    && !lifecycle.acknowledged);
        let disposition = if matches {
            self.pending = None;
            self.phase = ClientAuthorityPhase::Live;
            ClientAuthorityDisposition::Applied
        } else {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        };
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::LifecycleRejected,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn quality_hold_remaining(&self, now: f64) -> Option<f64> {
        self.quality_hold
            .map(|hold| (hold.deadline_seconds - now).max(0.0))
    }

    pub(in crate::game) fn observe_quality_directive(
        &mut self,
        fact: crate::protocol::ConnectionQualityDirective,
        now: f64,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let result = self.apply_quality_directive(fact, now);
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::ConnectionQuality,
            prior,
            result,
            None,
        );
        result
    }

    pub(in crate::game) fn observe_spawn_idle_spectator(
        &mut self,
        fact: crate::protocol::SpawnIdleSpectator,
    ) -> ClientAuthorityDisposition {
        use ClientAuthorityDisposition as D;
        use ClientAuthorityRejection as R;
        let prior = self.phase;
        let scope_matches = fact.validate().is_ok()
            && self.readiness.joined
            && self.scope.server_session_id == Some(fact.server_session_id)
            && self.scope.arena_stream_id == Some(fact.arena_stream_id)
            && self.scope.player_id == Some(fact.player_id);
        let successor =
            self.scope.life_seq == Some(fact.life_seq) && self.scope.alive == Some(false);
        let result = if !scope_matches {
            D::Rejected(R::ScopeMismatch)
        } else if successor && self.phase == ClientAuthorityPhase::Spectating {
            D::Duplicate
        } else if self.scope.life_seq != Some(fact.predecessor_life_seq) && !successor {
            D::Rejected(R::LifeMismatch)
        } else {
            self.cancel_pending();
            self.retire_life_scoped_application_barriers();
            self.released_correction_actions.clear();
            self.discard_combat_view_actions();
            self.deferred_actions = ClientProtocolActions::default();
            self.deferred_action_packet_bytes = 0;
            self.scope.life_seq = Some(fact.life_seq);
            self.scope.alive = Some(false);
            self.pending = None;
            self.phase = ClientAuthorityPhase::Spectating;
            D::Replacement
        };
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::SpawnIdle,
            prior,
            result,
            None,
        );
        result
    }

    fn apply_quality_directive(
        &mut self,
        fact: crate::protocol::ConnectionQualityDirective,
        now: f64,
    ) -> ClientAuthorityDisposition {
        use crate::protocol::ConnectionQualityDirectiveKind as Kind;
        use ClientAuthorityDisposition as D;
        use ClientAuthorityRejection as R;
        if fact.validate().is_err()
            || !now.is_finite()
            || !self.readiness.joined
            || self.scope.server_session_id != Some(fact.server_session_id)
            || self.scope.arena_stream_id != Some(fact.arena_stream_id)
            || self.scope.player_id != Some(fact.player_id)
        {
            return D::Rejected(R::ScopeMismatch);
        }
        if fact.hold_id < self.last_quality_hold_id {
            return D::Rejected(R::Cancelled);
        }
        let matching_hold = self
            .quality_hold
            .is_some_and(|hold| hold.hold_id == fact.hold_id && hold.life_seq == fact.life_seq);
        // A cleared hold is a terminal even if a delayed Force is replayed.
        if fact.hold_id == self.last_quality_hold_id && !matching_hold {
            return if fact.kind == Kind::Clear {
                D::Duplicate
            } else {
                D::Rejected(R::Cancelled)
            };
        }
        if fact.kind == Kind::Force && matching_hold {
            return D::Duplicate;
        }
        let observed_successor =
            self.scope.life_seq == Some(fact.life_seq) && self.scope.alive == Some(false);
        match fact.kind {
            Kind::Force => {
                if self.scope.life_seq != Some(fact.predecessor_life_seq) && !observed_successor {
                    return D::Rejected(R::LifeMismatch);
                }
                self.cancel_pending();
                self.retire_life_scoped_application_barriers();
                self.released_correction_actions.clear();
                self.discard_combat_view_actions();
                self.deferred_actions = ClientProtocolActions::default();
                self.deferred_action_packet_bytes = 0;
                self.scope.life_seq = Some(fact.life_seq);
                self.scope.alive = Some(false);
                self.pending = None;
                self.phase = ClientAuthorityPhase::Spectating;
            }
            Kind::Hold => {
                let initial = self.initial_spectator && self.scope.life_seq.is_none();
                if !observed_successor && !initial {
                    return D::Rejected(R::LifeMismatch);
                }
                if let Some((ship, sequence)) = fact.rejected_request {
                    let matches = matches!(self.pending.as_ref(), Some(PendingTransition::Lifecycle(pending))
                        if pending.intent == PendingLifecycleRequest::Ship(ship)
                            && pending.predecessor_life_seq == fact.life_seq
                            && pending.request_sequence == Some(sequence) && !pending.acknowledged);
                    if !matches {
                        return D::Rejected(R::DeliveryMismatch);
                    }
                    self.cancel_pending();
                    self.pending = None;
                    self.phase = ClientAuthorityPhase::Spectating;
                } else if initial {
                    self.cancel_pending();
                    self.pending = None;
                    self.scope.life_seq = Some(fact.life_seq);
                    self.scope.alive = Some(false);
                    self.phase = ClientAuthorityPhase::Spectating;
                }
            }
            Kind::Clear => {
                // The unordered live snapshot can overtake the reliable Clear
                // which precedes the ship ACK. It may only clear the exact hold
                // being returned from, with an already submitted matching
                // lifecycle request and its observed immediate successor.
                let returning_successor = matches!(self.pending.as_ref(),
                    Some(PendingTransition::Lifecycle(pending))
                        if matches!(pending.intent, PendingLifecycleRequest::Ship(ship) if ship < SPECTATOR_SHIP)
                            && pending.request_sequence.is_some()
                            && pending.predecessor_life_seq == fact.life_seq
                            && pending.target_life_seq == self.scope.life_seq
                            && self.scope.life_seq == Some(fact.life_seq.wrapping_add(1).max(1))
                            && self.scope.alive == Some(true));
                if !matching_hold || (!observed_successor && !returning_successor) {
                    return D::Rejected(R::LifeMismatch);
                }
                self.quality_hold = None;
                self.last_quality_hold_id = fact.hold_id;
                return D::Applied;
            }
        }
        let deadline = now + f64::from(fact.remaining_ms) / 1000.0;
        let deadline_seconds = self
            .quality_hold
            .filter(|hold| hold.hold_id == fact.hold_id)
            .map_or(deadline, |hold| hold.deadline_seconds.min(deadline));
        self.quality_hold = Some(ClientQualityHold {
            hold_id: fact.hold_id,
            life_seq: fact.life_seq,
            deadline_seconds,
        });
        self.last_quality_hold_id = fact.hold_id;
        if fact.kind == Kind::Force {
            D::Replacement
        } else {
            D::Applied
        }
    }

    pub(in crate::game) fn observe_ship_ack(
        &mut self,
        ship: u8,
        life_seq: Option<u32>,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == PendingLifecycleRequest::Ship(ship)
                    && lifecycle.request_sequence.is_some()
                    && !lifecycle.acknowledged
                    && life_seq.is_some_and(|life_seq| {
                        life_seq != 0
                            && life_seq == lifecycle.predecessor_life_seq.wrapping_add(1).max(1)
                            && lifecycle
                                .target_life_seq
                                .is_none_or(|observed| observed == life_seq)
                    }) =>
            {
                lifecycle.target_life_seq = life_seq;
                lifecycle.acknowledged = true;
                self.scope.life_seq = life_seq;
                self.scope.alive = Some(ship < SPECTATOR_SHIP);
                ClientAuthorityDisposition::Replacement
            }
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == PendingLifecycleRequest::Ship(ship)
                    && lifecycle.acknowledged
                    && lifecycle.target_life_seq == life_seq =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        if disposition == ClientAuthorityDisposition::Replacement {
            self.retire_life_scoped_application_barriers();
            if ship == SPECTATOR_SHIP {
                self.pending = None;
                self.phase = ClientAuthorityPhase::Spectating;
            }
        }
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::LifecycleAcknowledged,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn observe_frequency_ack(
        &mut self,
        ack: FrequencyCutoverAck,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == PendingLifecycleRequest::Frequency(ack.frequency)
                    && lifecycle.request_sequence == Some(ack.position_sequence_base)
                    && !lifecycle.acknowledged
                    && ack.life_seq != 0
                    && ack.life_seq == lifecycle.predecessor_life_seq.wrapping_add(1).max(1)
                    && lifecycle
                        .target_life_seq
                        .is_none_or(|observed| observed == ack.life_seq) =>
            {
                let Some(mut baseline) = lifecycle.baseline.take() else {
                    return self.record_impossible_lifecycle_ack(prior);
                };
                baseline.direction = ack.direction;
                baseline.timestamp = 0;
                baseline.x = ack.x;
                baseline.y = ack.y;
                baseline.velocity_x = ack.velocity_x;
                baseline.velocity_y = ack.velocity_y;
                baseline.togglables = ack.togglables;
                baseline.bounty = ack.bounty;
                baseline.energy = ack.energy;
                baseline.weapon = 0;
                baseline.canonical_state = ack.canonical_state;
                baseline.extra = Some(ClientPositionExtra {
                    s2c_latency: 0,
                    flag_timer: 0,
                    items: crate::continuum_position::zubspace_position_sequence_items(
                        ack.position_sequence_base,
                    ),
                });
                lifecycle.target_life_seq = Some(ack.life_seq);
                lifecycle.acknowledged = true;
                lifecycle.baseline = Some(baseline);
                self.scope.life_seq = Some(ack.life_seq);
                self.scope.alive = Some(true);
                ClientAuthorityDisposition::Replacement
            }
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.intent == PendingLifecycleRequest::Frequency(ack.frequency)
                    && lifecycle.request_sequence == Some(ack.position_sequence_base)
                    && lifecycle.acknowledged
                    && lifecycle.target_life_seq == Some(ack.life_seq) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        if disposition == ClientAuthorityDisposition::Replacement {
            self.retire_life_scoped_application_barriers();
        }
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::LifecycleAcknowledged,
            prior,
            disposition,
            None,
        );
        disposition
    }

    fn record_impossible_lifecycle_ack(
        &mut self,
        prior: ClientAuthorityPhase,
    ) -> ClientAuthorityDisposition {
        let disposition =
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase);
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::LifecycleAcknowledged,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn lifecycle_baseline_to_submit(&self) -> Option<ClientPositionUpdate> {
        match self.pending.as_ref() {
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.acknowledged
                    && lifecycle.target_life_seq.is_some()
                    && lifecycle.operation.is_none()
                    && matches!(lifecycle.intent, PendingLifecycleRequest::Frequency(_)) =>
            {
                lifecycle.baseline.clone()
            }
            _ => None,
        }
    }

    pub(in crate::game) fn observe_movement_correction(
        &mut self,
        correction: MovementCorrection,
        next_input_sequence: u16,
        newer_boundary_pending: bool,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let scope_matches = self.scope.server_session_id == Some(correction.server_session_id)
            && self.scope.player_id == Some(correction.player_id)
            && self.scope.life_seq == Some(correction.life_seq)
            && self.scope.alive == Some(true)
            && matches!(
                correction.reason,
                MovementCorrection::REASON_SUSTAINED_DRIFT
                    | MovementCorrection::REASON_REJECTED_WEAPON_RECOIL
                    | MovementCorrection::REASON_INPUT_HISTORY_GAP
                    | MovementCorrection::REASON_BRICK_OVERLAP
                    | MovementCorrection::REASON_REPEL_WINDOW
            )
            && correction.correction_generation != 0
            && match correction.repel_window {
                Some((start, end, generation)) => {
                    let span = end.wrapping_sub(start);
                    correction.reason == MovementCorrection::REASON_REPEL_WINDOW
                        && generation != 0
                        && span > 1
                        && span < (1 << 15)
                }
                None => correction.reason != MovementCorrection::REASON_REPEL_WINDOW,
            };
        let disposition = if !scope_matches {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
        } else if self
            .correction_terminals
            .iter()
            .any(|terminal| correction_proposal_matches_terminal(correction, *terminal))
            || self
                .cancelled_corrections
                .iter()
                .any(|closed| *closed == correction)
        {
            ClientAuthorityDisposition::Duplicate
        } else if let Some(PendingTransition::Correction(active)) = self.pending.as_ref() {
            if active.proposal == correction {
                ClientAuthorityDisposition::Duplicate
            } else {
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
            }
        } else if self.view().live_scope().is_none() || self.pending.is_some() {
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        } else {
            let application = MovementCorrectionApplied {
                intent: if newer_boundary_pending {
                    MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY
                } else {
                    MovementCorrectionApplied::INTENT_APPLY
                },
                server_session_id: correction.server_session_id,
                player_id: correction.player_id,
                life_seq: correction.life_seq,
                correction_generation: correction.correction_generation,
                detected_sequence: correction.position_sequence,
                application_sequence: next_input_sequence.wrapping_sub(1),
            };
            self.pending = Some(PendingTransition::Correction(CorrectionTransition {
                proposal: correction,
                application,
                operation: None,
                retained_actions: VecDeque::new(),
            }));
            self.phase = ClientAuthorityPhase::CorrectionHeld;
            ClientAuthorityDisposition::Applied
        };
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::CorrectionProposed,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn correction_application_to_submit(
        &self,
    ) -> Option<MovementCorrectionApplied> {
        match self.pending.as_ref() {
            Some(PendingTransition::Correction(correction)) if correction.operation.is_none() => {
                Some(correction.application)
            }
            _ => None,
        }
    }

    pub(in crate::game) fn record_correction_application_submitted(
        &mut self,
        delivery_id: u32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Correction(correction)) if correction.operation.is_none() => {
                correction.operation = Some(DurableOperation {
                    delivery_id,
                    sequence: correction.application.application_sequence,
                    elapsed_seconds: 0.0,
                });
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::Correction(correction))
                if correction
                    .operation
                    .is_some_and(|operation| operation.delivery_id == delivery_id) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::CorrectionSubmitted,
            prior,
            disposition,
            Some(delivery_id),
        );
        disposition
    }

    pub(in crate::game) fn retain_correction_action(
        &mut self,
        action: SemanticInputAction,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Correction(correction))
                if correction.retained_actions.len()
                    < crate::input::MAX_PENDING_SEMANTIC_ACTIONS =>
            {
                correction.retained_actions.push_back(action);
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::Correction(_)) => {
                self.correction_actions_dropped = self.correction_actions_dropped.wrapping_add(1);
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::Cancelled)
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::CorrectionProposed,
            prior,
            disposition,
            None,
        );
        disposition
    }

    /// UI takes ownership of input; delayed weapon edges must not replay on resume.
    pub(in crate::game) fn discard_gameplay_actions_for_menu(&mut self) {
        let mut count = self.released_correction_actions.len();
        self.released_correction_actions.clear();
        if let Some(PendingTransition::Correction(correction)) = self.pending.as_mut() {
            count += correction.retained_actions.len();
            correction.retained_actions.clear();
        }
        self.discard_correction_actions(count);
        self.discard_combat_view_actions();
    }

    pub(in crate::game) fn take_released_correction_actions(
        &mut self,
        release_ready: bool,
    ) -> Vec<SemanticInputAction> {
        if !release_ready {
            return Vec::new();
        }
        self.released_correction_actions.drain(..).collect()
    }

    pub(in crate::game) fn retain_combat_view_actions(
        &mut self,
        actions: Vec<SemanticInputAction>,
    ) -> ClientAuthorityDisposition {
        let action_count = actions.len();
        if action_count == 0 {
            return ClientAuthorityDisposition::Observed;
        }
        if self.phase != ClientAuthorityPhase::Live || !self.scope.is_complete_live() {
            self.combat_view_actions_dropped = self
                .combat_view_actions_dropped
                .wrapping_add(u32::try_from(action_count).unwrap_or(u32::MAX));
            return ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase);
        }
        if self
            .deferred_combat_view_actions
            .len()
            .saturating_add(action_count)
            > crate::input::MAX_PENDING_SEMANTIC_ACTIONS
        {
            self.combat_view_actions_dropped = self
                .combat_view_actions_dropped
                .wrapping_add(u32::try_from(action_count).unwrap_or(u32::MAX));
            return ClientAuthorityDisposition::Rejected(
                ClientAuthorityRejection::CapacityExceeded,
            );
        }
        self.deferred_combat_view_actions
            .extend(actions.into_iter().map(|action| DeferredCombatViewAction {
                segment: self.segment,
                scope: self.scope,
                action,
            }));
        ClientAuthorityDisposition::Applied
    }

    pub(in crate::game) fn take_combat_view_actions(
        &mut self,
        release_ready: bool,
    ) -> Vec<SemanticInputAction> {
        if !release_ready {
            return Vec::new();
        }
        let segment = self.segment;
        let scope = self.scope;
        let mut released = Vec::with_capacity(self.deferred_combat_view_actions.len());
        while let Some(deferred) = self.deferred_combat_view_actions.pop_front() {
            if deferred.segment == segment && deferred.scope == scope {
                released.push(deferred.action);
            } else {
                self.combat_view_actions_dropped = self.combat_view_actions_dropped.wrapping_add(1);
            }
        }
        released
    }

    /// Cancel a correction at an explicit local authority boundary. Hard
    /// generation and life replacement use the same private cancellation
    /// primitive through `cancel_pending`; callers cannot clear only part of
    /// the transaction or retain its held gameplay edges.
    pub(in crate::game) fn cancel_correction(&mut self) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = if matches!(
            self.pending.as_ref(),
            Some(PendingTransition::Correction(_))
        ) {
            self.cancel_pending();
            self.phase = if self.scope.is_complete_live() && self.readiness.initial_baseline_ready()
            {
                ClientAuthorityPhase::Live
            } else {
                ClientAuthorityPhase::Failed
            };
            ClientAuthorityDisposition::Applied
        } else {
            ClientAuthorityDisposition::Duplicate
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::CorrectionCancelled,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn observe_correction_terminal(
        &mut self,
        receipt: MovementCorrectionAccepted,
    ) -> CorrectionReceiptOutcome {
        let prior = self.phase;
        self.correction_receipts_seen = self.correction_receipts_seen.wrapping_add(1);
        if self.correction_terminals.contains(&receipt) {
            self.record(
                ClientAuthoritySource::Reliable,
                ClientAuthorityFactKind::CorrectionTerminal,
                prior,
                ClientAuthorityDisposition::Duplicate,
                None,
            );
            return CorrectionReceiptOutcome::Duplicate;
        }
        let active_matches = match self.pending.as_ref() {
            Some(PendingTransition::Correction(correction)) => {
                correction.operation.is_some()
                    && correction_proposal_matches_terminal(correction.proposal, receipt)
                    && match receipt.outcome {
                        MovementCorrectionAccepted::OUTCOME_APPLIED => {
                            correction_application_matches_terminal(correction.application, receipt)
                        }
                        MovementCorrectionAccepted::OUTCOME_CANCELLED => true,
                        _ => false,
                    }
            }
            _ => false,
        };
        let current_runtime = self.scope.server_session_id == Some(receipt.server_session_id)
            && self.scope.player_id == Some(receipt.player_id)
            && self.scope.life_seq == Some(receipt.life_seq)
            && receipt.correction_generation != 0;
        let same_session_cancel = receipt.outcome == MovementCorrectionAccepted::OUTCOME_CANCELLED
            && self.scope.server_session_id == Some(receipt.server_session_id)
            && self.scope.player_id == Some(receipt.player_id);
        let cancelled_match = self
            .cancelled_corrections
            .iter()
            .any(|proposal| correction_proposal_matches_terminal(*proposal, receipt));
        if active_matches {
            let Some(PendingTransition::Correction(correction)) = self.pending.take() else {
                unreachable!()
            };
            self.release_correction_actions(correction.retained_actions);
            self.push_correction_terminal(receipt);
            self.phase = ClientAuthorityPhase::Live;
            self.correction_receipts_matched = self.correction_receipts_matched.wrapping_add(1);
            let outcome = if receipt.outcome == MovementCorrectionAccepted::OUTCOME_APPLIED {
                CorrectionReceiptOutcome::Applied {
                    receipt,
                    proposal_reason: correction.proposal.reason,
                    repel_window: correction.proposal.repel_window,
                }
            } else {
                CorrectionReceiptOutcome::Cancelled
            };
            self.record(
                ClientAuthoritySource::Reliable,
                ClientAuthorityFactKind::CorrectionTerminal,
                prior,
                ClientAuthorityDisposition::Applied,
                correction.operation.map(|operation| operation.delivery_id),
            );
            outcome
        } else if cancelled_match
            || ((current_runtime || same_session_cancel) && self.pending.is_none())
        {
            self.push_correction_terminal(receipt);
            self.correction_receipts_matched = self.correction_receipts_matched.wrapping_add(1);
            self.record(
                ClientAuthoritySource::Reliable,
                ClientAuthorityFactKind::CorrectionTerminal,
                prior,
                ClientAuthorityDisposition::Observed,
                None,
            );
            CorrectionReceiptOutcome::Duplicate
        } else {
            self.correction_receipts_unmatched = self.correction_receipts_unmatched.wrapping_add(1);
            self.record(
                ClientAuthoritySource::Reliable,
                ClientAuthorityFactKind::CorrectionTerminal,
                prior,
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::DeliveryMismatch),
                None,
            );
            CorrectionReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch)
        }
    }

    fn push_correction_terminal(&mut self, receipt: MovementCorrectionAccepted) {
        if self.correction_terminals.len() == MAX_CORRECTION_TERMINALS {
            self.correction_terminals.pop_front();
        }
        self.correction_terminals.push_back(receipt);
    }

    fn push_cancelled_correction(&mut self, proposal: MovementCorrection) {
        if self.cancelled_corrections.contains(&proposal) {
            return;
        }
        if self.cancelled_corrections.len() == MAX_CORRECTION_TERMINALS {
            self.cancelled_corrections.pop_front();
        }
        self.cancelled_corrections.push_back(proposal);
    }

    fn discard_correction_actions(&mut self, count: usize) {
        self.correction_actions_dropped = self
            .correction_actions_dropped
            .wrapping_add(u32::try_from(count).unwrap_or(u32::MAX));
    }

    fn discard_combat_view_actions(&mut self) {
        self.combat_view_actions_dropped = self.combat_view_actions_dropped.wrapping_add(
            u32::try_from(self.deferred_combat_view_actions.len()).unwrap_or(u32::MAX),
        );
        self.deferred_combat_view_actions.clear();
    }

    fn release_correction_actions(&mut self, mut actions: VecDeque<SemanticInputAction>) {
        let available = crate::input::MAX_PENDING_SEMANTIC_ACTIONS
            .saturating_sub(self.released_correction_actions.len());
        let retained = available.min(actions.len());
        self.released_correction_actions
            .extend(actions.drain(..retained));
        self.discard_correction_actions(actions.len());
    }

    /// Advance the presentation-side wait for the server-owned respawn
    /// deadline. Becoming eligible only exposes the exact neutral baseline
    /// command; it never makes the successor life live by itself.
    pub(in crate::game) fn advance_respawn_eligibility(
        &mut self,
        dt: f32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Respawn(respawn)) if !respawn.baseline_ready => {
                respawn.remaining_seconds = (respawn.remaining_seconds - dt.max(0.0)).max(0.0);
                if respawn.remaining_seconds == 0.0 {
                    respawn.baseline_ready = true;
                    self.phase = ClientAuthorityPhase::AwaitingDurableSubmission;
                    ClientAuthorityDisposition::Applied
                } else {
                    ClientAuthorityDisposition::Observed
                }
            }
            Some(PendingTransition::Respawn(_)) => ClientAuthorityDisposition::Duplicate,
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::Deadline,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn record_lifecycle_baseline_submitted(
        &mut self,
        delivery_id: u32,
        sequence: u16,
        accepted_baseline: Option<ClientPositionUpdate>,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.target_life_seq.is_some()
                    && lifecycle.operation.is_none()
                    && lifecycle.request_sequence == Some(sequence) =>
            {
                lifecycle.operation = Some(DurableOperation {
                    delivery_id,
                    sequence,
                    elapsed_seconds: 0.0,
                });
                lifecycle.accepted_baseline = accepted_baseline;
                lifecycle.elapsed_seconds = 0.0;
                self.phase = ClientAuthorityPhase::AwaitingApplicationReceipt;
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.operation.is_some_and(|operation| {
                    operation.delivery_id == delivery_id && operation.sequence == sequence
                }) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            Some(PendingTransition::Respawn(respawn))
                if respawn.baseline_ready && respawn.operation.is_none() =>
            {
                respawn.operation = Some(DurableOperation {
                    delivery_id,
                    sequence,
                    elapsed_seconds: 0.0,
                });
                self.phase = ClientAuthorityPhase::AwaitingApplicationReceipt;
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::Respawn(respawn))
                if respawn.operation.is_some_and(|operation| {
                    operation.delivery_id == delivery_id && operation.sequence == sequence
                }) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::DurableSubmitted,
            prior,
            disposition,
            Some(delivery_id),
        );
        disposition
    }

    pub(in crate::game) fn record_initial_baseline_submitted(
        &mut self,
        delivery_id: u32,
        sequence: u16,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::InitialJoin(initial))
                if self.phase == ClientAuthorityPhase::AwaitingDurableSubmission
                    && initial.life_seq.is_some()
                    && initial.operation.is_none() =>
            {
                initial.operation = Some(DurableOperation {
                    delivery_id,
                    sequence,
                    elapsed_seconds: 0.0,
                });
                self.phase = ClientAuthorityPhase::AwaitingApplicationReceipt;
                ClientAuthorityDisposition::Applied
            }
            Some(PendingTransition::InitialJoin(initial))
                if initial.operation.is_some_and(|operation| {
                    operation.delivery_id == delivery_id && operation.sequence == sequence
                }) =>
            {
                ClientAuthorityDisposition::Duplicate
            }
            _ => ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase),
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::DurableSubmitted,
            prior,
            disposition,
            Some(delivery_id),
        );
        disposition
    }

    pub(in crate::game) fn observe_initial_receipt(
        &mut self,
        receipt: LifecycleBaselineAccepted,
    ) -> InitialReceiptOutcome {
        let prior = self.phase;
        let matching_closed = self.closed.iter().any(|closed| {
            closed.segment == self.segment
                && closed.kind == ClientAuthorityTransitionKind::InitialJoin
                && closed.delivery_id == receipt.client_reliable_delivery_id
                && closed.sequence == receipt.position_sequence
                && closed.scope.life_seq == Some(receipt.life_seq)
        });
        let initial_transition_active = matches!(
            self.pending.as_ref(),
            Some(PendingTransition::InitialJoin(_))
        );
        if !matching_closed && !initial_transition_active {
            self.record(
                ClientAuthoritySource::Reliable,
                ClientAuthorityFactKind::ApplicationReceipt,
                prior,
                ClientAuthorityDisposition::Observed,
                Some(receipt.client_reliable_delivery_id),
            );
            return InitialReceiptOutcome::NotInitial;
        }
        self.lifecycle_receipts_seen = self.lifecycle_receipts_seen.wrapping_add(1);
        let (outcome, disposition) = if matching_closed {
            (
                InitialReceiptOutcome::Duplicate,
                ClientAuthorityDisposition::Duplicate,
            )
        } else {
            let matching = match self.pending.as_ref() {
                Some(PendingTransition::InitialJoin(initial)) => initial
                    .operation
                    .zip(initial.life_seq)
                    .is_some_and(|(operation, life_seq)| {
                        operation.delivery_id == receipt.client_reliable_delivery_id
                            && operation.sequence == receipt.position_sequence
                            && life_seq == receipt.life_seq
                    }),
                _ => false,
            };
            if matching {
                let operation = match self.pending.as_ref() {
                    Some(PendingTransition::InitialJoin(initial)) => initial.operation.unwrap(),
                    _ => unreachable!(),
                };
                self.push_closed(ClosedTransition {
                    segment: self.segment,
                    scope: self.scope,
                    kind: ClientAuthorityTransitionKind::InitialJoin,
                    delivery_id: operation.delivery_id,
                    sequence: operation.sequence,
                    completed: true,
                });
                self.pending = None;
                self.phase = ClientAuthorityPhase::Live;
                self.lifecycle_receipts_matched = self.lifecycle_receipts_matched.wrapping_add(1);
                (
                    InitialReceiptOutcome::Applied,
                    ClientAuthorityDisposition::Applied,
                )
            } else if initial_transition_active {
                self.lifecycle_receipts_unmatched =
                    self.lifecycle_receipts_unmatched.wrapping_add(1);
                (
                    InitialReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch),
                    ClientAuthorityDisposition::Rejected(
                        ClientAuthorityRejection::DeliveryMismatch,
                    ),
                )
            } else {
                unreachable!("inactive initial transition returned before receipt matching")
            }
        };
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::ApplicationReceipt,
            prior,
            disposition,
            Some(receipt.client_reliable_delivery_id),
        );
        outcome
    }

    pub(in crate::game) fn observe_lifecycle_receipt(
        &mut self,
        receipt: LifecycleBaselineAccepted,
    ) -> LifecycleReceiptOutcome {
        let prior = self.phase;
        let matching_closed = self.closed.iter().any(|closed| {
            closed.segment == self.segment
                && matches!(
                    closed.kind,
                    ClientAuthorityTransitionKind::Ship
                        | ClientAuthorityTransitionKind::Frequency
                        | ClientAuthorityTransitionKind::Respawn
                )
                && closed.delivery_id == receipt.client_reliable_delivery_id
                && closed.sequence == receipt.position_sequence
                && closed.scope.life_seq == Some(receipt.life_seq)
        });
        let lifecycle_active = matches!(
            self.pending.as_ref(),
            Some(PendingTransition::Lifecycle(_) | PendingTransition::Respawn(_))
        );
        if !matching_closed && !lifecycle_active {
            self.record(
                ClientAuthoritySource::Reliable,
                ClientAuthorityFactKind::ApplicationReceipt,
                prior,
                ClientAuthorityDisposition::Observed,
                Some(receipt.client_reliable_delivery_id),
            );
            return LifecycleReceiptOutcome::NotLifecycle;
        }
        self.lifecycle_receipts_seen = self.lifecycle_receipts_seen.wrapping_add(1);
        let (outcome, disposition) = if matching_closed {
            (
                LifecycleReceiptOutcome::Duplicate,
                ClientAuthorityDisposition::Duplicate,
            )
        } else {
            let matching = match self.pending.as_ref() {
                Some(PendingTransition::Lifecycle(lifecycle)) => lifecycle
                    .operation
                    .zip(lifecycle.target_life_seq)
                    .is_some_and(|(operation, life_seq)| {
                        operation.delivery_id == receipt.client_reliable_delivery_id
                            && operation.sequence == receipt.position_sequence
                            && life_seq == receipt.life_seq
                    }),
                Some(PendingTransition::Respawn(respawn)) => {
                    respawn.operation.is_some_and(|operation| {
                        operation.delivery_id == receipt.client_reliable_delivery_id
                            && operation.sequence == receipt.position_sequence
                            && respawn.target_life_seq == receipt.life_seq
                    })
                }
                _ => false,
            };
            if matching {
                let (kind, operation, accepted_baseline) = match self
                    .pending
                    .take()
                    .expect("matching lifecycle transition exists")
                {
                    PendingTransition::Lifecycle(lifecycle) => (
                        lifecycle.kind(),
                        lifecycle
                            .operation
                            .expect("matching lifecycle operation exists"),
                        lifecycle.accepted_baseline,
                    ),
                    PendingTransition::Respawn(respawn) => (
                        ClientAuthorityTransitionKind::Respawn,
                        respawn
                            .operation
                            .expect("matching respawn operation exists"),
                        None,
                    ),
                    _ => unreachable!(),
                };
                self.scope.life_seq = Some(receipt.life_seq);
                self.scope.alive = Some(true);
                self.push_closed(ClosedTransition {
                    segment: self.segment,
                    scope: self.scope,
                    kind,
                    delivery_id: operation.delivery_id,
                    sequence: operation.sequence,
                    completed: true,
                });
                self.phase = ClientAuthorityPhase::Live;
                self.lifecycle_receipts_matched = self.lifecycle_receipts_matched.wrapping_add(1);
                (
                    LifecycleReceiptOutcome::Applied {
                        kind,
                        accepted_baseline,
                    },
                    ClientAuthorityDisposition::Applied,
                )
            } else {
                self.lifecycle_receipts_unmatched =
                    self.lifecycle_receipts_unmatched.wrapping_add(1);
                (
                    LifecycleReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch),
                    ClientAuthorityDisposition::Rejected(
                        ClientAuthorityRejection::DeliveryMismatch,
                    ),
                )
            }
        };
        self.record(
            ClientAuthoritySource::Reliable,
            ClientAuthorityFactKind::ApplicationReceipt,
            prior,
            disposition,
            Some(receipt.client_reliable_delivery_id),
        );
        outcome
    }

    pub(in crate::game) fn advance_initial_deadline(
        &mut self,
        dt: f32,
        timeout_seconds: f32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::InitialJoin(initial)) => {
                if let Some(operation) = initial.operation.as_mut() {
                    operation.elapsed_seconds =
                        (operation.elapsed_seconds + dt.max(0.0)).min(timeout_seconds);
                    if operation.elapsed_seconds >= timeout_seconds {
                        let closed = ClosedTransition {
                            segment: self.segment,
                            scope: self.scope,
                            kind: ClientAuthorityTransitionKind::InitialJoin,
                            delivery_id: operation.delivery_id,
                            sequence: operation.sequence,
                            completed: false,
                        };
                        self.push_closed(closed);
                        self.pending = None;
                        self.phase = ClientAuthorityPhase::Failed;
                        ClientAuthorityDisposition::TimedOut
                    } else {
                        ClientAuthorityDisposition::Observed
                    }
                } else {
                    // The wait before submission used to be unbounded, so a client whose arena readiness never
                    // completed stayed frozen with nothing naming why. It is now bounded by the same budget as the
                    // submitted wait, and expiry *re-asks* rather than failing: the server's answers are what resolve
                    // the join, so the client re-arms its request retry budget instead of inventing state or tearing
                    // the session down. The accumulator resets so the cadence repeats instead of firing every frame.
                    initial.pre_submission_seconds =
                        (initial.pre_submission_seconds + dt.max(0.0)).min(timeout_seconds);
                    if initial.pre_submission_seconds >= timeout_seconds {
                        initial.pre_submission_seconds = 0.0;
                        ClientAuthorityDisposition::Reattempt
                    } else {
                        ClientAuthorityDisposition::Observed
                    }
                }
            }
            _ => ClientAuthorityDisposition::Observed,
        };
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::Deadline,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn advance_lifecycle_deadline(
        &mut self,
        dt: f32,
        timeout_seconds: f32,
    ) -> ClientAuthorityDisposition {
        let prior = self.phase;
        let mut timed_out = None;
        let mut correction_timed_out = false;
        let disposition = match self.pending.as_mut() {
            Some(PendingTransition::Lifecycle(lifecycle)) => {
                if lifecycle.request_sequence.is_none() {
                    ClientAuthorityDisposition::Observed
                } else {
                    // The request was durably queued, so its authoritative
                    // roster/private ACK, unsent baseline, and application
                    // receipt share one bounded transaction deadline.
                    lifecycle.elapsed_seconds =
                        (lifecycle.elapsed_seconds + dt.max(0.0)).min(timeout_seconds);
                    if lifecycle.elapsed_seconds >= timeout_seconds {
                        if let (Some(target_life_seq), Some(operation)) =
                            (lifecycle.target_life_seq, lifecycle.operation)
                        {
                            timed_out = Some((lifecycle.kind(), target_life_seq, operation));
                        }
                        ClientAuthorityDisposition::TimedOut
                    } else {
                        ClientAuthorityDisposition::Observed
                    }
                }
            }
            Some(PendingTransition::Respawn(respawn)) => {
                if let Some(operation) = respawn.operation.as_mut() {
                    operation.elapsed_seconds =
                        (operation.elapsed_seconds + dt.max(0.0)).min(timeout_seconds);
                    if operation.elapsed_seconds >= timeout_seconds {
                        timed_out = Some((
                            ClientAuthorityTransitionKind::Respawn,
                            respawn.target_life_seq,
                            *operation,
                        ));
                        ClientAuthorityDisposition::TimedOut
                    } else {
                        ClientAuthorityDisposition::Observed
                    }
                } else {
                    // Asset readiness, the authoritative respawn wait, and
                    // reliable-capacity pressure cannot consume an
                    // application deadline before a durable operation exists.
                    ClientAuthorityDisposition::Observed
                }
            }
            Some(PendingTransition::Correction(correction)) => {
                if let Some(operation) = correction.operation.as_mut() {
                    operation.elapsed_seconds =
                        (operation.elapsed_seconds + dt.max(0.0)).min(timeout_seconds);
                    if operation.elapsed_seconds >= timeout_seconds {
                        correction_timed_out = true;
                        ClientAuthorityDisposition::TimedOut
                    } else {
                        ClientAuthorityDisposition::Observed
                    }
                } else {
                    // Reliable-capacity pressure before durable submission is
                    // not part of the server application deadline.
                    ClientAuthorityDisposition::Observed
                }
            }
            _ => ClientAuthorityDisposition::Observed,
        };
        if disposition == ClientAuthorityDisposition::TimedOut {
            if let Some((kind, target_life_seq, operation)) = timed_out {
                let mut scope = self.scope;
                scope.life_seq = Some(target_life_seq);
                self.push_closed(ClosedTransition {
                    segment: self.segment,
                    scope,
                    kind,
                    delivery_id: operation.delivery_id,
                    sequence: operation.sequence,
                    completed: false,
                });
            }
            if correction_timed_out {
                let Some(PendingTransition::Correction(correction)) = self.pending.take() else {
                    unreachable!("correction timeout must retain its transaction");
                };
                self.push_cancelled_correction(correction.proposal);
                self.discard_correction_actions(correction.retained_actions.len());
            } else {
                self.pending = None;
            }
            self.phase = ClientAuthorityPhase::Failed;
        }
        self.record(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::Deadline,
            prior,
            disposition,
            None,
        );
        disposition
    }

    pub(in crate::game) fn advance_application_barrier_deadline(
        &mut self,
        dt: f32,
        timeout_seconds: f32,
    ) -> Option<ClientAuthorityApplicationBarrierKind> {
        let dt = dt.max(0.0);
        let mut timed_out = None;
        if let Some(pending) = self.prize_effect_applications.front_mut()
            && let Some(operation) = pending.operation.as_mut()
        {
            operation.elapsed_seconds = (operation.elapsed_seconds + dt).min(timeout_seconds);
            if operation.elapsed_seconds >= timeout_seconds {
                timed_out = Some((
                    ClientAuthorityApplicationBarrierKind::PrizeEffect,
                    operation.delivery_id,
                    pending.application.collectible_id,
                    pending.application.application_sequence,
                ));
            }
        }
        if timed_out.is_none()
            && let Some(pending) = self.portal_warp.as_mut()
        {
            pending.operation.elapsed_seconds =
                (pending.operation.elapsed_seconds + dt).min(timeout_seconds);
            if pending.operation.elapsed_seconds >= timeout_seconds {
                timed_out = Some((
                    ClientAuthorityApplicationBarrierKind::PortalWarp,
                    pending.operation.delivery_id,
                    pending.request.action_id,
                    pending.request.position_sequence,
                ));
            }
        }
        let Some((kind, delivery_id, application_identity, operation_sequence)) = timed_out else {
            return None;
        };
        let prior = self.phase;
        self.retire_life_scoped_application_barriers();
        self.deferred_actions = ClientProtocolActions::default();
        self.deferred_action_packet_bytes = 0;
        self.phase = ClientAuthorityPhase::Failed;
        self.record_operation(
            ClientAuthoritySource::Local,
            ClientAuthorityFactKind::Deadline,
            prior,
            ClientAuthorityDisposition::TimedOut,
            Some(delivery_id),
            Some(application_identity),
            Some(operation_sequence),
        );
        Some(kind)
    }

    fn refresh_initial_phase(&mut self) {
        let Some(PendingTransition::InitialJoin(initial)) = self.pending.as_ref() else {
            return;
        };
        self.phase = if initial.operation.is_some() {
            ClientAuthorityPhase::AwaitingApplicationReceipt
        } else if initial.life_seq.is_some() && self.readiness.initial_baseline_ready() {
            ClientAuthorityPhase::AwaitingDurableSubmission
        } else {
            ClientAuthorityPhase::AwaitingReadiness
        };
    }

    fn cancel_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            PendingTransition::InitialJoin(initial) => {
                if let (Some(life_seq), Some(operation)) = (initial.life_seq, initial.operation) {
                    let mut scope = self.scope;
                    scope.life_seq = Some(life_seq);
                    self.push_closed(ClosedTransition {
                        segment: self.segment,
                        scope,
                        kind: ClientAuthorityTransitionKind::InitialJoin,
                        delivery_id: operation.delivery_id,
                        sequence: operation.sequence,
                        completed: false,
                    });
                }
            }
            PendingTransition::Lifecycle(lifecycle) => {
                if let (Some(life_seq), Some(operation)) =
                    (lifecycle.target_life_seq, lifecycle.operation)
                {
                    let mut scope = self.scope;
                    scope.life_seq = Some(life_seq);
                    self.push_closed(ClosedTransition {
                        segment: self.segment,
                        scope,
                        kind: lifecycle.kind(),
                        delivery_id: operation.delivery_id,
                        sequence: operation.sequence,
                        completed: false,
                    });
                }
            }
            PendingTransition::Respawn(respawn) => {
                if let Some(operation) = respawn.operation {
                    let mut scope = self.scope;
                    scope.life_seq = Some(respawn.target_life_seq);
                    self.push_closed(ClosedTransition {
                        segment: self.segment,
                        scope,
                        kind: ClientAuthorityTransitionKind::Respawn,
                        delivery_id: operation.delivery_id,
                        sequence: operation.sequence,
                        completed: false,
                    });
                }
            }
            PendingTransition::Correction(correction) => {
                self.push_cancelled_correction(correction.proposal);
                self.discard_correction_actions(correction.retained_actions.len());
            }
        }
    }

    fn push_closed(&mut self, closed: ClosedTransition) {
        if self.closed.len() == MAX_CLOSED_TRANSITIONS {
            self.closed.pop_front();
        }
        self.closed.push_back(closed);
    }

    fn record(
        &mut self,
        source: ClientAuthoritySource,
        fact: ClientAuthorityFactKind,
        prior: ClientAuthorityPhase,
        disposition: ClientAuthorityDisposition,
        delivery_id: Option<u32>,
    ) {
        self.record_operation(source, fact, prior, disposition, delivery_id, None, None);
    }

    fn record_operation(
        &mut self,
        source: ClientAuthoritySource,
        fact: ClientAuthorityFactKind,
        prior: ClientAuthorityPhase,
        disposition: ClientAuthorityDisposition,
        delivery_id: Option<u32>,
        application_identity: Option<u32>,
        operation_sequence: Option<u16>,
    ) {
        let deadline_elapsed_seconds = match self.pending.as_ref() {
            Some(PendingTransition::InitialJoin(InitialJoinTransition {
                operation: Some(operation),
                ..
            })) => Some(operation.elapsed_seconds),
            Some(PendingTransition::Lifecycle(lifecycle))
                if lifecycle.request_sequence.is_some() =>
            {
                Some(lifecycle.elapsed_seconds)
            }
            Some(PendingTransition::Respawn(respawn)) => {
                respawn.operation.map(|operation| operation.elapsed_seconds)
            }
            Some(PendingTransition::Correction(correction)) => correction
                .operation
                .map(|operation| operation.elapsed_seconds),
            _ => self
                .prize_effect_applications
                .front()
                .and_then(|pending| pending.operation)
                .map(|operation| operation.elapsed_seconds)
                .or_else(|| {
                    self.portal_warp
                        .map(|pending| pending.operation.elapsed_seconds)
                }),
        };
        if self.ledger.len() == MAX_TRANSITION_LEDGER {
            self.ledger.pop_front();
        }
        let entry = ClientAuthorityLedgerEntry {
            source,
            fact,
            scope: self.scope,
            prior,
            disposition,
            resulting: self.phase,
            delivery_id,
            application_identity,
            operation_sequence,
            deadline_elapsed_seconds,
        };
        if entry.resulting != entry.prior {
            self.last_phase_change = Some(entry);
        }
        self.ledger.push_back(entry);
    }

    #[cfg(test)]
    pub(in crate::game) fn ledger(&self) -> &VecDeque<ClientAuthorityLedgerEntry> {
        &self.ledger
    }

    #[cfg(test)]
    pub(in crate::game) fn closed_len(&self) -> usize {
        self.closed.len()
    }

    #[cfg(test)]
    pub(in crate::game) fn install_test_scope(
        &mut self,
        segment: u64,
        scope: AuthorityScopeIdentity,
        readiness: ClientAuthorityReadiness,
    ) {
        self.segment = segment;
        self.scope = scope;
        self.readiness = readiness;
        if scope.is_complete_live() && readiness.initial_baseline_ready() {
            self.pending = None;
            self.phase = ClientAuthorityPhase::Live;
        } else if readiness.joined
            && scope.server_session_id.is_some()
            && scope.arena_stream_id.is_some()
            && scope.player_id.is_some()
            && scope.life_seq.is_none()
        {
            self.pending = Some(PendingTransition::InitialJoin(InitialJoinTransition {
                life_seq: None,
                operation: None,
                pre_submission_seconds: 0.0,
            }));
            self.phase = ClientAuthorityPhase::AwaitingReadiness;
        } else {
            self.pending = None;
            self.phase = ClientAuthorityPhase::Bootstrapping;
        }
        self.closed.clear();
        self.ledger.clear();
        self.last_phase_change = None;
        self.correction_terminals.clear();
        self.cancelled_corrections.clear();
        self.released_correction_actions.clear();
        self.correction_actions_dropped = 0;
        self.correction_receipts_seen = 0;
        self.correction_receipts_matched = 0;
        self.correction_receipts_unmatched = 0;
        self.deferred_combat_view_actions.clear();
        self.combat_view_actions_dropped = 0;
        self.deferred_actions = ClientProtocolActions::default();
        self.deferred_action_packet_bytes = 0;
        self.deferred_action_overflows = 0;
        self.deferred_warp_pickup_acks.clear();
        self.deferred_warp_pickup_ack_overflows = 0;
        self.prize_effect_applications.clear();
        self.prize_effect_terminals.clear();
        self.prize_effect_application_receipts_seen = 0;
        self.prize_effect_application_receipts_matched = 0;
        self.prize_effect_application_receipts_unmatched = 0;
        self.portal_warp = None;
        self.portal_warp_terminals.clear();
        self.application_barrier_overflows = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputModifiers;

    fn partial_scope() -> AuthorityScopeIdentity {
        AuthorityScopeIdentity {
            server_session_id: Some(7),
            arena_stream_id: Some(11),
            player_id: Some(23),
            life_seq: None,
            alive: None,
        }
    }

    fn live_scope() -> AuthorityScopeIdentity {
        AuthorityScopeIdentity {
            life_seq: Some(41),
            alive: Some(true),
            ..partial_scope()
        }
    }

    fn arm_initial(sequencer: &mut ClientAuthoritySequencer) {
        assert_eq!(
            sequencer.observe_hard_replacement(
                ClientAuthoritySource::Reliable,
                1,
                Some(partial_scope()),
            ),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(
            sequencer.observe_joined_arena(9),
            ClientAuthorityDisposition::Applied
        );
        sequencer.observe_settings_ready();
        sequencer.observe_security_ready();
        sequencer.observe_map_ready();
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Event,
                live_scope(),
                None,
                0.0,
            ),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.view().phase,
            ClientAuthorityPhase::AwaitingDurableSubmission
        );
    }

    #[test]
    fn initial_join_deadline_starts_only_after_durable_submission() {
        let mut sequencer = ClientAuthoritySequencer::default();
        arm_initial(&mut sequencer);
        for _ in 0..9 {
            assert_eq!(
                sequencer.advance_initial_deadline(1.0, 10.0),
                ClientAuthorityDisposition::Observed
            );
        }
        // This used to assert that an unsubmitted join waits forever, which is the unbounded
        // freeze bounded below; the name now describes when the *submission* deadline starts rather than claiming
        // there is no clock at all before it.
        assert_eq!(
            sequencer.advance_initial_deadline(1.0, 10.0),
            ClientAuthorityDisposition::Reattempt,
            "an unsubmitted join must re-ask rather than wait forever",
        );
        assert_ne!(sequencer.view().phase, ClientAuthorityPhase::Failed);
        assert_eq!(sequencer.view().deadline_elapsed_seconds, None);
        assert_eq!(
            sequencer.record_initial_baseline_submitted(28, 25044),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.advance_initial_deadline(9.9, 10.0),
            ClientAuthorityDisposition::Observed
        );
        assert_eq!(
            sequencer.advance_initial_deadline(0.1, 10.0),
            ClientAuthorityDisposition::TimedOut
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Failed);
    }

    #[test]
    fn decisive_ledger_record_survives_the_routine_deadline_ticks() {
        let mut sequencer = ClientAuthoritySequencer::default();
        arm_initial(&mut sequencer);
        sequencer.record_initial_baseline_submitted(28, 25044);
        assert_eq!(
            sequencer.advance_initial_deadline(10.0, 10.0),
            ClientAuthorityDisposition::TimedOut
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Failed);

        // Every later frame appends a routine Deadline/Observed record, so the
        // retained capture must still be able to name the record that actually
        // ended the phase instead of the last frame tick.
        for _ in 0..8 {
            assert_eq!(
                sequencer.advance_initial_deadline(0.1, 10.0),
                ClientAuthorityDisposition::Observed
            );
        }
        assert_eq!(
            sequencer
                .view()
                .last_transition
                .map(|entry| entry.disposition),
            Some(ClientAuthorityDisposition::Observed),
        );
        let decisive = sequencer
            .view()
            .last_decisive_transition
            .expect("the timeout record stays decisive after later frame ticks");
        assert_eq!(decisive.resulting, ClientAuthorityPhase::Failed);
        assert_ne!(
            decisive.prior, decisive.resulting,
            "the decisive record names the flip into Failed, not a later frame tick",
        );
        assert_eq!(decisive.disposition, ClientAuthorityDisposition::TimedOut);
    }

    #[test]
    fn exact_initial_receipt_completes_once_and_same_life_cannot_rearm() {
        let mut sequencer = ClientAuthoritySequencer::default();
        arm_initial(&mut sequencer);
        sequencer.record_initial_baseline_submitted(28, 25044);
        let receipt = LifecycleBaselineAccepted {
            client_reliable_delivery_id: 28,
            life_seq: 41,
            position_sequence: 25044,
            accepted_server_tick: 7_000,
            accepted_position: None,
        };
        assert_eq!(
            sequencer.observe_initial_receipt(receipt),
            InitialReceiptOutcome::Applied
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        assert_eq!(sequencer.view().live_scope(), Some(live_scope()));
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Event,
                live_scope(),
                None,
                0.0,
            ),
            ClientAuthorityDisposition::Duplicate
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        assert_eq!(
            sequencer.observe_initial_receipt(receipt),
            InitialReceiptOutcome::Duplicate
        );
        assert_eq!(sequencer.view().lifecycle_receipts_matched, 1);
    }

    #[test]
    fn replacement_cancels_old_delivery_and_stale_receipt_cannot_open_successor() {
        let mut sequencer = ClientAuthoritySequencer::default();
        arm_initial(&mut sequencer);
        sequencer.record_initial_baseline_submitted(u32::MAX, u16::MAX);
        let predecessor_receipt = LifecycleBaselineAccepted {
            client_reliable_delivery_id: u32::MAX,
            life_seq: 41,
            position_sequence: u16::MAX,
            accepted_server_tick: u32::MAX,
            accepted_position: None,
        };
        let replacement = AuthorityScopeIdentity {
            server_session_id: Some(8),
            arena_stream_id: Some(0),
            player_id: Some(23),
            life_seq: None,
            alive: None,
        };
        assert_eq!(
            sequencer.observe_hard_replacement(
                ClientAuthoritySource::Reliable,
                2,
                Some(replacement),
            ),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(
            sequencer.observe_initial_receipt(predecessor_receipt),
            InitialReceiptOutcome::NotInitial
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Bootstrapping);
    }

    #[test]
    fn readiness_and_terminal_storage_are_bounded_and_wrap_safe() {
        let mut sequencer = ClientAuthoritySequencer::default();
        sequencer.segment = u64::MAX;
        assert_eq!(
            sequencer.observe_hard_replacement(
                ClientAuthoritySource::Reliable,
                0,
                Some(partial_scope()),
            ),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(
            sequencer.observe_map_pending(),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        );
        assert_eq!(
            sequencer.observe_files_ready(),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ImpossiblePhase)
        );

        for index in 0..(MAX_CLOSED_TRANSITIONS + 8) {
            sequencer.push_closed(ClosedTransition {
                segment: index as u64,
                scope: live_scope(),
                kind: ClientAuthorityTransitionKind::Correction,
                delivery_id: index as u32,
                sequence: index as u16,
                completed: index % 2 == 0,
            });
            sequencer.record(
                ClientAuthoritySource::Realtime,
                ClientAuthorityFactKind::PrivateLifecycle,
                ClientAuthorityPhase::Live,
                ClientAuthorityDisposition::Observed,
                None,
            );
        }
        assert_eq!(sequencer.closed_len(), MAX_CLOSED_TRANSITIONS);
        assert_eq!(sequencer.ledger().len(), MAX_TRANSITION_LEDGER);
    }

    fn live_sequencer() -> ClientAuthoritySequencer {
        let mut sequencer = ClientAuthoritySequencer::default();
        sequencer.install_test_scope(
            1,
            live_scope(),
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(9),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        sequencer
    }

    fn frequency_baseline(sequence: u16) -> ClientPositionUpdate {
        ClientPositionUpdate {
            direction: crate::continuum_position::protocol_direction(0.3),
            timestamp: 0,
            x: 400,
            y: 500,
            velocity_x: crate::continuum_position::protocol_velocity_component(6.0),
            velocity_y: crate::continuum_position::protocol_velocity_component(7.0),
            togglables: 1,
            bounty: 8,
            energy: 900,
            weapon: 0,
            extra: Some(ClientPositionExtra {
                s2c_latency: 0,
                flag_timer: 0,
                items: crate::continuum_position::zubspace_position_sequence_items(sequence),
            }),
            canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
                400.0, 500.0, 6.0, 7.0, 0.3,
            ),
            repel_application: None,
            input_history: None,
            warp_collectible_id: None,
        }
    }

    fn correction(position_sequence: u16, generation: u32) -> MovementCorrection {
        MovementCorrection {
            reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
            repel_window: None,
            server_session_id: 7,
            player_id: 23,
            life_seq: 41,
            correction_generation: generation,
            position_sequence,
            server_tick: 1_234,
            position_x: 10_500.375,
            position_y: 8_191.75,
            velocity_x: -321.125,
            velocity_y: 47.5,
            orientation: 0.9875,
        }
    }

    fn correction_terminal(
        application: MovementCorrectionApplied,
        outcome: u8,
    ) -> MovementCorrectionAccepted {
        MovementCorrectionAccepted {
            outcome,
            server_tick: 100,
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            correction_generation: application.correction_generation,
            detected_sequence: application.detected_sequence,
            application_sequence: application.application_sequence,
            position_x: 12_000.375,
            position_y: 9_000.75,
            velocity_x: -201.125,
            velocity_y: 91.5,
            orientation: 0.75,
        }
    }

    fn quality_force(
        owner: &ClientAuthoritySequencer,
    ) -> crate::protocol::ConnectionQualityDirective {
        let scope = owner.view().scope;
        crate::protocol::ConnectionQualityDirective {
            kind: crate::protocol::ConnectionQualityDirectiveKind::Force,
            server_session_id: scope.server_session_id.unwrap(),
            arena_stream_id: scope.arena_stream_id.unwrap(),
            player_id: scope.player_id.unwrap(),
            predecessor_life_seq: scope.life_seq.unwrap(),
            life_seq: scope.life_seq.unwrap().wrapping_add(1).max(1),
            hold_id: 8,
            remaining_ms: 120_000,
            rejected_request: None,
        }
    }

    #[test]
    fn quality_force_cancels_pending_ship_and_cannot_rearm_after_clear() {
        use crate::protocol::ConnectionQualityDirectiveKind as Kind;
        let mut owner = live_sequencer();
        let fact = quality_force(&owner);
        owner.queue_lifecycle_intent(PendingLifecycleRequest::Ship(3));
        owner.record_ship_request_submitted(3, 80);
        assert_eq!(
            owner.observe_quality_directive(fact, 100.0),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(owner.view().phase, ClientAuthorityPhase::Spectating);
        assert!(owner.view().observer_scope().is_some());
        assert!(owner.view().gameplay_scope().is_none());
        assert!(!owner.view().transition_pending);
        assert_eq!(owner.quality_hold_remaining(110.0), Some(110.0));
        assert_eq!(
            owner.observe_quality_directive(fact, 110.0),
            ClientAuthorityDisposition::Duplicate
        );
        assert_eq!(owner.quality_hold_remaining(110.0), Some(110.0));
        let hold = crate::protocol::ConnectionQualityDirective {
            kind: Kind::Hold,
            predecessor_life_seq: fact.life_seq,
            ..fact
        };
        owner.observe_quality_directive(hold, 120.0);
        assert_eq!(owner.quality_hold_remaining(120.0), Some(100.0));
        let clear = crate::protocol::ConnectionQualityDirective {
            kind: Kind::Clear,
            remaining_ms: 0,
            ..hold
        };
        assert_eq!(
            owner.observe_quality_directive(clear, 230.0),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(owner.quality_hold_remaining(230.0), None);
        assert_eq!(
            owner.observe_quality_directive(fact, 231.0),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::Cancelled)
        );
        assert_eq!(
            owner.observe_quality_directive(clear, 231.0),
            ClientAuthorityDisposition::Duplicate
        );
    }

    #[test]
    fn quality_force_accepts_snapshot_overtake_but_rejects_foreign_and_old_scope() {
        let mut owner = live_sequencer();
        let fact = quality_force(&owner);
        for foreign in [
            crate::protocol::ConnectionQualityDirective {
                server_session_id: fact.server_session_id + 1,
                ..fact
            },
            crate::protocol::ConnectionQualityDirective {
                arena_stream_id: fact.arena_stream_id + 1,
                ..fact
            },
            crate::protocol::ConnectionQualityDirective {
                player_id: fact.player_id + 1,
                ..fact
            },
        ] {
            assert_eq!(
                owner.observe_quality_directive(foreign, 0.0),
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
            );
        }
        let mut successor = owner.view().scope;
        successor.life_seq = Some(fact.life_seq);
        successor.alive = Some(false);
        owner.observe_private_lifecycle(ClientAuthoritySource::Realtime, successor, Some(500), 1.0);
        assert_eq!(owner.view().scope.life_seq, Some(fact.life_seq));
        assert_eq!(
            owner.observe_quality_directive(fact, 0.0),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(owner.view().phase, ClientAuthorityPhase::Spectating);
        let mut newer = fact;
        newer.hold_id += 1;
        owner.scope.life_seq = Some(fact.life_seq + 1);
        assert_eq!(
            owner.observe_quality_directive(newer, 0.0),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::LifeMismatch)
        );
    }

    #[test]
    fn quality_hold_rejects_only_the_exact_pending_return_to_play_request() {
        use crate::protocol::ConnectionQualityDirectiveKind as Kind;
        let mut owner = live_sequencer();
        let fact = quality_force(&owner);
        owner.observe_quality_directive(fact, 0.0);
        owner.queue_lifecycle_intent(PendingLifecycleRequest::Ship(7));
        owner.record_ship_request_submitted(7, 33);
        let rejection = crate::protocol::ConnectionQualityDirective {
            kind: Kind::Hold,
            predecessor_life_seq: fact.life_seq,
            rejected_request: Some((7, 33)),
            remaining_ms: 119_000,
            ..fact
        };
        assert_eq!(
            owner.observe_quality_directive(
                crate::protocol::ConnectionQualityDirective {
                    rejected_request: Some((7, 34)),
                    ..rejection
                },
                1.0
            ),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::DeliveryMismatch)
        );
        assert!(owner.view().transition_pending);
        assert_eq!(
            owner.observe_quality_directive(rejection, 1.0),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(owner.view().phase, ClientAuthorityPhase::Spectating);
        assert!(!owner.view().transition_pending);
        assert!(owner.view().observer_scope().is_some());
        assert!(owner.view().gameplay_scope().is_none());
        owner.observe_hard_replacement(
            ClientAuthoritySource::Reliable,
            owner.segment + 1,
            Some(partial_scope()),
        );
        assert_eq!(owner.quality_hold_remaining(1.0), None);
    }

    fn retained_fire_action() -> SemanticInputAction {
        SemanticInputAction {
            kind: SemanticInputActionKind::PrimaryFireOrSpectate,
            modifiers: InputModifiers::default(),
        }
    }

    #[test]
    fn initial_spectator_requires_authored_ship_and_ready_scoped_lifecycle() {
        let mut owner = ClientAuthoritySequencer::default();
        owner.observe_hard_replacement(ClientAuthoritySource::Reliable, 1, Some(partial_scope()));
        owner.observe_joined_arena(9);
        owner.observe_initial_ship(8);
        let scope = AuthorityScopeIdentity {
            alive: Some(false),
            ..live_scope()
        };
        assert_eq!(
            owner.observe_private_lifecycle(ClientAuthoritySource::Event, scope, None, 0.0),
            ClientAuthorityDisposition::Replacement
        );
        assert!(owner.view().observer_scope().is_none());
        owner.observe_settings_ready();
        owner.observe_security_ready();
        owner.observe_map_ready();
        assert!(owner.view().observer_scope().is_some());
        assert!(owner.view().gameplay_scope().is_none());
        owner.observe_hard_replacement(ClientAuthoritySource::Reliable, 2, Some(partial_scope()));
        assert!(owner.view().observer_scope().is_none());
    }

    #[test]
    fn spectator_cutover_has_observer_scope_but_never_gameplay_scope() {
        let mut owner = live_sequencer();
        let next_life = owner.view().scope.life_seq.unwrap() + 1;
        assert_eq!(
            owner.queue_lifecycle_intent(PendingLifecycleRequest::Ship(8)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            owner.record_ship_request_submitted(8, 11),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            owner.observe_ship_ack(8, Some(next_life)),
            ClientAuthorityDisposition::Replacement
        );
        let view = owner.view();
        assert!(view.observer_scope().is_some());
        assert!(view.gameplay_scope().is_none());
        assert!(view.live_scope().is_none());
        assert!(!view.transition_pending);
        assert_eq!(view.scope.alive, Some(false));
        assert_eq!(
            owner.queue_lifecycle_intent(PendingLifecycleRequest::Ship(7)),
            ClientAuthorityDisposition::Applied
        );
        owner.record_ship_request_submitted(7, 12);
        owner.observe_ship_ack(7, Some(next_life + 1));
        assert!(owner.view().observer_scope().is_none());
        assert!(
            owner.view().gameplay_scope().is_none(),
            "return to play still needs its exact position receipt"
        );
        assert!(owner.view().lifecycle_baseline_ready_to_submit);
    }

    #[test]
    fn spectator_snapshot_before_roster_ack_does_not_become_a_respawn() {
        let mut owner = live_sequencer();
        owner.queue_lifecycle_intent(PendingLifecycleRequest::Ship(8));
        owner.record_ship_request_submitted(8, 11);
        let mut next = owner.view().scope;
        next.life_seq = Some(next.life_seq.unwrap() + 1);
        next.alive = Some(false);
        assert_eq!(
            owner.observe_private_lifecycle(ClientAuthoritySource::Event, next, None, 0.0),
            ClientAuthorityDisposition::Observed
        );
        assert_eq!(
            owner.view().lifecycle_intent,
            Some(PendingLifecycleRequest::Ship(8))
        );
        assert!(owner.view().observer_scope().is_none());
        owner.observe_ship_ack(8, next.life_seq);
        assert!(owner.view().observer_scope().is_some());
        assert_eq!(
            owner.observe_private_lifecycle(ClientAuthoritySource::Event, next, None, 0.0),
            ClientAuthorityDisposition::Duplicate
        );
    }

    #[test]
    fn only_the_exact_unacknowledged_ship_rejection_reopens_the_old_life() {
        let mut sequencer = live_sequencer();
        let old_scope = sequencer.view().scope;
        sequencer.queue_lifecycle_intent(PendingLifecycleRequest::Ship(6));
        sequencer.record_ship_request_submitted(6, u16::MAX);
        let receipt = crate::protocol::ShipChangeRejected {
            ship: 6,
            server_session_id: old_scope.server_session_id.unwrap(),
            arena_stream_id: old_scope.arena_stream_id.unwrap(),
            player_id: old_scope.player_id.unwrap(),
            life_seq: old_scope.life_seq.unwrap(),
            position_sequence: u16::MAX,
        };
        for stale in [
            crate::protocol::ShipChangeRejected { ship: 5, ..receipt },
            crate::protocol::ShipChangeRejected {
                server_session_id: receipt.server_session_id + 1,
                ..receipt
            },
            crate::protocol::ShipChangeRejected {
                arena_stream_id: receipt.arena_stream_id + 1,
                ..receipt
            },
            crate::protocol::ShipChangeRejected {
                player_id: receipt.player_id + 1,
                ..receipt
            },
            crate::protocol::ShipChangeRejected {
                life_seq: receipt.life_seq + 1,
                ..receipt
            },
            crate::protocol::ShipChangeRejected {
                position_sequence: 0,
                ..receipt
            },
        ] {
            assert!(matches!(
                sequencer.observe_ship_rejection(stale),
                ClientAuthorityDisposition::Rejected(_)
            ));
            assert!(sequencer.view().lifecycle_intent.is_some());
        }
        assert_eq!(
            sequencer.observe_ship_rejection(receipt),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        assert_eq!(sequencer.view().scope, old_scope);
        assert_eq!(sequencer.view().lifecycle_intent, None);
        assert!(matches!(
            sequencer.observe_ship_rejection(receipt),
            ClientAuthorityDisposition::Rejected(_)
        ));
        sequencer.queue_lifecycle_intent(PendingLifecycleRequest::Ship(6));
        sequencer.record_ship_request_submitted(6, 0);
        assert!(matches!(
            sequencer.observe_ship_rejection(receipt),
            ClientAuthorityDisposition::Rejected(_)
        ));
        sequencer.observe_ship_ack(6, Some(receipt.life_seq + 1));
        assert!(matches!(
            sequencer.observe_ship_rejection(crate::protocol::ShipChangeRejected {
                position_sequence: 0,
                ..receipt
            }),
            ClientAuthorityDisposition::Rejected(_)
        ));
    }

    #[test]
    fn ship_cutover_advances_scope_once_and_exact_receipt_reopens_gameplay() {
        let mut sequencer = live_sequencer();
        assert_eq!(
            sequencer.queue_lifecycle_intent(PendingLifecycleRequest::Ship(6)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(sequencer.view().live_scope(), None);
        assert_eq!(
            sequencer.record_ship_request_submitted(6, u16::MAX),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.observe_ship_ack(6, Some(42)),
            ClientAuthorityDisposition::Replacement
        );
        assert!(sequencer.view().lifecycle_baseline_ready_to_submit);
        assert_eq!(sequencer.view().scope.life_seq, Some(42));
        assert_eq!(
            sequencer.record_lifecycle_baseline_submitted(u32::MAX, u16::MAX, None),
            ClientAuthorityDisposition::Applied
        );
        let receipt = LifecycleBaselineAccepted {
            client_reliable_delivery_id: u32::MAX,
            life_seq: 42,
            position_sequence: u16::MAX,
            accepted_server_tick: 8_000,
            accepted_position: None,
        };
        assert_eq!(
            sequencer.observe_lifecycle_receipt(receipt),
            LifecycleReceiptOutcome::Applied {
                kind: ClientAuthorityTransitionKind::Ship,
                accepted_baseline: None,
            }
        );
        assert_eq!(sequencer.view().live_scope().unwrap().life_seq, Some(42));
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Event,
                AuthorityScopeIdentity {
                    life_seq: Some(42),
                    ..live_scope()
                },
                None,
                0.0,
            ),
            ClientAuthorityDisposition::Duplicate
        );
        assert_eq!(
            sequencer.observe_lifecycle_receipt(receipt),
            LifecycleReceiptOutcome::Duplicate
        );
    }

    #[test]
    fn frequency_cutover_retains_canonical_baseline_and_rejects_old_life_receipt() {
        let mut sequencer = live_sequencer();
        let baseline = frequency_baseline(0);
        assert!(baseline.canonical_state.matches_legacy_wire(
            baseline.direction,
            baseline.x,
            baseline.y,
            baseline.velocity_x,
            baseline.velocity_y,
        ));
        assert_eq!(
            sequencer.queue_lifecycle_intent(PendingLifecycleRequest::Frequency(2)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.record_frequency_request_submitted(2, 0, baseline),
            ClientAuthorityDisposition::Applied
        );
        let ack = FrequencyCutoverAck {
            frequency: 2,
            position_sequence_base: 0,
            life_seq: 42,
            direction: crate::continuum_position::protocol_direction(0.4075),
            x: 700,
            y: 800,
            velocity_x: crate::continuum_position::protocol_velocity_component(9.025),
            velocity_y: crate::continuum_position::protocol_velocity_component(10.025),
            togglables: 2,
            bounty: 11,
            energy: 1200,
            canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
                700.25, 800.25, 9.025, 10.025, 0.4075,
            ),
        };
        // Feed the sequencer only a packet accepted by production decoding.
        // Exact subpixel/sub-bin state must survive the legacy projection.
        let mut wire = vec![FrequencyCutoverAck::TYPE];
        wire.extend(ack.frequency.to_le_bytes());
        wire.extend(ack.position_sequence_base.to_le_bytes());
        wire.extend(ack.life_seq.to_le_bytes());
        wire.push(ack.direction);
        wire.extend(ack.x.to_le_bytes());
        wire.extend(ack.y.to_le_bytes());
        wire.extend(ack.velocity_x.to_le_bytes());
        wire.extend(ack.velocity_y.to_le_bytes());
        wire.push(ack.togglables);
        wire.extend(ack.bounty.to_le_bytes());
        wire.extend(ack.energy.to_le_bytes());
        ack.canonical_state.encode(&mut wire);
        let decoded = FrequencyCutoverAck::parse(&wire).expect("valid wire/canonical fixture");
        assert_eq!(decoded, ack);
        assert_eq!(
            sequencer.observe_frequency_ack(decoded),
            ClientAuthorityDisposition::Replacement
        );
        let accepted = sequencer.lifecycle_baseline_to_submit().unwrap();
        assert_eq!(accepted.canonical_state, ack.canonical_state);
        assert_eq!((accepted.x, accepted.y, accepted.energy), (700, 800, 1200));
        assert_eq!(
            sequencer.record_lifecycle_baseline_submitted(9, 0, Some(accepted.clone())),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.observe_lifecycle_receipt(LifecycleBaselineAccepted {
                client_reliable_delivery_id: 9,
                life_seq: 41,
                position_sequence: 0,
                accepted_server_tick: 8_100,
                accepted_position: None,
            }),
            LifecycleReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch)
        );
        assert_eq!(sequencer.view().live_scope(), None);
        assert_eq!(
            sequencer.observe_lifecycle_receipt(LifecycleBaselineAccepted {
                client_reliable_delivery_id: 9,
                life_seq: 42,
                position_sequence: 0,
                accepted_server_tick: 8_100,
                accepted_position: None,
            }),
            LifecycleReceiptOutcome::Applied {
                kind: ClientAuthorityTransitionKind::Frequency,
                accepted_baseline: Some(accepted),
            }
        );
    }

    #[test]
    fn unsent_cutover_owns_no_deadline_but_submitted_request_is_bounded() {
        let mut sequencer = live_sequencer();
        sequencer.queue_lifecycle_intent(PendingLifecycleRequest::Ship(1));
        for _ in 0..20 {
            assert_eq!(
                sequencer.advance_lifecycle_deadline(1.0, 5.0),
                ClientAuthorityDisposition::Observed
            );
        }
        assert_eq!(sequencer.view().deadline_elapsed_seconds, None);
        sequencer.record_ship_request_submitted(1, 7);
        assert_eq!(
            sequencer.advance_lifecycle_deadline(5.0, 5.0),
            ClientAuthorityDisposition::TimedOut
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Failed);
    }

    #[test]
    fn death_installs_exact_dead_successor_and_respawn_receipt_reopens_once() {
        let mut sequencer = live_sequencer();
        let dead_successor = AuthorityScopeIdentity {
            life_seq: Some(42),
            alive: Some(false),
            ..live_scope()
        };
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Reliable,
                dead_successor,
                Some(u32::MAX),
                2.0,
            ),
            ClientAuthorityDisposition::Replacement
        );
        let view = sequencer.view();
        assert_eq!(view.phase, ClientAuthorityPhase::DeadRespawning);
        assert_eq!(view.scope, dead_successor);
        assert_eq!(view.respawn_at_tick, Some(u32::MAX));
        assert_eq!(view.respawn_remaining_seconds, 2.0);
        assert_eq!(view.live_scope(), None);

        assert_eq!(
            sequencer.advance_respawn_eligibility(1.5),
            ClientAuthorityDisposition::Observed
        );
        assert_eq!(sequencer.view().respawn_remaining_seconds, 0.5);
        assert_eq!(
            sequencer.advance_respawn_eligibility(0.5),
            ClientAuthorityDisposition::Applied
        );
        assert!(sequencer.view().lifecycle_baseline_ready_to_submit);
        assert_eq!(
            sequencer.record_lifecycle_baseline_submitted(0, 0, None),
            ClientAuthorityDisposition::Applied
        );
        let receipt = LifecycleBaselineAccepted {
            client_reliable_delivery_id: 0,
            life_seq: 42,
            position_sequence: 0,
            accepted_server_tick: 9_000,
            accepted_position: None,
        };
        assert_eq!(
            sequencer.observe_lifecycle_receipt(receipt),
            LifecycleReceiptOutcome::Applied {
                kind: ClientAuthorityTransitionKind::Respawn,
                accepted_baseline: None,
            }
        );
        assert_eq!(
            sequencer.view().live_scope(),
            Some(AuthorityScopeIdentity {
                alive: Some(true),
                ..dead_successor
            })
        );
        assert_eq!(
            sequencer.observe_lifecycle_receipt(receipt),
            LifecycleReceiptOutcome::Duplicate
        );
    }

    #[test]
    fn respawn_snapshot_may_overtake_receipt_but_cannot_open_gameplay() {
        let mut sequencer = live_sequencer();
        let dead_successor = AuthorityScopeIdentity {
            life_seq: Some(42),
            alive: Some(false),
            ..live_scope()
        };
        sequencer.observe_private_lifecycle(
            ClientAuthoritySource::Event,
            dead_successor,
            Some(50),
            0.0,
        );
        sequencer.advance_respawn_eligibility(0.0);
        sequencer.record_lifecycle_baseline_submitted(19, u16::MAX, None);
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Realtime,
                AuthorityScopeIdentity {
                    alive: Some(true),
                    ..dead_successor
                },
                None,
                0.0,
            ),
            ClientAuthorityDisposition::Observed
        );
        assert_eq!(sequencer.view().live_scope(), None);
        assert_eq!(
            sequencer.observe_lifecycle_receipt(LifecycleBaselineAccepted {
                client_reliable_delivery_id: 19,
                life_seq: 42,
                position_sequence: u16::MAX,
                accepted_server_tick: 9_100,
                accepted_position: None,
            }),
            LifecycleReceiptOutcome::Applied {
                kind: ClientAuthorityTransitionKind::Respawn,
                accepted_baseline: None,
            }
        );
        assert!(sequencer.view().live_scope().is_some());
    }

    #[test]
    fn death_cancels_an_unacknowledged_cutover_and_rejects_predecessor_receipt() {
        let mut sequencer = live_sequencer();
        sequencer.queue_lifecycle_intent(PendingLifecycleRequest::Ship(6));
        sequencer.record_ship_request_submitted(6, 91);
        let dead_successor = AuthorityScopeIdentity {
            life_seq: Some(42),
            alive: Some(false),
            ..live_scope()
        };
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Reliable,
                dead_successor,
                Some(700),
                0.0,
            ),
            ClientAuthorityDisposition::Replacement
        );
        sequencer.advance_respawn_eligibility(0.0);
        sequencer.record_lifecycle_baseline_submitted(92, 0, None);
        assert_eq!(
            sequencer.observe_lifecycle_receipt(LifecycleBaselineAccepted {
                client_reliable_delivery_id: 91,
                life_seq: 42,
                position_sequence: 91,
                accepted_server_tick: 9_200,
                accepted_position: None,
            }),
            LifecycleReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch)
        );
        assert_eq!(
            sequencer.view().transition_kind,
            Some(ClientAuthorityTransitionKind::Respawn)
        );
    }

    #[test]
    fn correction_owns_submission_terminal_and_retained_actions_exactly_once() {
        let mut sequencer = live_sequencer();
        let proposal = correction(u16::MAX, u32::MAX);
        assert_eq!(
            sequencer.observe_movement_correction(proposal, 1, false),
            ClientAuthorityDisposition::Applied
        );
        let application = sequencer.correction_application_to_submit().unwrap();
        assert_eq!(application.detected_sequence, u16::MAX);
        assert_eq!(application.application_sequence, 0);
        assert_eq!(
            sequencer.retain_correction_action(retained_fire_action()),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.record_correction_application_submitted(u32::MAX),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.advance_lifecycle_deadline(1.0, 10.0),
            ClientAuthorityDisposition::Observed
        );
        let terminal =
            correction_terminal(application, MovementCorrectionAccepted::OUTCOME_APPLIED);
        assert_eq!(
            sequencer.observe_correction_terminal(terminal),
            CorrectionReceiptOutcome::Applied {
                receipt: terminal,
                proposal_reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
                repel_window: None,
            }
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        assert_eq!(sequencer.view().correction_retained_actions, 1);
        assert!(sequencer.take_released_correction_actions(false).is_empty());
        assert_eq!(sequencer.view().correction_retained_actions, 1);
        assert_eq!(
            sequencer.take_released_correction_actions(true),
            vec![retained_fire_action()]
        );
        assert_eq!(sequencer.view().correction_retained_actions, 0);
        assert_eq!(
            sequencer.observe_correction_terminal(terminal),
            CorrectionReceiptOutcome::Duplicate
        );
        assert!(sequencer.take_released_correction_actions(true).is_empty());
        assert_eq!(sequencer.view().correction_receipts_matched, 1);
    }

    #[test]
    fn menu_discards_delayed_weapon_edges_without_changing_authority() {
        let mut sequencer = live_sequencer();
        let scope = sequencer.view().scope;
        sequencer.retain_combat_view_actions(vec![retained_fire_action()]);
        sequencer
            .released_correction_actions
            .push_back(retained_fire_action());
        sequencer.discard_gameplay_actions_for_menu();
        assert!(sequencer.take_combat_view_actions(true).is_empty());
        assert!(sequencer.take_released_correction_actions(true).is_empty());
        assert_eq!(sequencer.view().scope, scope);
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        assert_eq!(sequencer.view().combat_view_actions_dropped, 1);
        assert_eq!(sequencer.view().correction_actions_dropped, 1);
    }

    #[test]
    fn combat_view_action_waits_for_readiness_and_releases_exactly_once() {
        let mut sequencer = live_sequencer();
        assert_eq!(
            sequencer.retain_combat_view_actions(vec![retained_fire_action()]),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(sequencer.view().combat_view_retained_actions, 1);
        assert!(sequencer.take_combat_view_actions(false).is_empty());
        assert_eq!(sequencer.view().combat_view_retained_actions, 1);
        assert_eq!(
            sequencer.take_combat_view_actions(true),
            vec![retained_fire_action()]
        );
        assert_eq!(sequencer.view().combat_view_retained_actions, 0);
        assert!(sequencer.take_combat_view_actions(true).is_empty());
        assert_eq!(sequencer.view().combat_view_actions_dropped, 0);
    }

    #[test]
    fn combat_view_action_cannot_cross_a_successor_life() {
        let mut sequencer = live_sequencer();
        assert_eq!(
            sequencer.retain_combat_view_actions(vec![retained_fire_action()]),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Reliable,
                AuthorityScopeIdentity {
                    life_seq: Some(42),
                    alive: Some(false),
                    ..live_scope()
                },
                Some(700),
                2.0,
            ),
            ClientAuthorityDisposition::Replacement
        );
        assert!(sequencer.take_combat_view_actions(true).is_empty());
        assert_eq!(sequencer.view().combat_view_retained_actions, 0);
        assert_eq!(sequencer.view().combat_view_actions_dropped, 1);
    }

    #[test]
    fn combat_view_action_capacity_rejects_a_whole_batch() {
        let mut sequencer = live_sequencer();
        let retained = vec![retained_fire_action(); crate::input::MAX_PENDING_SEMANTIC_ACTIONS - 1];
        assert_eq!(
            sequencer.retain_combat_view_actions(retained),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer
                .retain_combat_view_actions(vec![retained_fire_action(), retained_fire_action(),]),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::CapacityExceeded)
        );
        assert_eq!(
            sequencer.view().combat_view_retained_actions,
            crate::input::MAX_PENDING_SEMANTIC_ACTIONS - 1
        );
        assert_eq!(sequencer.view().combat_view_actions_dropped, 2);
    }

    #[test]
    fn correction_terminal_can_overtake_offer_without_rearming_it() {
        let mut sequencer = live_sequencer();
        let proposal = correction(70, 8);
        let application = MovementCorrectionApplied {
            intent: MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: proposal.server_session_id,
            player_id: proposal.player_id,
            life_seq: proposal.life_seq,
            correction_generation: proposal.correction_generation,
            detected_sequence: proposal.position_sequence,
            application_sequence: 74,
        };
        let terminal =
            correction_terminal(application, MovementCorrectionAccepted::OUTCOME_CANCELLED);
        assert_eq!(
            sequencer.observe_correction_terminal(terminal),
            CorrectionReceiptOutcome::Duplicate
        );
        assert_eq!(
            sequencer.observe_movement_correction(proposal, 75, false),
            ClientAuthorityDisposition::Duplicate
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        assert_eq!(sequencer.view().correction_terminal_outcomes, 1);
    }

    #[test]
    fn active_correction_terminal_cannot_complete_before_durable_submission() {
        let mut sequencer = live_sequencer();
        let proposal = correction(70, 8);
        assert_eq!(
            sequencer.observe_movement_correction(proposal, 75, false),
            ClientAuthorityDisposition::Applied
        );
        let application = sequencer.correction_application_to_submit().unwrap();
        let terminal =
            correction_terminal(application, MovementCorrectionAccepted::OUTCOME_APPLIED);
        assert_eq!(
            sequencer.observe_correction_terminal(terminal),
            CorrectionReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch)
        );
        assert_eq!(
            sequencer.view().transition_kind,
            Some(ClientAuthorityTransitionKind::Correction)
        );
        assert_eq!(
            sequencer.record_correction_application_submitted(44),
            ClientAuthorityDisposition::Applied
        );
        assert!(matches!(
            sequencer.observe_correction_terminal(terminal),
            CorrectionReceiptOutcome::Applied { .. }
        ));
    }

    #[test]
    fn released_correction_actions_remain_globally_bounded_across_terminals() {
        let mut sequencer = live_sequencer();
        for (position_sequence, generation, next_sequence, delivery_id) in
            [(70, 8, 75, 44), (80, 9, 85, 45)]
        {
            assert_eq!(
                sequencer.observe_movement_correction(
                    correction(position_sequence, generation),
                    next_sequence,
                    false,
                ),
                ClientAuthorityDisposition::Applied
            );
            for _ in 0..crate::input::MAX_PENDING_SEMANTIC_ACTIONS {
                assert_eq!(
                    sequencer.retain_correction_action(retained_fire_action()),
                    ClientAuthorityDisposition::Applied
                );
            }
            let application = sequencer.correction_application_to_submit().unwrap();
            sequencer.record_correction_application_submitted(delivery_id);
            assert!(matches!(
                sequencer.observe_correction_terminal(correction_terminal(
                    application,
                    MovementCorrectionAccepted::OUTCOME_APPLIED,
                )),
                CorrectionReceiptOutcome::Applied { .. }
            ));
        }
        assert_eq!(
            sequencer.take_released_correction_actions(true).len(),
            crate::input::MAX_PENDING_SEMANTIC_ACTIONS
        );
        assert_eq!(
            sequencer.view().correction_actions_dropped as usize,
            crate::input::MAX_PENDING_SEMANTIC_ACTIONS
        );
    }

    #[test]
    fn death_cancels_correction_and_drops_old_life_actions() {
        let mut sequencer = live_sequencer();
        let proposal = correction(88, 9);
        sequencer.observe_movement_correction(proposal, 90, false);
        sequencer.retain_correction_action(retained_fire_action());
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Reliable,
                AuthorityScopeIdentity {
                    life_seq: Some(42),
                    alive: Some(false),
                    ..live_scope()
                },
                Some(700),
                2.0,
            ),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(
            sequencer.view().transition_kind,
            Some(ClientAuthorityTransitionKind::Respawn)
        );
        assert_eq!(sequencer.view().correction_actions_dropped, 1);
        assert!(sequencer.take_released_correction_actions(true).is_empty());
        assert_eq!(
            sequencer.observe_movement_correction(proposal, 90, false),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
        );
    }

    #[test]
    fn correction_deadline_begins_only_after_durable_submission() {
        let mut sequencer = live_sequencer();
        let proposal = correction(9, 10);
        sequencer.observe_movement_correction(proposal, 11, false);
        sequencer.retain_correction_action(retained_fire_action());
        for _ in 0..20 {
            assert_eq!(
                sequencer.advance_lifecycle_deadline(1.0, 5.0),
                ClientAuthorityDisposition::Observed
            );
        }
        assert_eq!(sequencer.view().deadline_elapsed_seconds, None);
        sequencer.record_correction_application_submitted(17);
        assert_eq!(
            sequencer.advance_lifecycle_deadline(5.0, 5.0),
            ClientAuthorityDisposition::TimedOut
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Failed);
        assert_eq!(sequencer.view().correction_actions_dropped, 1);
        assert!(sequencer.take_released_correction_actions(true).is_empty());
    }

    #[test]
    fn brick_overlap_correction_uses_the_normal_receipt_barrier() {
        let mut sequencer = live_sequencer();
        let proposal = MovementCorrection {
            reason: MovementCorrection::REASON_BRICK_OVERLAP,
            repel_window: None,
            ..correction(u16::MAX, 3)
        };
        assert_eq!(MovementCorrection::parse(&proposal.encode()), Ok(proposal));
        assert_eq!(
            sequencer.observe_movement_correction(proposal, 1, false),
            ClientAuthorityDisposition::Applied
        );
        let application = sequencer.correction_application_to_submit().unwrap();
        assert_eq!(application.application_sequence, 0);
        sequencer.record_correction_application_submitted(42);
        let receipt = correction_terminal(application, MovementCorrectionAccepted::OUTCOME_APPLIED);
        assert_eq!(
            sequencer.observe_correction_terminal(receipt),
            CorrectionReceiptOutcome::Applied {
                receipt,
                proposal_reason: MovementCorrection::REASON_BRICK_OVERLAP,
                repel_window: None,
            }
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
    }

    #[test]
    fn correction_requires_exact_live_scope_reason_and_terminal_identity() {
        for invalid in [
            MovementCorrection {
                server_session_id: 8,
                ..correction(17, 3)
            },
            MovementCorrection {
                player_id: 24,
                ..correction(17, 3)
            },
            MovementCorrection {
                life_seq: 42,
                ..correction(17, 3)
            },
            MovementCorrection {
                correction_generation: 0,
                ..correction(17, 3)
            },
            MovementCorrection {
                reason: 0xff,
                ..correction(17, 3)
            },
        ] {
            let mut sequencer = live_sequencer();
            assert_eq!(
                sequencer.observe_movement_correction(invalid, 41, false),
                ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::ScopeMismatch)
            );
            assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
        }

        let proposal = correction(17, 3);
        let mut sequencer = live_sequencer();
        sequencer.observe_movement_correction(proposal, 41, false);
        let application = sequencer.correction_application_to_submit().unwrap();
        sequencer.record_correction_application_submitted(55);
        let receipt = correction_terminal(application, MovementCorrectionAccepted::OUTCOME_APPLIED);
        for forged in [
            MovementCorrectionAccepted {
                server_session_id: receipt.server_session_id + 1,
                ..receipt
            },
            MovementCorrectionAccepted {
                player_id: receipt.player_id + 1,
                ..receipt
            },
            MovementCorrectionAccepted {
                life_seq: receipt.life_seq + 1,
                ..receipt
            },
            MovementCorrectionAccepted {
                correction_generation: receipt.correction_generation + 1,
                ..receipt
            },
            MovementCorrectionAccepted {
                detected_sequence: receipt.detected_sequence + 1,
                ..receipt
            },
            MovementCorrectionAccepted {
                application_sequence: receipt.application_sequence + 1,
                ..receipt
            },
        ] {
            assert_eq!(
                sequencer.observe_correction_terminal(forged),
                CorrectionReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch)
            );
            assert_eq!(
                sequencer.view().transition_kind,
                Some(ClientAuthorityTransitionKind::Correction)
            );
        }
        assert!(matches!(
            sequencer.observe_correction_terminal(receipt),
            CorrectionReceiptOutcome::Applied { .. }
        ));
    }

    #[test]
    fn newer_same_life_boundary_requests_exact_cancel_terminal() {
        let mut sequencer = live_sequencer();
        let proposal = correction(70, 4);
        sequencer.observe_movement_correction(proposal, 75, true);
        let application = sequencer.correction_application_to_submit().unwrap();
        assert_eq!(
            application.intent,
            MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY
        );
        assert_eq!(application.detected_sequence, 70);
        assert_eq!(application.application_sequence, 74);
        sequencer.record_correction_application_submitted(56);

        let forged_applied =
            correction_terminal(application, MovementCorrectionAccepted::OUTCOME_APPLIED);
        assert_eq!(
            sequencer.observe_correction_terminal(forged_applied),
            CorrectionReceiptOutcome::Rejected(ClientAuthorityRejection::DeliveryMismatch)
        );
        let cancelled =
            correction_terminal(application, MovementCorrectionAccepted::OUTCOME_CANCELLED);
        assert_eq!(
            sequencer.observe_correction_terminal(cancelled),
            CorrectionReceiptOutcome::Cancelled
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Live);
    }

    #[test]
    fn correction_terminal_history_covers_the_reliable_delivery_window() {
        let mut sequencer = live_sequencer();
        let oldest_proposal = correction(9, 1);
        let oldest_application = MovementCorrectionApplied {
            intent: MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: oldest_proposal.server_session_id,
            player_id: oldest_proposal.player_id,
            life_seq: oldest_proposal.life_seq,
            correction_generation: oldest_proposal.correction_generation,
            detected_sequence: oldest_proposal.position_sequence,
            application_sequence: oldest_proposal.position_sequence,
        };
        let oldest = correction_terminal(
            oldest_application,
            MovementCorrectionAccepted::OUTCOME_CANCELLED,
        );
        sequencer.observe_correction_terminal(oldest);
        for offset in 1..=257_u32 {
            sequencer.observe_correction_terminal(MovementCorrectionAccepted {
                correction_generation: oldest.correction_generation.wrapping_add(offset),
                ..oldest
            });
        }
        assert!(sequencer.correction_terminals.contains(&oldest));
        assert_eq!(sequencer.view().correction_terminal_outcomes, 258);
        assert!(MAX_CORRECTION_TERMINALS >= 4_096);
    }

    #[test]
    fn deferred_authority_actions_are_exactly_bounded_and_overflow_fails_closed() {
        let mut sequencer = live_sequencer();
        let actions = ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![ClientProtocolEvent::KeepAlive; MAX_DEFERRED_AUTHORITY_ACTIONS],
            authority_admission: vec![
                AuthorityEventAdmission::default();
                MAX_DEFERRED_AUTHORITY_ACTIONS
            ],
        };
        assert_eq!(sequencer.defer_authority_actions(actions), Ok(()));
        assert_eq!(
            sequencer.view().deferred_authority_actions,
            MAX_DEFERRED_AUTHORITY_ACTIONS
        );

        let retained = sequencer.take_deferred_authority_actions();
        assert_eq!(retained.events.len(), MAX_DEFERRED_AUTHORITY_ACTIONS);
        assert_eq!(sequencer.view().deferred_authority_actions, 0);
        assert_eq!(
            sequencer.restore_deferred_authority_actions(retained),
            Ok(())
        );
        assert_eq!(
            sequencer.defer_authority_actions(ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![ClientProtocolEvent::KeepAlive],
                authority_admission: vec![AuthorityEventAdmission::default()],
            }),
            Err(ClientAuthorityRejection::CapacityExceeded)
        );
        let view = sequencer.view();
        assert_eq!(view.phase, ClientAuthorityPhase::Failed);
        assert_eq!(view.deferred_authority_actions, 0);
        assert_eq!(view.deferred_authority_action_overflows, 1);
        assert_eq!(
            sequencer.ledger().back().map(|entry| entry.fact),
            Some(ClientAuthorityFactKind::DeferredActionsOverflow)
        );
    }

    #[test]
    fn deferred_authority_actions_reject_unscoped_admission_vectors_atomically() {
        let mut sequencer = live_sequencer();
        assert_eq!(
            sequencer.defer_authority_actions(ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![ClientProtocolEvent::KeepAlive],
                authority_admission: Vec::new(),
            }),
            Err(ClientAuthorityRejection::AdmissionMismatch)
        );
        let view = sequencer.view();
        assert_eq!(view.phase, ClientAuthorityPhase::Failed);
        assert_eq!(view.deferred_authority_actions, 0);
        assert_eq!(view.deferred_authority_action_overflows, 1);
        assert_eq!(
            sequencer.ledger().back().map(|entry| entry.fact),
            Some(ClientAuthorityFactKind::DeferredActionsRejected)
        );
    }

    fn scoped_warp_pickup_ack(collectible_id: u32) -> ScopedPrizePickupAck {
        let scope = live_scope();
        ScopedPrizePickupAck {
            ack: PrizePickupAck {
                server_session_id: scope.server_session_id.unwrap(),
                arena_stream_id: scope.arena_stream_id.unwrap(),
                player_id: scope.player_id.unwrap(),
                collectible_id,
                x: 400,
                y: 500,
                prize_id: PrizeKind::Warp.protocol_id(),
                authoritative_energy: 900,
                life_seq: scope.life_seq.unwrap(),
                prize_state_revision: collectible_id,
                engine_shutdown_until_tick: None,
                engine_shutdown_application_required: false,
            },
            server_session_id: scope.server_session_id,
            arena_stream_id: scope.arena_stream_id,
            player_id: scope.player_id,
        }
    }

    #[test]
    fn warp_after_repel_barrier_is_bounded_deduplicated_and_life_scoped() {
        let mut sequencer = live_sequencer();
        let first = scoped_warp_pickup_ack(1);
        assert_eq!(
            sequencer.defer_warp_pickup_ack(first),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.defer_warp_pickup_ack(first),
            ClientAuthorityDisposition::Duplicate
        );
        assert_eq!(sequencer.view().deferred_warp_pickup_acks, 1);
        assert_eq!(sequencer.pop_deferred_warp_pickup_ack(), Some(first));
        assert_eq!(sequencer.view().deferred_warp_pickup_acks, 0);

        assert_eq!(
            sequencer.defer_warp_pickup_ack(first),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.observe_private_lifecycle(
                ClientAuthoritySource::Reliable,
                AuthorityScopeIdentity {
                    life_seq: Some(live_scope().life_seq.unwrap().wrapping_add(1)),
                    alive: Some(false),
                    ..live_scope()
                },
                Some(700),
                2.0,
            ),
            ClientAuthorityDisposition::Replacement
        );
        assert_eq!(sequencer.view().deferred_warp_pickup_acks, 0);
    }

    #[test]
    fn impossible_unique_warp_barrier_overflow_fails_closed() {
        let mut sequencer = live_sequencer();
        for collectible_id in 1..=MAX_DEFERRED_WARP_PICKUP_ACKS as u32 {
            assert_eq!(
                sequencer.defer_warp_pickup_ack(scoped_warp_pickup_ack(collectible_id)),
                ClientAuthorityDisposition::Applied
            );
        }
        assert_eq!(
            sequencer.defer_warp_pickup_ack(scoped_warp_pickup_ack(
                MAX_DEFERRED_WARP_PICKUP_ACKS as u32 + 1,
            )),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::CapacityExceeded)
        );
        let view = sequencer.view();
        assert_eq!(view.phase, ClientAuthorityPhase::Failed);
        assert_eq!(view.deferred_warp_pickup_acks, 0);
        assert_eq!(view.deferred_warp_pickup_ack_overflows, 1);
    }

    fn prize_application(collectible_id: u32, sequence: u16) -> PrizeEffectApplied {
        let scope = live_scope();
        PrizeEffectApplied {
            server_session_id: scope.server_session_id.unwrap(),
            player_id: scope.player_id.unwrap(),
            life_seq: scope.life_seq.unwrap(),
            collectible_id,
            prize_id: PrizeKind::Rotation.protocol_id(),
            prize_state_revision: collectible_id,
            application_sequence: sequence,
        }
    }

    fn prize_terminal(
        application: PrizeEffectApplied,
        application_sequence: u16,
        outcome: u8,
    ) -> PrizeEffectAccepted {
        PrizeEffectAccepted {
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            collectible_id: application.collectible_id,
            prize_id: application.prize_id,
            prize_state_revision: application.prize_state_revision,
            requested_application_sequence: application.application_sequence,
            application_sequence,
            outcome,
        }
    }

    #[test]
    fn prize_application_barrier_owns_submission_rebase_terminal_and_deadline() {
        let mut sequencer = live_sequencer();
        let original = prize_application(17, u16::MAX);
        assert_eq!(
            sequencer.arm_prize_effect_application(original, Some(500)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(sequencer.view().live_scope(), Some(live_scope()));
        assert_eq!(sequencer.view().gameplay_scope(), None);
        assert_eq!(
            sequencer.advance_application_barrier_deadline(100.0, 10.0),
            None,
            "an unsent barrier owns no application deadline"
        );
        assert_eq!(
            sequencer.record_prize_effect_application_submitted(original, 71),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.advance_application_barrier_deadline(9.0, 10.0),
            None
        );

        let rebase = prize_terminal(original, 0, PrizeEffectAccepted::OUTCOME_REBASE);
        assert_eq!(
            sequencer.observe_prize_effect_terminal(rebase),
            PrizeEffectReceiptOutcome::Rebased
        );
        assert_eq!(
            sequencer.observe_prize_effect_terminal(rebase),
            PrizeEffectReceiptOutcome::Duplicate
        );
        let rebased = PrizeEffectApplied {
            application_sequence: 0,
            ..original
        };
        assert_eq!(
            sequencer.record_prize_effect_application_submitted(rebased, 72),
            ClientAuthorityDisposition::Applied
        );
        let applied = prize_terminal(rebased, 0, PrizeEffectAccepted::OUTCOME_APPLIED);
        assert_eq!(
            sequencer.observe_prize_effect_terminal(applied),
            PrizeEffectReceiptOutcome::Applied(ClientPrizeEffectApplication {
                application: rebased,
                engine_shutdown_until_tick: Some(500),
                operation: Some(DurableOperation {
                    delivery_id: 72,
                    sequence: 0,
                    elapsed_seconds: 0.0,
                }),
            })
        );
        assert_eq!(sequencer.view().gameplay_scope(), Some(live_scope()));
        assert_eq!(
            sequencer.arm_prize_effect_application(rebased, Some(500)),
            ClientAuthorityDisposition::Duplicate,
            "a terminal identity cannot re-arm itself"
        );
        let last = sequencer
            .view()
            .last_transition
            .expect("bounded ledger entry");
        assert_eq!(last.application_identity, Some(17));
        assert_eq!(last.operation_sequence, Some(0));
    }

    #[test]
    fn impossible_prize_application_barrier_overflow_fails_closed() {
        let mut sequencer = live_sequencer();
        for collectible_id in 1..=MAX_PRIZE_EFFECT_APPLICATION_BARRIERS as u32 {
            assert_eq!(
                sequencer.arm_prize_effect_application(
                    prize_application(collectible_id, collectible_id as u16),
                    None,
                ),
                ClientAuthorityDisposition::Applied
            );
        }
        assert_eq!(
            sequencer.arm_prize_effect_application(
                prize_application(MAX_PRIZE_EFFECT_APPLICATION_BARRIERS as u32 + 1, 0),
                None,
            ),
            ClientAuthorityDisposition::Rejected(ClientAuthorityRejection::CapacityExceeded)
        );
        let view = sequencer.view();
        assert_eq!(view.phase, ClientAuthorityPhase::Failed);
        assert_eq!(view.application_barriers, 0);
        assert_eq!(view.application_barrier_overflows, 1);
    }

    fn portal_warp_request() -> PortalWarpRequest {
        let scope = live_scope();
        PortalWarpRequest {
            force_random: false,
            server_session_id: scope.server_session_id.unwrap(),
            arena_stream_id: scope.arena_stream_id.unwrap(),
            player_id: scope.player_id.unwrap(),
            life_seq: scope.life_seq.unwrap(),
            action_id: u32::MAX,
            position_sequence: u16::MAX,
        }
    }

    fn portal_warp_terminal(request: PortalWarpRequest, outcome: u8) -> PortalWarpAccepted {
        PortalWarpAccepted {
            server_session_id: request.server_session_id,
            arena_stream_id: request.arena_stream_id,
            player_id: request.player_id,
            life_seq: request.life_seq,
            action_id: request.action_id,
            position_sequence: request.position_sequence,
            server_tick: 900,
            position_x_bits: 10.5_f32.to_bits(),
            position_y_bits: 20.5_f32.to_bits(),
            velocity_x_bits: 1.0_f32.to_bits(),
            velocity_y_bits: 2.0_f32.to_bits(),
            orientation_bits: 0.25_f32.to_bits(),
            energy: 300,
            used_portal: true,
            outcome,
        }
    }

    #[test]
    fn portal_warp_barrier_is_exact_idempotent_life_scoped_and_timed() {
        let mut sequencer = live_sequencer();
        let request = portal_warp_request();
        assert_eq!(
            sequencer.arm_portal_warp(request, 81),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(sequencer.view().gameplay_scope(), None);
        let mut wrong = portal_warp_terminal(request, PortalWarpAccepted::OUTCOME_APPLIED);
        wrong.life_seq = wrong.life_seq.wrapping_add(1);
        assert_eq!(
            sequencer.observe_portal_warp_terminal(wrong),
            PortalWarpReceiptOutcome::RejectedAuthority(ClientAuthorityRejection::ScopeMismatch)
        );
        assert!(sequencer.view().portal_warp_barrier);
        let terminal = portal_warp_terminal(request, PortalWarpAccepted::OUTCOME_APPLIED);
        assert_eq!(
            sequencer.observe_portal_warp_terminal(terminal),
            PortalWarpReceiptOutcome::Applied
        );
        assert_eq!(sequencer.view().gameplay_scope(), Some(live_scope()));
        assert_eq!(
            sequencer.observe_portal_warp_terminal(terminal),
            PortalWarpReceiptOutcome::Duplicate
        );
        assert_eq!(
            sequencer.arm_portal_warp(request, 82),
            ClientAuthorityDisposition::Duplicate,
            "a completed Portal Warp cannot re-arm"
        );

        let mut next = request;
        next.action_id = 1;
        next.position_sequence = 0;
        assert_eq!(
            sequencer.arm_portal_warp(next, 83),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            sequencer.advance_application_barrier_deadline(10.0, 10.0),
            Some(ClientAuthorityApplicationBarrierKind::PortalWarp)
        );
        assert_eq!(sequencer.view().phase, ClientAuthorityPhase::Failed);
        assert_eq!(sequencer.view().application_barriers, 0);
        let last = sequencer
            .view()
            .last_transition
            .expect("timeout ledger entry");
        assert_eq!(last.delivery_id, Some(83));
        assert_eq!(last.application_identity, Some(1));
        assert_eq!(last.operation_sequence, Some(0));
    }

    #[test]
    fn source_structure_enforces_one_mutable_client_authority_owner() {
        let game = include_str!("../game.rs");
        let browser = include_str!("network/browser.rs");
        let outbound = include_str!("network/outbound.rs");
        let lifecycle = include_str!("player/lifecycle.rs");
        let world = include_str!("../network/world.rs");
        let world_lifecycle = include_str!("../network/world/lifecycle.rs");

        for forbidden in [
            "network_authority_segment:",
            "network_server_session_id:",
            "network_arena_stream_id:",
            "last_self_server_lifecycle_life_seq:",
            "pending_ship_reset_position:",
            "pending_ship_reset_delivery_id:",
            "pending_ship_change:",
            "pending_frequency_change:",
            "pending_movement_correction:",
            "pending_lifecycle_request:",
            "pending_prize_effect_applications:",
            "pending_portal_warp:",
        ] {
            assert!(
                !game.contains(forbidden),
                "Game must not regain sequencer-owned field {forbidden}"
            );
        }
        for forbidden in [
            "authority_segment:",
            "self_player_id:",
            "joined_arena:",
            "authoritative_snapshot_epoch:",
        ] {
            assert!(
                !world.contains(forbidden),
                "NetworkWorld must remain presentation-only: {forbidden}"
            );
        }
        assert!(!world_lifecycle.contains("self.authority_segment"));
        assert!(!world_lifecycle.contains("self.joined_arena"));
        assert!(!browser.contains("self.pending_prize_effect_applications"));
        assert!(!browser.contains("self.pending_portal_warp"));
        assert!(!outbound.contains("self.pending_portal_warp"));
        assert!(lifecycle.contains("self.client_authority.view().gameplay_scope()"));
        assert!(!lifecycle.contains("self.pending_prize_effect_applications"));
        assert!(!lifecycle.contains("self.pending_portal_warp"));
    }
}
