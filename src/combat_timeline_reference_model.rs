//! Independent executable specification for the v1 combat-view policy.
//!
//! This test-only model deliberately does not call the production projectile,
//! interpolation, receipt, or lifecycle implementations. It consumes only the
//! frozen Phase 3 value/wire types and models their required behavior so later
//! ownership migrations have an external oracle.

#![allow(dead_code)]

use crate::authority_identity::PlayerRuntimeIdentity;
use crate::combat_timeline::{
    ACTOR_MOTION_HISTORY_RETENTION_TICKS, ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY,
    ActorMotionSampleV1, CombatViewReceiptV1, MotionSegmentIdentity, OBSERVER_DELAY_MAX_TICKS,
    OBSERVER_DELAY_MIN_TICKS, OBSERVER_DELAY_SLEW_INTERVAL_TICKS,
    OBSERVER_PRESENTATION_HOLD_GRACE_TICKS, OBSERVER_PRESENTATION_INPUT_PHASE_TICKS,
    OBSERVER_PRESENTATION_MAX_STEP_SECONDS, OBSERVER_PRESENTATION_RECOVERY_TICKS_PER_SECOND,
    OBSERVER_SCHEDULE_HISTORY_CAPACITY, OBSERVER_SCHEDULE_RETENTION_TICKS,
    OBSERVER_TIMELINE_MIN_GENERATION_TICKS, ObserverNetworkProfile, ObserverTimelineDirectiveV1,
    PRESENTATION_AUTHORITY_MAX_ERROR_Q8,
};
use std::collections::{BTreeMap, VecDeque};

const MAX_ACCEPTED_INPUT_DECISIONS: usize = 256;
const MAX_REFERENCE_COLLISIONS: usize = 4_096;

// Candidate sample-time model only; not a new production authority owner.
// One quantum is 1/300 s, representing both a 100 Hz server tick (3)
// and a 60 Hz validated movement step (5) without cumulative rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AcceptedSampleClockReference {
    segment: MotionSegmentIdentity,
    sequence: u16,
    source_quanta: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ServerTickUnwrapperReference {
    wire_tick: u32,
    elapsed_ticks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PublishedSampleReference {
    snapshot_id: u16,
    segment: MotionSegmentIdentity,
    sequence: u16,
    source_quanta: u64,
    emitted_quanta: u64,
    x_q8: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SamplePublicationReference {
    clock: AcceptedSampleClockReference,
    emitted: VecDeque<PublishedSampleReference>,
    capacity: usize,
}

impl SamplePublicationReference {
    fn publish(
        &mut self,
        snapshot_id: u16,
        now: u64,
        max_skew: u64,
        x_q8: i32,
    ) -> Result<PublishedSampleReference, SampleClockDisposition> {
        if let Some(last) = self.emitted.back() {
            if !sequence_is_newer(snapshot_id, last.snapshot_id) {
                return Err(SampleClockDisposition::MissingOrStaleStep);
            }
        }
        let SampleClockDisposition::Eligible(source_quanta) = self.clock.eligibility(now, max_skew)
        else {
            return Err(self.clock.eligibility(now, max_skew));
        };
        let sample = PublishedSampleReference {
            snapshot_id,
            segment: self.clock.segment,
            sequence: self.clock.sequence,
            source_quanta,
            emitted_quanta: now,
            x_q8,
        };
        self.emitted.push_back(sample);
        while self.emitted.len() > self.capacity {
            self.emitted.pop_front();
        }
        Ok(sample)
    }

    fn apply_authorized_boundary(&mut self, boundary: AcceptedSampleClockReference) {
        // Retained snapshot facts keep the source time that was actually sent.
        // Replacing the current clock cannot relabel old combat-view geometry.
        self.clock = boundary;
    }

    fn emitted_sample(&self, snapshot_id: u16) -> Option<PublishedSampleReference> {
        self.emitted
            .iter()
            .find(|s| s.snapshot_id == snapshot_id)
            .copied()
    }
}

impl ServerTickUnwrapperReference {
    fn observe(&mut self, wire_tick: u32) -> Option<u64> {
        // Independent modular arithmetic: the actual server clock is 31 bits.
        let modulus = 1_u64 << 31;
        if u64::from(wire_tick) >= modulus || u64::from(self.wire_tick) >= modulus {
            return None;
        }
        let forward = (u64::from(wire_tick) + modulus - u64::from(self.wire_tick)) % modulus;
        // Only monotonic observations of the SERVER clock enter this model.
        // An ambiguous half-range jump is not permission to advance an epoch.
        if forward >= modulus / 2 {
            return None;
        }
        self.wire_tick = wire_tick;
        self.elapsed_ticks += forward;
        Some(self.elapsed_ticks * 3)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleClockDisposition {
    Recorded(u64),
    Eligible(u64),
    NotYetDue(u64),
    NeedsExplicitBoundary,
    WrongSegment,
    MissingOrStaleStep,
}

impl AcceptedSampleClockReference {
    fn at_authorized_boundary(
        segment: MotionSegmentIdentity,
        sequence: u16,
        server_quanta: u64,
    ) -> Self {
        Self {
            segment,
            sequence,
            source_quanta: server_quanta,
        }
    }

    // Consume EACH proven replay step, not the unproven numeric difference
    // between two packet tails. Omission recovery must supply those witnesses.
    fn append_proven_step(
        &mut self,
        segment: MotionSegmentIdentity,
        sequence: u16,
    ) -> SampleClockDisposition {
        if segment != self.segment {
            return SampleClockDisposition::WrongSegment;
        }
        if sequence != self.sequence.wrapping_add(1) {
            return SampleClockDisposition::MissingOrStaleStep;
        }
        self.sequence = sequence;
        self.source_quanta += 5;
        SampleClockDisposition::Recorded(self.source_quanta)
    }

    fn eligibility(self, server_quanta: u64, max_skew_quanta: u64) -> SampleClockDisposition {
        if self.source_quanta.abs_diff(server_quanta) > max_skew_quanta {
            return SampleClockDisposition::NeedsExplicitBoundary;
        }
        if self.source_quanta > server_quanta {
            SampleClockDisposition::NotYetDue(self.source_quanta)
        } else {
            SampleClockDisposition::Eligible(self.source_quanta)
        }
    }

    fn recover_after_accepted_batch(&mut self, server_quanta: u64) -> bool {
        // The least-delayed observer cursor is now minus the minimum delay. Only
        // a proven tail older than that window needs a new timing anchor.
        // Missing deliveries and future samples alone never create a boundary.
        let delay = u64::from(OBSERVER_DELAY_MIN_TICKS) * 3;
        if server_quanta.saturating_sub(self.source_quanta) <= delay {
            return false;
        }
        self.source_quanta = server_quanta;
        true
    }
}

fn tick_diff(candidate: u32, reference: u32) -> i32 {
    // Independent signed distance in the server's 31-bit timestamp space.
    let modulus = 1_i64 << 31;
    let forward = (i64::from(candidate & 0x7fff_ffff) - i64::from(reference & 0x7fff_ffff))
        .rem_euclid(modulus);
    if forward >= modulus / 2 {
        (forward - modulus) as i32
    } else {
        forward as i32
    }
}

fn tick_reached(now: u32, deadline: u32) -> bool {
    tick_diff(now, deadline) >= 0
}

fn sequence_is_newer(candidate: u16, previous: u16) -> bool {
    let delta = candidate.wrapping_sub(previous);
    delta != 0 && delta < (1_u16 << 15)
}

fn sequence_reached(candidate: u16, boundary: u16) -> bool {
    candidate == boundary || sequence_is_newer(candidate, boundary)
}

fn snapshot_reached(candidate: u16, boundary: u16) -> bool {
    candidate == boundary || sequence_is_newer(candidate, boundary)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryUnavailable {
    Empty,
    WrongSegment,
    BeforeRetainedWindow,
    NoSampleAtOrBefore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryAppendDisposition {
    Applied,
    Duplicate,
    RejectedStaleTick,
    RejectedStaleSampleTime,
    RejectedRetiredSegment,
}

#[derive(Clone, Debug)]
struct MotionHistoryReference {
    samples: VecDeque<ActorMotionSampleV1>,
    current_segment: Option<MotionSegmentIdentity>,
    retired_segments: VecDeque<MotionSegmentIdentity>,
    evictions: u64,
}

impl MotionHistoryReference {
    fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            current_segment: None,
            retired_segments: VecDeque::new(),
            evictions: 0,
        }
    }

    fn append(&mut self, sample: ActorMotionSampleV1) -> HistoryAppendDisposition {
        if self.retired_segments.contains(&sample.segment) {
            return HistoryAppendDisposition::RejectedRetiredSegment;
        }
        if let Some(last) = self.samples.back().copied() {
            if sample == last {
                return HistoryAppendDisposition::Duplicate;
            }
            if sample.segment == last.segment && !tick_reached(sample.server_tick, last.server_tick)
            {
                return HistoryAppendDisposition::RejectedStaleTick;
            }
            let (new_tick, new_third) = sample.sample_time.parts();
            let (old_tick, old_third) = last.sample_time.parts();
            let ticks = tick_diff(new_tick, old_tick);
            if sample.segment == last.segment
                && (ticks == -(1 << 30)
                    || i64::from(ticks) * 3 + i64::from(new_third) - i64::from(old_third) < 0)
            {
                return HistoryAppendDisposition::RejectedStaleSampleTime;
            }
        }
        if self.current_segment != Some(sample.segment) {
            if let Some(retired) = self.current_segment.replace(sample.segment) {
                self.retired_segments.push_back(retired);
                while self.retired_segments.len() > ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY {
                    self.retired_segments.pop_front();
                }
            }
        }
        self.samples.push_back(sample);
        self.evict_for(sample.server_tick);
        HistoryAppendDisposition::Applied
    }

    fn evict_for(&mut self, newest_tick: u32) {
        while self.samples.len() > ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY
            || self.samples.front().is_some_and(|oldest| {
                tick_diff(newest_tick, oldest.server_tick)
                    > ACTOR_MOTION_HISTORY_RETENTION_TICKS as i32
            })
        {
            self.samples.pop_front();
            self.evictions = self.evictions.saturating_add(1);
        }
    }

    fn at_or_before(
        &self,
        segment: MotionSegmentIdentity,
        target_tick: u32,
    ) -> Result<ActorMotionSampleV1, HistoryUnavailable> {
        if self.samples.is_empty() {
            return Err(HistoryUnavailable::Empty);
        }
        if !self.samples.iter().any(|sample| sample.segment == segment) {
            return Err(HistoryUnavailable::WrongSegment);
        }
        if self.samples.iter().all(|sample| {
            sample.segment != segment || tick_diff(target_tick, sample.server_tick) < 0
        }) {
            let oldest = self
                .samples
                .iter()
                .find(|sample| sample.segment == segment)
                .expect("segment membership checked");
            return if tick_diff(target_tick, oldest.server_tick) < 0 {
                Err(HistoryUnavailable::BeforeRetainedWindow)
            } else {
                Err(HistoryUnavailable::NoSampleAtOrBefore)
            };
        }
        self.samples
            .iter()
            .rev()
            .copied()
            .find(|sample| {
                sample.segment == segment && tick_diff(target_tick, sample.server_tick) >= 0
            })
            .ok_or(HistoryUnavailable::NoSampleAtOrBefore)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReceiptEvidence {
    protocol_received: bool,
    decoded: bool,
    codec_acked: bool,
    game_applied: bool,
    presentable: bool,
    drawn: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EmittedReceiptScope {
    runtime: PlayerRuntimeIdentity,
    emitted_tick: u32,
    contains_source: Option<PlayerRuntimeIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientReceiptFact {
    ProtocolReceived,
    Decoded,
    CodecAcked,
    GameApplied,
    Presentable,
    Drawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReceiptDisposition {
    Applied,
    Duplicate,
    RejectedUnknownSnapshot,
    RejectedScope,
    RejectedMissingPrerequisite,
}

#[derive(Clone, Debug, Default)]
struct ReceiptReference {
    emitted: BTreeMap<u16, EmittedReceiptScope>,
    evidence: BTreeMap<u16, ReceiptEvidence>,
}

impl ReceiptReference {
    fn emit(&mut self, snapshot_id: u16, scope: EmittedReceiptScope) {
        self.emitted.insert(snapshot_id, scope);
        self.evidence.entry(snapshot_id).or_default();
    }

    fn observe_client(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        snapshot_id: u16,
        fact: ClientReceiptFact,
    ) -> ReceiptDisposition {
        let Some(scope) = self.emitted.get(&snapshot_id).copied() else {
            return ReceiptDisposition::RejectedUnknownSnapshot;
        };
        if scope.runtime != runtime {
            return ReceiptDisposition::RejectedScope;
        }
        let evidence = self
            .evidence
            .get_mut(&snapshot_id)
            .expect("emission creates evidence");
        let (already, prerequisite) = match fact {
            ClientReceiptFact::ProtocolReceived => (evidence.protocol_received, true),
            ClientReceiptFact::Decoded => (evidence.decoded, evidence.protocol_received),
            ClientReceiptFact::CodecAcked => (evidence.codec_acked, evidence.decoded),
            ClientReceiptFact::GameApplied => (evidence.game_applied, evidence.decoded),
            ClientReceiptFact::Presentable => (evidence.presentable, evidence.game_applied),
            ClientReceiptFact::Drawn => (evidence.drawn, evidence.presentable),
        };
        if already {
            return ReceiptDisposition::Duplicate;
        }
        if !prerequisite {
            return ReceiptDisposition::RejectedMissingPrerequisite;
        }
        match fact {
            ClientReceiptFact::ProtocolReceived => evidence.protocol_received = true,
            ClientReceiptFact::Decoded => evidence.decoded = true,
            ClientReceiptFact::CodecAcked => evidence.codec_acked = true,
            ClientReceiptFact::GameApplied => evidence.game_applied = true,
            ClientReceiptFact::Presentable => evidence.presentable = true,
            ClientReceiptFact::Drawn => evidence.drawn = true,
        }
        ReceiptDisposition::Applied
    }

    /// Server arrival order is not a strength ordering: an application receipt
    /// on reliable may overtake the periodic codec ACK. Both must name an exact
    /// emitted snapshot, but neither is inferred from the other.
    fn observe_server_wire_fact(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        snapshot_id: u16,
        fact: ClientReceiptFact,
    ) -> ReceiptDisposition {
        let Some(scope) = self.emitted.get(&snapshot_id).copied() else {
            return ReceiptDisposition::RejectedUnknownSnapshot;
        };
        if scope.runtime != runtime {
            return ReceiptDisposition::RejectedScope;
        }
        let evidence = self
            .evidence
            .get_mut(&snapshot_id)
            .expect("emission creates evidence");
        match fact {
            ClientReceiptFact::CodecAcked if evidence.codec_acked => ReceiptDisposition::Duplicate,
            ClientReceiptFact::CodecAcked => {
                evidence.codec_acked = true;
                ReceiptDisposition::Applied
            }
            ClientReceiptFact::GameApplied if evidence.game_applied => {
                ReceiptDisposition::Duplicate
            }
            ClientReceiptFact::GameApplied => {
                evidence.game_applied = true;
                ReceiptDisposition::Applied
            }
            _ => ReceiptDisposition::RejectedMissingPrerequisite,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PresentationSample {
    snapshot_id: u16,
    server_tick: u32,
    segment: MotionSegmentIdentity,
    x_q8: i32,
    y_q8: i32,
    velocity_x_q8: i32,
    velocity_y_q8: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectionDisposition {
    Exact,
    Interpolated,
    HeldNoUpperSample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectionUnavailable {
    NoAppliedSample,
    BeforeFirstSample,
    WrongSegment,
    AuthorityHistory(HistoryUnavailable),
    AuthorityErrorExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProjectionAnchor {
    disposition: ProjectionDisposition,
    render_tick: u32,
    lower_snapshot_id: u16,
    upper_snapshot_id: u16,
    x_q8: i32,
    y_q8: i32,
    authority_x_q8: i32,
    authority_y_q8: i32,
    authority_error_q8: i32,
}

#[derive(Clone, Debug, Default)]
struct PresentationGeometryReference {
    samples: Vec<PresentationSample>,
}

impl PresentationGeometryReference {
    fn push(&mut self, sample: PresentationSample) {
        if !self.samples.contains(&sample) {
            self.samples.push(sample);
        }
    }

    fn resolve(
        &self,
        segment: MotionSegmentIdentity,
        render_tick: u32,
        applied_snapshot_id: u16,
        motion: &MotionHistoryReference,
    ) -> Result<ProjectionAnchor, ProjectionUnavailable> {
        let mut eligible = self
            .samples
            .iter()
            .copied()
            .filter(|sample| {
                sample.segment == segment
                    && snapshot_reached(applied_snapshot_id, sample.snapshot_id)
            })
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            return if self.samples.iter().any(|sample| {
                sample.segment == segment
                    && snapshot_reached(applied_snapshot_id, sample.snapshot_id)
            }) {
                Err(ProjectionUnavailable::NoAppliedSample)
            } else if self.samples.iter().any(|sample| sample.segment == segment) {
                Err(ProjectionUnavailable::NoAppliedSample)
            } else {
                Err(ProjectionUnavailable::WrongSegment)
            };
        }
        eligible.sort_by_key(|sample| tick_diff(sample.server_tick, render_tick));
        let lower = eligible
            .iter()
            .copied()
            .filter(|sample| tick_diff(render_tick, sample.server_tick) >= 0)
            .min_by_key(|sample| tick_diff(render_tick, sample.server_tick))
            .ok_or(ProjectionUnavailable::BeforeFirstSample)?;
        let upper = eligible
            .iter()
            .copied()
            .filter(|sample| tick_diff(sample.server_tick, render_tick) >= 0)
            .min_by_key(|sample| tick_diff(sample.server_tick, render_tick));
        let (upper, disposition, x_q8, y_q8) = match upper {
            Some(upper) if upper.server_tick == lower.server_tick => {
                (upper, ProjectionDisposition::Exact, lower.x_q8, lower.y_q8)
            }
            Some(upper) => {
                let (x, y) = reference_hermite(lower, upper, render_tick);
                (upper, ProjectionDisposition::Interpolated, x, y)
            }
            None => (
                lower,
                ProjectionDisposition::HeldNoUpperSample,
                lower.x_q8,
                lower.y_q8,
            ),
        };
        let authority = motion
            .at_or_before(segment, render_tick)
            .map_err(ProjectionUnavailable::AuthorityHistory)?;
        let dx = i64::from(x_q8) - i64::from(authority.x_q8);
        let dy = i64::from(y_q8) - i64::from(authority.y_q8);
        let error_q8 = ((dx * dx + dy * dy) as f64).sqrt().round() as i32;
        if error_q8 > PRESENTATION_AUTHORITY_MAX_ERROR_Q8 {
            return Err(ProjectionUnavailable::AuthorityErrorExceeded);
        }
        Ok(ProjectionAnchor {
            disposition,
            render_tick,
            lower_snapshot_id: lower.snapshot_id,
            upper_snapshot_id: upper.snapshot_id,
            x_q8,
            y_q8,
            authority_x_q8: authority.x_q8,
            authority_y_q8: authority.y_q8,
            authority_error_q8: error_q8,
        })
    }
}

fn reference_hermite(
    lower: PresentationSample,
    upper: PresentationSample,
    render_tick: u32,
) -> (i32, i32) {
    let span_ticks = tick_diff(upper.server_tick, lower.server_tick).max(1) as f64;
    let elapsed_ticks =
        tick_diff(render_tick, lower.server_tick).clamp(0, span_ticks as i32) as f64;
    let t = (elapsed_ticks / span_ticks).clamp(0.0, 1.0);
    let p0 = (f64::from(lower.x_q8), f64::from(lower.y_q8));
    let p1 = (f64::from(upper.x_q8), f64::from(upper.y_q8));
    let seconds = span_ticks * 0.01;
    let m0 = clamp_tangent(
        f64::from(lower.velocity_x_q8) * seconds,
        f64::from(lower.velocity_y_q8) * seconds,
        p1.0 - p0.0,
        p1.1 - p0.1,
    );
    let m1 = clamp_tangent(
        f64::from(upper.velocity_x_q8) * seconds,
        f64::from(upper.velocity_y_q8) * seconds,
        p1.0 - p0.0,
        p1.1 - p0.1,
    );
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    (
        (p0.0 * h00 + m0.0 * h10 + p1.0 * h01 + m1.0 * h11).round() as i32,
        (p0.1 * h00 + m0.1 * h10 + p1.1 * h01 + m1.1 * h11).round() as i32,
    )
}

fn clamp_tangent(vx: f64, vy: f64, chord_x: f64, chord_y: f64) -> (f64, f64) {
    let chord = (chord_x * chord_x + chord_y * chord_y).sqrt();
    let length = (vx * vx + vy * vy).sqrt();
    if chord <= f64::EPSILON || length <= f64::EPSILON {
        return (0.0, 0.0);
    }
    if length <= chord {
        (vx, vy)
    } else {
        (vx * chord / length, vy * chord / length)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EmittedSnapshotReference {
    runtime: PlayerRuntimeIdentity,
    snapshot_id: u16,
    server_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineRejection {
    WrongRuntime,
    WrongLife,
    UnknownGeneration,
    GenerationNotEffective,
    UnknownAppliedSnapshot,
    AppliedSnapshotFromOtherRuntime,
    AppliedSnapshotAheadOfInput,
    ApplicationTooFarBehind,
    HeldWeaponEdge,
    HoldExpired,
    StaleInput,
    DuplicateMismatch,
    GenerationTooSoon,
    EffectiveSequenceNotNewer,
    ClientSelectedCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimelineDecision {
    generation: u32,
    input_sequence: u16,
    server_tick: u32,
    render_tick: u32,
    delay_ticks: u8,
    applied_snapshot_id: u16,
    weapon_allowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AcceptedInputReference {
    receipt: CombatViewReceiptV1,
    decision: TimelineDecision,
}

#[derive(Clone, Debug)]
struct ObserverTimelineReference {
    observer: PlayerRuntimeIdentity,
    arena_stream_id: u64,
    profile: ObserverNetworkProfile,
    directives: VecDeque<ObserverTimelineDirectiveV1>,
    emitted: VecDeque<EmittedSnapshotReference>,
    accepted: VecDeque<AcceptedInputReference>,
    last_input_sequence: Option<u16>,
}

impl ObserverTimelineReference {
    fn new(
        observer: PlayerRuntimeIdentity,
        arena_stream_id: u64,
        profile: ObserverNetworkProfile,
        initial_generation: u32,
        now: u32,
        effective_sequence: u16,
    ) -> Self {
        assert_ne!(
            initial_generation, 0,
            "server allocator never issues generation zero"
        );
        let envelope = profile.delay_envelope();
        let directive = ObserverTimelineDirectiveV1 {
            generation: initial_generation,
            issued_server_tick: now,
            effective_server_tick: now,
            effective_from_input_sequence: effective_sequence,
            from_delay_ticks: envelope.target_ticks,
            target_delay_ticks: envelope.target_ticks,
            ceiling_delay_ticks: envelope.ceiling_ticks,
            slew_interval_ticks: OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
            hold_grace_ticks: OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
            profile,
        };
        Self {
            observer,
            arena_stream_id,
            profile,
            directives: VecDeque::from([directive]),
            emitted: VecDeque::new(),
            accepted: VecDeque::new(),
            last_input_sequence: None,
        }
    }

    fn current_generation(&self) -> u32 {
        self.directives
            .back()
            .expect("initial directive")
            .generation
    }

    fn observe_emitted(&mut self, snapshot_id: u16, server_tick: u32) {
        self.emitted.push_back(EmittedSnapshotReference {
            runtime: self.observer,
            snapshot_id,
            server_tick,
        });
        while self.emitted.len() > 64 {
            self.emitted.pop_front();
        }
    }

    fn issue_delay_target(
        &mut self,
        now: u32,
        effective_sequence: u16,
        requested_target: u8,
    ) -> Result<ObserverTimelineDirectiveV1, TimelineRejection> {
        let previous = *self.directives.back().expect("initial directive");
        if tick_diff(now, previous.issued_server_tick)
            < OBSERVER_TIMELINE_MIN_GENERATION_TICKS as i32
        {
            return Err(TimelineRejection::GenerationTooSoon);
        }
        if !sequence_is_newer(effective_sequence, previous.effective_from_input_sequence) {
            return Err(TimelineRejection::EffectiveSequenceNotNewer);
        }
        let envelope = self.profile.delay_envelope();
        let target = requested_target.clamp(OBSERVER_DELAY_MIN_TICKS, envelope.ceiling_ticks);
        let from = self.delay_at(now);
        let mut generation = previous.generation.wrapping_add(1);
        if generation == 0 {
            generation = 1;
        }
        let directive = ObserverTimelineDirectiveV1 {
            generation,
            issued_server_tick: now,
            effective_server_tick: now,
            effective_from_input_sequence: effective_sequence,
            from_delay_ticks: from,
            target_delay_ticks: target,
            ceiling_delay_ticks: envelope.ceiling_ticks,
            slew_interval_ticks: OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
            hold_grace_ticks: OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
            profile: self.profile,
        };
        directive.validate().expect("reference issues valid policy");
        self.directives.push_back(directive);
        self.evict_directives(now);
        Ok(directive)
    }

    fn evict_directives(&mut self, now: u32) {
        while self.directives.len() > OBSERVER_SCHEDULE_HISTORY_CAPACITY
            || (self.directives.len() > 1
                && self.directives.front().is_some_and(|directive| {
                    tick_diff(now, directive.issued_server_tick)
                        > OBSERVER_SCHEDULE_RETENTION_TICKS as i32
                }))
        {
            self.directives.pop_front();
        }
    }

    fn directive_for_tick(&self, tick: u32) -> ObserverTimelineDirectiveV1 {
        self.directives
            .iter()
            .rev()
            .copied()
            .find(|directive| tick_reached(tick, directive.effective_server_tick))
            .unwrap_or_else(|| *self.directives.front().expect("directive history"))
    }

    fn delay_at(&self, tick: u32) -> u8 {
        let directive = self.directive_for_tick(tick);
        let elapsed = tick_diff(tick, directive.effective_server_tick).max(0) as u32;
        let steps = elapsed / OBSERVER_DELAY_SLEW_INTERVAL_TICKS;
        if directive.target_delay_ticks >= directive.from_delay_ticks {
            directive
                .from_delay_ticks
                .saturating_add(steps.min(u32::from(u8::MAX)) as u8)
                .min(directive.target_delay_ticks)
        } else {
            directive
                .from_delay_ticks
                .saturating_sub(steps.min(u32::from(u8::MAX)) as u8)
                .max(directive.target_delay_ticks)
        }
    }

    fn directive_for_sequence(&self, sequence: u16) -> Option<ObserverTimelineDirectiveV1> {
        self.directives
            .iter()
            .rev()
            .copied()
            .find(|directive| sequence_reached(sequence, directive.effective_from_input_sequence))
    }

    fn verify_receipt(
        &mut self,
        authenticated_runtime: PlayerRuntimeIdentity,
        server_tick: u32,
        receipt: CombatViewReceiptV1,
        weapon_edge: bool,
    ) -> Result<TimelineDecision, TimelineRejection> {
        if authenticated_runtime != self.observer {
            return Err(TimelineRejection::WrongRuntime);
        }
        if receipt.observer_life_seq != self.observer.life.life_seq {
            return Err(TimelineRejection::WrongLife);
        }
        if let Some(existing) = self
            .accepted
            .iter()
            .find(|accepted| accepted.receipt.input_sequence == receipt.input_sequence)
            .copied()
        {
            return if existing.receipt == receipt {
                Ok(existing.decision)
            } else {
                Err(TimelineRejection::DuplicateMismatch)
            };
        }
        if self
            .last_input_sequence
            .is_some_and(|previous| !sequence_is_newer(receipt.input_sequence, previous))
        {
            return Err(TimelineRejection::StaleInput);
        }
        let directive = self
            .directive_for_sequence(receipt.input_sequence)
            .ok_or(TimelineRejection::GenerationNotEffective)?;
        if receipt.timeline_generation != directive.generation {
            return if self
                .directives
                .iter()
                .any(|candidate| candidate.generation == receipt.timeline_generation)
            {
                Err(TimelineRejection::GenerationNotEffective)
            } else {
                Err(TimelineRejection::UnknownGeneration)
            };
        }
        let emitted = self
            .emitted
            .iter()
            .find(|emitted| emitted.snapshot_id == receipt.latest_game_applied_snapshot_id)
            .copied()
            .ok_or(TimelineRejection::UnknownAppliedSnapshot)?;
        if emitted.runtime != authenticated_runtime {
            return Err(TimelineRejection::AppliedSnapshotFromOtherRuntime);
        }
        if !tick_reached(server_tick, emitted.server_tick) {
            return Err(TimelineRejection::AppliedSnapshotAheadOfInput);
        }
        let delay_ticks = self.delay_at(server_tick);
        let render_tick = server_tick.wrapping_sub(u32::from(delay_ticks));
        let application_age = tick_diff(render_tick, emitted.server_tick);
        if receipt.hold_or_stall() {
            if weapon_edge {
                return Err(TimelineRejection::HeldWeaponEdge);
            }
            if application_age > i32::from(OBSERVER_PRESENTATION_HOLD_GRACE_TICKS) {
                return Err(TimelineRejection::HoldExpired);
            }
        } else if application_age > i32::from(OBSERVER_PRESENTATION_HOLD_GRACE_TICKS) {
            return Err(TimelineRejection::ApplicationTooFarBehind);
        }
        let decision = TimelineDecision {
            generation: directive.generation,
            input_sequence: receipt.input_sequence,
            server_tick,
            render_tick,
            delay_ticks,
            applied_snapshot_id: receipt.latest_game_applied_snapshot_id,
            weapon_allowed: weapon_edge,
        };
        self.last_input_sequence = Some(receipt.input_sequence);
        self.accepted
            .push_back(AcceptedInputReference { receipt, decision });
        if self.accepted.len() > MAX_ACCEPTED_INPUT_DECISIONS {
            self.accepted.pop_front();
        }
        Ok(decision)
    }

    fn verify_client_selected_cursor(
        &self,
        _server_tick: u32,
        _claimed_render_tick: u32,
    ) -> Result<(), TimelineRejection> {
        Err(TimelineRejection::ClientSelectedCursor)
    }
}

/// Independent browser-scheduler model for the actually drawn cursor. The
/// server schedule supplies every target; browser time can only bound recovery
/// toward that target and therefore cannot manufacture rewind authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PresentationCursorReference {
    drawn_tick: Option<u32>,
    scheduled_tick: Option<u32>,
    fractional_budget: u64,
}

impl PresentationCursorReference {
    fn advance(&mut self, scheduled_tick: u32, render_dt_seconds: f32) -> u32 {
        self.scheduled_tick = Some(scheduled_tick);
        let Some(drawn_tick) = self.drawn_tick else {
            self.drawn_tick = Some(scheduled_tick);
            return scheduled_tick;
        };
        let distance = tick_diff(scheduled_tick, drawn_tick);
        if distance <= 0 {
            if distance < 0 {
                self.drawn_tick = Some(scheduled_tick);
                self.fractional_budget = 0;
            }
            return self.drawn_tick.expect("cursor initialized");
        }

        let bounded_dt = if render_dt_seconds.is_finite() {
            render_dt_seconds.clamp(0.0, OBSERVER_PRESENTATION_MAX_STEP_SECONDS)
        } else {
            0.0
        };
        // Model the two allowances independently: real elapsed time inside
        // the existing eight 60 Hz gameplay-step budget, plus a bounded
        // catch-up allowance. A longer suspension retains the old hitch bound.
        // These pinned model values do not call a production budget helper.
        let ordinary_dt =
            if render_dt_seconds.is_finite() && (0.0..=8.0 / 60.0).contains(&render_dt_seconds) {
                render_dt_seconds
            } else {
                bounded_dt
            };
        let ordinary_nanos = (f64::from(ordinary_dt) * 1_000_000_000.0)
            .round()
            .clamp(0.0, u64::MAX as f64) as u64;
        let recovery_nanos = (f64::from(bounded_dt) * 1_000_000_000.0).round() as u64;
        let budget = self.fractional_budget
            + ordinary_nanos * 100
            + recovery_nanos * (OBSERVER_PRESENTATION_RECOVERY_TICKS_PER_SECOND - 100);
        let available = budget / 1_000_000_000;
        self.fractional_budget = budget % 1_000_000_000;
        let advance = available.min(distance as u64) as u32;
        let next = drawn_tick.wrapping_add(advance);
        self.drawn_tick = Some(next);
        next
    }

    fn weapon_ready(self, scheduled_tick: u32) -> bool {
        self.drawn_tick == Some(scheduled_tick)
    }

    fn weapon_ready_for_input(self, current_scheduled_tick: u32) -> bool {
        let Some(frame_scheduled_tick) = self.scheduled_tick else {
            return false;
        };
        let phase_ticks = tick_diff(current_scheduled_tick, frame_scheduled_tick);
        self.drawn_tick == Some(frame_scheduled_tick)
            && (0..=OBSERVER_PRESENTATION_INPUT_PHASE_TICKS as i32).contains(&phase_ticks)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReferenceWeaponFamily {
    Bullet,
    BouncingBullet,
    Bomb,
    ProximityBomb,
    Mine,
    Shrapnel,
    Thor,
    Burst,
    RepelMutated,
}

fn reference_contact_boxes_overlap(
    first_x: f32,
    first_y: f32,
    first_half_extent: f32,
    second_x: f32,
    second_y: f32,
    second_half_extent: f32,
) -> bool {
    let first_x = first_x as i32 as f32;
    let first_y = first_y as i32 as f32;
    let second_x = second_x as i32 as f32;
    let second_y = second_y as i32 as f32;
    first_x + first_half_extent >= second_x - second_half_extent
        && first_x - first_half_extent < second_x + second_half_extent
        && first_y + first_half_extent >= second_y - second_half_extent
        && first_y - first_half_extent < second_y + second_half_extent
}

fn reference_direct_weapon_contact(
    weapon_x: f32,
    weapon_y: f32,
    ship_x: f32,
    ship_y: f32,
    ship_radius: f32,
) -> bool {
    reference_contact_boxes_overlap(ship_x, ship_y, ship_radius, weapon_x, weapon_y, 4.0)
}

fn reference_proximity_radius(proximity_distance: f32, level: u8) -> f32 {
    (proximity_distance + level.saturating_sub(1) as f32) * 18.0
}

fn reference_proximity_contact(
    weapon_x: f32,
    weapon_y: f32,
    ship_x: f32,
    ship_y: f32,
    sensor_radius: f32,
    ship_radius: f32,
) -> bool {
    reference_contact_boxes_overlap(
        ship_x,
        ship_y,
        ship_radius,
        weapon_x,
        weapon_y,
        (sensor_radius - 14.0).max(0.0),
    )
}

fn reference_radial_damage(distance: f32, radius: f32, full_damage: f32) -> u16 {
    if radius <= 0.0 || distance >= radius {
        return 0;
    }
    ((radius - distance) * (full_damage / radius))
        .trunc()
        .clamp(0.0, u16::MAX as f32) as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrozenCollisionWitness {
    projectile_id: u32,
    collision_tick: u32,
    source_segment: MotionSegmentIdentity,
    victim_segment: MotionSegmentIdentity,
    anchor_x_q8: i32,
    anchor_y_q8: i32,
    damage: u16,
    harmful_open_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrozenCollisionState {
    Pending,
    Committed,
    CancelledStaleLife,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrozenCollision {
    witness: FrozenCollisionWitness,
    state: FrozenCollisionState,
}

#[derive(Clone, Debug, Default)]
struct CollisionReference {
    fifo: VecDeque<FrozenCollision>,
}

impl CollisionReference {
    fn freeze(&mut self, witness: FrozenCollisionWitness) -> bool {
        if self.fifo.len() >= MAX_REFERENCE_COLLISIONS
            || self
                .fifo
                .iter()
                .any(|collision| collision.witness.projectile_id == witness.projectile_id)
        {
            return false;
        }
        self.fifo.push_back(FrozenCollision {
            witness,
            state: FrozenCollisionState::Pending,
        });
        true
    }

    fn advance_head(
        &mut self,
        now: u32,
        source: MotionSegmentIdentity,
        victim: MotionSegmentIdentity,
    ) -> Option<FrozenCollision> {
        let head = self.fifo.front_mut()?;
        if head.state != FrozenCollisionState::Pending {
            return Some(*head);
        }
        if head.witness.source_segment != source || head.witness.victim_segment != victim {
            head.state = FrozenCollisionState::CancelledStaleLife;
        } else if tick_reached(now, head.witness.harmful_open_tick) {
            head.state = FrozenCollisionState::Committed;
        }
        Some(*head)
    }

    fn pop_terminal(&mut self) -> Option<FrozenCollision> {
        if self
            .fifo
            .front()
            .is_some_and(|collision| collision.state != FrozenCollisionState::Pending)
        {
            self.fifo.pop_front()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority_identity::{LifeIdentity, PlayerSessionId, PlayerSessionIdentity};
    use crate::combat_timeline::{
        COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL, ObserverTimelineWireError,
    };

    fn runtime(player: u16, session: u64, life: u32) -> PlayerRuntimeIdentity {
        PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(player, PlayerSessionId::new(session)),
            LifeIdentity::new(player, life),
        )
    }

    fn segment(
        player: u16,
        session: u64,
        life: u32,
        stream: u64,
        discontinuity: u32,
    ) -> MotionSegmentIdentity {
        MotionSegmentIdentity {
            actor: runtime(player, session, life),
            arena_stream_id: stream,
            discontinuity_generation: discontinuity,
        }
    }

    fn motion_sample(
        segment: MotionSegmentIdentity,
        tick: u32,
        x_px: i32,
        y_px: i32,
    ) -> ActorMotionSampleV1 {
        ActorMotionSampleV1 {
            segment,
            server_tick: tick,
            sample_time: crate::combat_timeline::PresentationSampleTime::from_tick(tick),
            x_q8: x_px * 256,
            y_q8: y_px * 256,
            velocity_x_q8: 0,
            velocity_y_q8: 0,
            orientation_u16: 0,
            hull_radius_q8: 14 * 256,
            protocol_direction: 0,
            safe_or_attached: 0,
            source_kind: crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        }
    }

    fn receipt(
        observer: PlayerRuntimeIdentity,
        sequence: u16,
        generation: u32,
        snapshot_id: u16,
        hold: bool,
    ) -> CombatViewReceiptV1 {
        CombatViewReceiptV1 {
            observer_life_seq: observer.life.life_seq,
            input_sequence: sequence,
            timeline_generation: generation,
            latest_game_applied_snapshot_id: snapshot_id,
            flags: if hold {
                COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL
            } else {
                0
            },
        }
    }

    fn permutations<T: Copy, const N: usize>(mut values: [T; N]) -> Vec<[T; N]> {
        fn visit<T: Copy, const N: usize>(
            values: &mut [T; N],
            start: usize,
            output: &mut Vec<[T; N]>,
        ) {
            if start == N {
                output.push(*values);
                return;
            }
            for index in start..N {
                values.swap(start, index);
                visit(values, start + 1, output);
                values.swap(start, index);
            }
        }
        let mut output = Vec::new();
        visit(&mut values, 0, &mut output);
        output
    }

    #[derive(Clone, Copy)]
    struct DeterministicRng(u64);

    impl DeterministicRng {
        fn next_u32(&mut self) -> u32 {
            // This model needs reproducible schedules, not entropy.
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 32) as u32
        }
    }

    fn next_nonzero_snapshot_id(snapshot_id: &mut u16) -> u16 {
        *snapshot_id = snapshot_id.wrapping_add(1);
        if *snapshot_id == 0 {
            *snapshot_id = 1;
        }
        *snapshot_id
    }

    #[test]
    fn motion_history_is_exact_segment_wrap_safe_bounded_and_explicitly_unavailable() {
        let first = segment(7, 11, 3, 5, 1);
        let warped = segment(7, 11, 3, 5, 2);
        let successor = segment(7, 11, 4, 5, 0);
        let mut history = MotionHistoryReference::new();
        assert_eq!(
            history.append(motion_sample(first, 0x7fff_ffff - 10, 10, 20)),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(
            history.append(motion_sample(first, 5, 11, 20)),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(history.at_or_before(first, 5).unwrap().x_q8, 11 * 256);
        assert_eq!(
            history.append(motion_sample(warped, 1, 100, 100)),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(
            history.append(motion_sample(first, 7, 12, 20)),
            HistoryAppendDisposition::RejectedRetiredSegment
        );
        assert_eq!(
            history.at_or_before(successor, 7),
            Err(HistoryUnavailable::WrongSegment)
        );

        for offset in 0..=(ACTOR_MOTION_HISTORY_RETENTION_TICKS + 20) {
            let tick = 100_u32.wrapping_add(offset);
            assert!(matches!(
                history.append(motion_sample(successor, tick, offset as i32, 0)),
                HistoryAppendDisposition::Applied | HistoryAppendDisposition::Duplicate
            ));
        }
        assert!(history.samples.len() <= ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY);
        assert!(history.evictions > 0);
        assert_eq!(
            history.at_or_before(successor, 100),
            Err(HistoryUnavailable::BeforeRetainedWindow)
        );
        assert_eq!(
            history.append(motion_sample(warped, 1_000, 0, 0)),
            HistoryAppendDisposition::RejectedRetiredSegment,
            "ordinary sample eviction cannot make a retired discontinuity reusable"
        );
    }

    #[test]
    fn receipt_tiers_are_distinct_and_wire_arrival_does_not_invent_strength() {
        let observer = runtime(2, 9, 1);
        let other = runtime(3, 10, 1);
        let mut model = ReceiptReference::default();
        model.emit(
            41,
            EmittedReceiptScope {
                runtime: observer,
                emitted_tick: 100,
                contains_source: Some(other),
            },
        );
        assert_eq!(
            model.observe_client(observer, 41, ClientReceiptFact::GameApplied),
            ReceiptDisposition::RejectedMissingPrerequisite
        );
        for fact in [
            ClientReceiptFact::ProtocolReceived,
            ClientReceiptFact::Decoded,
            ClientReceiptFact::CodecAcked,
            ClientReceiptFact::GameApplied,
            ClientReceiptFact::Presentable,
            ClientReceiptFact::Drawn,
        ] {
            assert_eq!(
                model.observe_client(observer, 41, fact),
                ReceiptDisposition::Applied
            );
            assert_eq!(
                model.observe_client(observer, 41, fact),
                ReceiptDisposition::Duplicate
            );
        }
        let evidence = model.evidence[&41];
        assert!(evidence.codec_acked && evidence.game_applied && evidence.drawn);
        assert_eq!(
            model.observe_client(other, 41, ClientReceiptFact::Drawn),
            ReceiptDisposition::RejectedScope
        );

        for order in permutations([
            ClientReceiptFact::CodecAcked,
            ClientReceiptFact::GameApplied,
        ]) {
            let mut server = ReceiptReference::default();
            server.emit(
                41,
                EmittedReceiptScope {
                    runtime: observer,
                    emitted_tick: 100,
                    contains_source: Some(other),
                },
            );
            for fact in order {
                assert_eq!(
                    server.observe_server_wire_fact(observer, 41, fact),
                    ReceiptDisposition::Applied
                );
            }
            let evidence = server.evidence[&41];
            assert!(evidence.codec_acked && evidence.game_applied);
            assert!(!evidence.presentable && !evidence.drawn);
        }
    }

    #[test]
    fn projection_is_order_independent_uses_exact_segment_and_never_extrapolates() {
        let actor = segment(8, 12, 4, 7, 1);
        let mut motion = MotionHistoryReference::new();
        for (tick, x) in [(100, 0), (105, 5), (110, 10)] {
            assert_eq!(
                motion.append(motion_sample(actor, tick, x, 0)),
                HistoryAppendDisposition::Applied
            );
        }
        let lower = PresentationSample {
            snapshot_id: u16::MAX,
            server_tick: 100,
            segment: actor,
            x_q8: 0,
            y_q8: 0,
            velocity_x_q8: 100 * 256,
            velocity_y_q8: 0,
        };
        let upper = PresentationSample {
            snapshot_id: 1,
            server_tick: 110,
            segment: actor,
            x_q8: 10 * 256,
            y_q8: 0,
            velocity_x_q8: 100 * 256,
            velocity_y_q8: 0,
        };
        let mut expected = None;
        for order in permutations([lower, upper]) {
            let mut projection = PresentationGeometryReference::default();
            for sample in order {
                projection.push(sample);
            }
            let anchor = projection.resolve(actor, 105, 1, &motion).unwrap();
            assert_eq!(anchor.disposition, ProjectionDisposition::Interpolated);
            assert_eq!(anchor.x_q8, 5 * 256);
            assert_eq!(anchor.authority_error_q8, 0);
            if let Some(expected) = expected {
                assert_eq!(anchor, expected);
            } else {
                expected = Some(anchor);
            }
        }

        let mut held = PresentationGeometryReference::default();
        held.push(lower);
        let anchor = held.resolve(actor, 108, u16::MAX, &motion).unwrap();
        assert_eq!(anchor.disposition, ProjectionDisposition::HeldNoUpperSample);
        assert_eq!(anchor.x_q8, 0);

        let wrong_life = segment(8, 12, 5, 7, 0);
        assert_eq!(
            held.resolve(wrong_life, 108, u16::MAX, &motion),
            Err(ProjectionUnavailable::WrongSegment)
        );
    }

    #[test]
    fn projection_error_above_twelve_pixels_fails_closed_without_hitbox_expansion() {
        let actor = segment(8, 12, 4, 7, 1);
        let mut motion = MotionHistoryReference::new();
        motion.append(motion_sample(actor, 100, 0, 0));
        let mut projection = PresentationGeometryReference::default();
        projection.push(PresentationSample {
            snapshot_id: 9,
            server_tick: 100,
            segment: actor,
            x_q8: 13 * 256,
            y_q8: 0,
            velocity_x_q8: 0,
            velocity_y_q8: 0,
        });
        assert_eq!(
            projection.resolve(actor, 100, 9, &motion),
            Err(ProjectionUnavailable::AuthorityErrorExceeded)
        );
    }

    #[test]
    fn server_owned_delay_profiles_slew_monotonically_and_clamp_requests() {
        let observer = runtime(1, 55, 9);
        for profile in [
            ObserverNetworkProfile::CleanLan,
            ObserverNetworkProfile::SupportedMildWan,
            ObserverNetworkProfile::SupportedSlowWan,
            ObserverNetworkProfile::RecoveryStress,
            ObserverNetworkProfile::AutomaticWan,
        ] {
            let envelope = profile.delay_envelope();
            let mut model = ObserverTimelineReference::new(observer, 3, profile, 1, 1_000, 10);
            assert_eq!(model.delay_at(1_000), envelope.target_ticks);
            let directive = model
                .issue_delay_target(1_100, 20, u8::MAX)
                .expect("bounded generation");
            assert_eq!(directive.target_delay_ticks, envelope.ceiling_ticks);
            let mut previous = model.delay_at(1_100);
            for step in 1..=40 {
                let delay = model.delay_at(1_100 + step * OBSERVER_DELAY_SLEW_INTERVAL_TICKS);
                assert!(delay >= previous);
                assert!(delay - previous <= 1);
                assert!((OBSERVER_DELAY_MIN_TICKS..=envelope.ceiling_ticks).contains(&delay));
                previous = delay;
            }
            assert_eq!(previous, envelope.ceiling_ticks);
            assert_eq!(
                model.issue_delay_target(1_150, 21, OBSERVER_DELAY_MIN_TICKS),
                Err(TimelineRejection::GenerationTooSoon)
            );
        }
    }

    #[test]
    fn exact_receipt_verification_is_idempotent_wrap_safe_and_observer_global() {
        let observer = runtime(1, 55, 9);
        let forged = runtime(1, 56, 9);
        let mut model = ObserverTimelineReference::new(
            observer,
            3,
            ObserverNetworkProfile::CleanLan,
            1,
            1_000,
            u16::MAX - 1,
        );
        model.observe_emitted(u16::MAX, 994);
        let first = receipt(observer, u16::MAX, 1, u16::MAX, false);
        let decision = model
            .verify_receipt(observer, 1_000, first, true)
            .expect("exact current receipt");
        assert_eq!(decision.render_tick, 993);
        assert_eq!(decision.delay_ticks, 7);
        assert_eq!(
            model.verify_receipt(observer, 1_000, first, true),
            Ok(decision),
            "duplicate input reproduces the exact frozen decision"
        );
        let altered = CombatViewReceiptV1 {
            latest_game_applied_snapshot_id: 1,
            ..first
        };
        assert_eq!(
            model.verify_receipt(observer, 1_000, altered, true),
            Err(TimelineRejection::DuplicateMismatch)
        );
        assert_eq!(
            model.verify_receipt(
                forged,
                1_001,
                receipt(observer, 1, 1, u16::MAX, false),
                true
            ),
            Err(TimelineRejection::WrongRuntime)
        );
        model.observe_emitted(1, 995);
        assert!(
            model
                .verify_receipt(observer, 1_002, receipt(observer, 1, 1, 1, false), true)
                .is_ok(),
            "input and snapshot ids advance through wrap"
        );
        assert_eq!(
            model.verify_client_selected_cursor(1_002, 100),
            Err(TimelineRejection::ClientSelectedCursor),
            "v1 has no client-selected target-specific rewind"
        );
    }

    #[test]
    fn holds_and_withheld_watermarks_fail_closed_for_weapons_without_freezing_time() {
        let observer = runtime(1, 55, 9);
        let mut model = ObserverTimelineReference::new(
            observer,
            3,
            ObserverNetworkProfile::SupportedMildWan,
            1,
            1_000,
            10,
        );
        model.observe_emitted(40, 990);
        assert_eq!(
            model.verify_receipt(observer, 1_000, receipt(observer, 10, 1, 40, true), true),
            Err(TimelineRejection::HeldWeaponEdge)
        );
        assert!(
            model
                .verify_receipt(observer, 1_000, receipt(observer, 10, 1, 40, true), false)
                .is_ok(),
            "ordinary movement may use the bounded hold grace"
        );
        assert_eq!(
            model.verify_receipt(observer, 1_040, receipt(observer, 11, 1, 40, true), false),
            Err(TimelineRejection::HoldExpired)
        );
        assert_eq!(
            model.verify_receipt(observer, 1_041, receipt(observer, 11, 1, 40, false), true),
            Err(TimelineRejection::ApplicationTooFarBehind)
        );
    }

    #[test]
    fn ordinary_frame_time_and_hitch_recovery_have_separate_allowances() {
        for frame_ticks in [1_u32, 2, 3, 5, 10, 12] {
            for initial_backlog in [0_u32, 35] {
                let mut cursor = PresentationCursorReference::default();
                cursor.advance(1_000, 0.0);
                if initial_backlog != 0 {
                    // A real suspension is still bounded to five ticks.
                    assert_eq!(cursor.advance(1_035, 0.350), 1_005);
                }
                let mut target = 1_000 + initial_backlog;
                for _ in 0..200 {
                    target += frame_ticks;
                    let drawn = cursor.advance(target, frame_ticks as f32 / 100.0);
                    assert!(tick_diff(target, drawn) >= 0);
                    if initial_backlog == 0 {
                        assert_eq!(drawn, target, "steady cadence must not create backlog");
                    }
                }
                assert!(
                    cursor.weapon_ready(target),
                    "regular cadence must retire prior backlog"
                );
                assert!(cursor.weapon_ready_for_input(target + 3));
                assert!(!cursor.weapon_ready_for_input(target + 4));
            }
        }
    }

    #[test]
    fn scheduler_stalls_recover_on_the_server_target_without_granting_weapon_authority() {
        let mut cursor = PresentationCursorReference::default();
        let scheduled = |server_tick: u32| server_tick.wrapping_sub(7);

        assert_eq!(cursor.advance(scheduled(200), 1.0 / 60.0), 193);
        assert!(cursor.weapon_ready(scheduled(200)));
        assert!(cursor.weapon_ready_for_input(scheduled(203)));
        assert!(!cursor.weapon_ready_for_input(scheduled(204)));

        // A 350 ms browser pause advances the server schedule by 35 ticks,
        // but one resumed frame may consume only the frozen 25 ms recovery
        // budget: five policy ticks at 200 Hz.
        let after_hitch = cursor.advance(scheduled(235), 0.350);
        assert_eq!(after_hitch, 198);
        assert!(!cursor.weapon_ready(scheduled(235)));

        let mut server_tick = 235_u32;
        let mut held_frames = 1;
        for _ in 0..40 {
            server_tick = server_tick.wrapping_add(2);
            let target = scheduled(server_tick);
            let prior = cursor.drawn_tick.expect("initialized cursor");
            let drawn = cursor.advance(target, 1.0 / 60.0);
            let step = tick_diff(drawn, prior);
            assert!((0..=4).contains(&step), "bounded recovery step: {step}");
            assert!(
                tick_diff(target, drawn) >= 0,
                "cursor never leads server target"
            );
            if cursor.weapon_ready(target) {
                break;
            }
            held_frames += 1;
        }
        assert!(held_frames > 1, "the hitch creates an observable hold");
        assert!(cursor.weapon_ready(scheduled(server_tick)));
        assert!(cursor.weapon_ready_for_input(scheduled(server_tick.wrapping_add(3))));
        assert!(!cursor.weapon_ready_for_input(scheduled(server_tick.wrapping_add(4))));

        // The same independent rule remains serial-safe at server tick wrap.
        let mut wrapped = PresentationCursorReference::default();
        assert_eq!(wrapped.advance(u32::MAX - 1, 0.0), u32::MAX - 1);
        assert_eq!(wrapped.advance(1, 1.0 / 60.0), 1);
        assert!(wrapped.weapon_ready(1));
    }

    #[test]
    fn lane_permutations_preserve_one_generation_and_one_combat_decision() {
        #[derive(Clone, Copy)]
        enum Event {
            CodecAck,
            Application,
            Input,
        }
        let observer = runtime(1, 55, 9);
        for order in permutations([Event::CodecAck, Event::Application, Event::Input]) {
            let mut receipts = ReceiptReference::default();
            receipts.emit(
                7,
                EmittedReceiptScope {
                    runtime: observer,
                    emitted_tick: 994,
                    contains_source: Some(runtime(768, u64::MAX - 255, 1)),
                },
            );
            let mut timeline = ObserverTimelineReference::new(
                observer,
                3,
                ObserverNetworkProfile::CleanLan,
                1,
                1_000,
                20,
            );
            timeline.observe_emitted(7, 994);
            let mut decision = None;
            for event in order {
                match event {
                    Event::CodecAck => {
                        assert_eq!(
                            receipts.observe_server_wire_fact(
                                observer,
                                7,
                                ClientReceiptFact::CodecAcked,
                            ),
                            ReceiptDisposition::Applied
                        );
                    }
                    Event::Application => {
                        assert_eq!(
                            receipts.observe_server_wire_fact(
                                observer,
                                7,
                                ClientReceiptFact::GameApplied,
                            ),
                            ReceiptDisposition::Applied
                        );
                    }
                    Event::Input => {
                        decision = Some(
                            timeline
                                .verify_receipt(
                                    observer,
                                    1_000,
                                    receipt(observer, 20, 1, 7, false),
                                    true,
                                )
                                .expect("receipt itself carries the applied watermark"),
                        );
                    }
                }
                assert_eq!(timeline.current_generation(), 1);
                assert!(
                    (OBSERVER_DELAY_MIN_TICKS..=OBSERVER_DELAY_MAX_TICKS)
                        .contains(&timeline.delay_at(1_000))
                );
            }
            assert_eq!(decision.expect("input event").render_tick, 993);
            let evidence = receipts.evidence[&7];
            assert!(evidence.codec_acked && evidence.game_applied);
            assert!(!evidence.presentable && !evidence.drawn);
        }
    }

    #[test]
    fn geometry_reference_matches_shared_reference_game_edges_and_falloff_examples() {
        for weapon_x in [-0.9, 0.0, 3.9, 4.0, 17.9, 18.0, 18.9, 19.0] {
            for ship_x in [0.0, 14.0, 18.0, 19.0, 20.0] {
                assert_eq!(
                    reference_direct_weapon_contact(weapon_x, 0.0, ship_x, 0.0, 14.0),
                    crate::continuum_weapon::continuum_weapon_contact_overlaps_ship(
                        weapon_x, 0.0, ship_x, 0.0, 14.0,
                    )
                );
            }
        }
        for level in 1..=4 {
            let radius = reference_proximity_radius(3.0, level);
            assert_eq!(
                radius,
                crate::continuum_weapon::continuum_proximity_bomb_trigger_radius(3.0, level)
            );
            for offset in [radius - 0.1, radius, radius + 0.1] {
                assert_eq!(
                    reference_proximity_contact(0.0, 0.0, offset, 0.0, radius, 14.0),
                    crate::continuum_weapon::continuum_proximity_sensor_overlaps_ship(
                        0.0, 0.0, offset, 0.0, radius, 14.0,
                    )
                );
            }
        }
        assert_eq!(reference_radial_damage(0.0, 160.0, 750.0), 750);
        assert_eq!(reference_radial_damage(38.0, 160.0, 750.0), 571);
        assert_eq!(reference_radial_damage(130.2, 160.0, 750.0), 139);
        assert_eq!(reference_radial_damage(160.0, 160.0, 750.0), 0);
        assert_eq!(reference_radial_damage(161.0, 160.0, 750.0), 0);

        for family in [
            ReferenceWeaponFamily::Bullet,
            ReferenceWeaponFamily::BouncingBullet,
            ReferenceWeaponFamily::Bomb,
            ReferenceWeaponFamily::ProximityBomb,
            ReferenceWeaponFamily::Mine,
            ReferenceWeaponFamily::Shrapnel,
            ReferenceWeaponFamily::Thor,
            ReferenceWeaponFamily::Burst,
            ReferenceWeaponFamily::RepelMutated,
        ] {
            assert!(
                reference_direct_weapon_contact(18.0, 0.0, 0.0, 0.0, 14.0),
                "{family:?} upper contact edge remains inclusive"
            );
            assert!(
                !reference_direct_weapon_contact(-18.0, 0.0, 0.0, 0.0, 14.0),
                "{family:?} lower contact edge remains exclusive"
            );
        }
    }

    #[test]
    fn collision_before_harmful_open_freezes_one_global_fifo_transaction() {
        let source = segment(768, u64::MAX - 255, 1, 3, 0);
        let victim = segment(1, 55, 9, 3, 0);
        let bystander = segment(2, 56, 4, 3, 0);
        let first = FrozenCollisionWitness {
            projectile_id: 10,
            collision_tick: 100,
            source_segment: source,
            victim_segment: victim,
            anchor_x_q8: 40 * 256,
            anchor_y_q8: 50 * 256,
            damage: 571,
            harmful_open_tick: 120,
        };
        let second = FrozenCollisionWitness {
            projectile_id: 11,
            collision_tick: 101,
            victim_segment: bystander,
            harmful_open_tick: 105,
            anchor_x_q8: 60 * 256,
            damage: 139,
            ..first
        };
        let mut collisions = CollisionReference::default();
        assert!(collisions.freeze(first));
        assert!(collisions.freeze(second));
        assert!(
            !collisions.freeze(second),
            "projectile identity is idempotent"
        );
        assert_eq!(
            collisions.advance_head(110, source, victim).unwrap().state,
            FrozenCollisionState::Pending,
            "later-ready bystander cannot overtake the global head"
        );
        let committed = collisions.advance_head(120, source, victim).unwrap();
        assert_eq!(committed.state, FrozenCollisionState::Committed);
        assert_eq!(
            committed.witness, first,
            "motion cannot rewrite the witness"
        );
        assert_eq!(collisions.pop_terminal(), Some(committed));
        let committed_bystander = collisions.advance_head(120, source, bystander).unwrap();
        assert_eq!(committed_bystander.state, FrozenCollisionState::Committed);
        assert_eq!(committed_bystander.witness.victim_segment, bystander);
    }

    #[test]
    fn stale_life_cancels_frozen_collision_without_rebinding_numeric_id() {
        let source = segment(768, u64::MAX - 255, 1, 3, 0);
        let victim = segment(1, 55, 9, 3, 0);
        let successor = segment(1, 55, 10, 3, 0);
        let witness = FrozenCollisionWitness {
            projectile_id: 10,
            collision_tick: 100,
            source_segment: source,
            victim_segment: victim,
            anchor_x_q8: 40 * 256,
            anchor_y_q8: 50 * 256,
            damage: 571,
            harmful_open_tick: 120,
        };
        let mut collisions = CollisionReference::default();
        collisions.freeze(witness);
        let cancelled = collisions.advance_head(120, source, successor).unwrap();
        assert_eq!(cancelled.state, FrozenCollisionState::CancelledStaleLife);
        assert_eq!(cancelled.witness.victim_segment, victim);
    }

    #[test]
    fn abuse_matrix_rejects_forged_scope_generation_future_and_stalled_weapon_edges() {
        let observer = runtime(1, 55, 9);
        let other_session = runtime(1, 56, 9);
        let other_life = runtime(1, 55, 10);
        let mut model = ObserverTimelineReference::new(
            observer,
            3,
            ObserverNetworkProfile::SupportedSlowWan,
            1,
            1_000,
            10,
        );
        model.observe_emitted(50, 985);
        assert_eq!(
            model.verify_receipt(
                other_session,
                1_000,
                receipt(observer, 10, 1, 50, false),
                true,
            ),
            Err(TimelineRejection::WrongRuntime)
        );
        assert_eq!(
            model.verify_receipt(observer, 1_000, receipt(other_life, 10, 1, 50, false), true,),
            Err(TimelineRejection::WrongLife)
        );
        assert_eq!(
            model.verify_receipt(observer, 1_000, receipt(observer, 10, 999, 50, false), true,),
            Err(TimelineRejection::UnknownGeneration)
        );
        assert_eq!(
            model.verify_receipt(observer, 1_000, receipt(observer, 10, 1, 51, false), true,),
            Err(TimelineRejection::UnknownAppliedSnapshot)
        );
        assert_eq!(
            model.verify_receipt(observer, 1_000, receipt(observer, 10, 1, 50, true), true,),
            Err(TimelineRejection::HeldWeaponEdge)
        );
    }

    #[test]
    fn seeded_profile_schedules_preserve_server_ownership_through_wrap_and_impairment() {
        let profiles = [
            ObserverNetworkProfile::CleanLan,
            ObserverNetworkProfile::SupportedMildWan,
            ObserverNetworkProfile::SupportedSlowWan,
            ObserverNetworkProfile::RecoveryStress,
            ObserverNetworkProfile::AutomaticWan,
        ];
        for (profile_index, profile) in profiles.into_iter().enumerate() {
            let envelope = profile.delay_envelope();
            for seed in 1_u64..=8 {
                let observer = runtime(1, 10_000 + seed, 9);
                let initial_generation = 100 + profile_index as u32 * 100 + seed as u32;
                let mut server_tick = if seed % 2 == 0 {
                    0x7fff_ffff - 80
                } else {
                    20_000 + seed as u32
                };
                let mut input_sequence = if seed % 3 == 0 {
                    u16::MAX - 40
                } else {
                    500 + seed as u16
                };
                let mut snapshot_id = if seed % 5 == 0 { u16::MAX - 20 } else { 100 };
                let mut rng = DeterministicRng(seed ^ ((profile_index as u64 + 1) << 48));
                let mut timeline = ObserverTimelineReference::new(
                    observer,
                    70 + seed,
                    profile,
                    initial_generation,
                    server_tick,
                    input_sequence,
                );
                let first_snapshot = next_nonzero_snapshot_id(&mut snapshot_id);
                timeline.observe_emitted(first_snapshot, server_tick.wrapping_sub(1));
                let mut latest_snapshot = first_snapshot;
                let mut ticks_since_emission = 0_u8;
                let mut last_generation_tick = server_tick;

                let mut discontinuity = 1_u32;
                let mut current_segment = segment(1, 10_000 + seed, 9, 70 + seed, discontinuity);
                let first_segment = current_segment;
                let mut motion = MotionHistoryReference::new();
                assert_eq!(
                    motion.append(motion_sample(current_segment, server_tick, 0, 0)),
                    HistoryAppendDisposition::Applied
                );

                for step in 1_u32..=360 {
                    server_tick = (server_tick + 1) & 0x7fff_ffff;
                    input_sequence = input_sequence.wrapping_add(1);

                    // Seeded omission is bounded to four ticks, while snapshot and
                    // input ids deliberately cross their serial-number wraps.
                    ticks_since_emission = ticks_since_emission.saturating_add(1);
                    if rng.next_u32() % 4 != 0 || ticks_since_emission >= 4 {
                        latest_snapshot = next_nonzero_snapshot_id(&mut snapshot_id);
                        timeline.observe_emitted(latest_snapshot, server_tick);
                        ticks_since_emission = 0;
                    }

                    if step % 41 == 0 {
                        let retired = current_segment;
                        discontinuity = discontinuity.wrapping_add(1).max(1);
                        current_segment = segment(1, 10_000 + seed, 9, 70 + seed, discontinuity);
                        assert_eq!(
                            motion.append(motion_sample(
                                current_segment,
                                server_tick,
                                step as i32,
                                profile_index as i32,
                            )),
                            HistoryAppendDisposition::Applied
                        );
                        assert_eq!(
                            motion.append(motion_sample(
                                retired,
                                server_tick,
                                step as i32,
                                profile_index as i32,
                            )),
                            HistoryAppendDisposition::RejectedRetiredSegment
                        );
                    } else {
                        assert_eq!(
                            motion.append(motion_sample(
                                current_segment,
                                server_tick,
                                step as i32,
                                profile_index as i32,
                            )),
                            HistoryAppendDisposition::Applied
                        );
                    }

                    if tick_diff(server_tick, last_generation_tick)
                        >= OBSERVER_TIMELINE_MIN_GENERATION_TICKS as i32
                    {
                        let requested = (rng.next_u32() & 0xff) as u8;
                        let directive = timeline
                            .issue_delay_target(server_tick, input_sequence, requested)
                            .expect("one server-owned generation per bounded interval");
                        assert_eq!(directive.effective_from_input_sequence, input_sequence);
                        assert_eq!(
                            directive.target_delay_ticks,
                            requested.clamp(OBSERVER_DELAY_MIN_TICKS, envelope.ceiling_ticks)
                        );
                        last_generation_tick = server_tick;
                    }

                    let directive = timeline
                        .directive_for_sequence(input_sequence)
                        .expect("server schedule covers every current input");
                    let prior_sequence = timeline.last_input_sequence;
                    if step % 29 == 0 {
                        let forged_generation = directive.generation.wrapping_add(1_000_000).max(1);
                        assert_eq!(
                            timeline.verify_receipt(
                                observer,
                                server_tick,
                                receipt(
                                    observer,
                                    input_sequence,
                                    forged_generation,
                                    latest_snapshot,
                                    false,
                                ),
                                true,
                            ),
                            Err(TimelineRejection::UnknownGeneration)
                        );
                        assert_eq!(timeline.last_input_sequence, prior_sequence);
                        assert_eq!(
                            timeline.verify_receipt(
                                observer,
                                server_tick,
                                receipt(observer, input_sequence, directive.generation, 0, false,),
                                true,
                            ),
                            Err(TimelineRejection::UnknownAppliedSnapshot)
                        );
                        assert_eq!(timeline.last_input_sequence, prior_sequence);
                    }
                    if step % 23 == 0 {
                        assert_eq!(
                            timeline.verify_receipt(
                                observer,
                                server_tick,
                                receipt(
                                    observer,
                                    input_sequence,
                                    directive.generation,
                                    latest_snapshot,
                                    true,
                                ),
                                true,
                            ),
                            Err(TimelineRejection::HeldWeaponEdge)
                        );
                        assert_eq!(timeline.last_input_sequence, prior_sequence);
                    }

                    let accepted_receipt = receipt(
                        observer,
                        input_sequence,
                        directive.generation,
                        latest_snapshot,
                        false,
                    );
                    let weapon_edge = rng.next_u32() % 5 == 0;
                    let decision = timeline
                        .verify_receipt(observer, server_tick, accepted_receipt, weapon_edge)
                        .expect("bounded applied watermark remains eligible");
                    assert_eq!(decision.generation, directive.generation);
                    assert_eq!(decision.input_sequence, input_sequence);
                    assert_eq!(decision.applied_snapshot_id, latest_snapshot);
                    assert_eq!(decision.weapon_allowed, weapon_edge);
                    assert_eq!(
                        decision.render_tick,
                        server_tick.wrapping_sub(u32::from(decision.delay_ticks))
                    );
                    assert!(
                        (OBSERVER_DELAY_MIN_TICKS..=envelope.ceiling_ticks)
                            .contains(&decision.delay_ticks)
                    );
                    assert_eq!(timeline.last_input_sequence, Some(input_sequence));
                    assert!(timeline.directives.len() <= OBSERVER_SCHEDULE_HISTORY_CAPACITY);
                    assert!(timeline.emitted.len() <= 64);
                    assert!(timeline.accepted.len() <= MAX_ACCEPTED_INPUT_DECISIONS);
                    assert_eq!(
                        motion
                            .at_or_before(current_segment, server_tick)
                            .expect("current exact segment remains queryable")
                            .server_tick,
                        server_tick
                    );

                    if step % 17 == 0 {
                        assert_eq!(
                            timeline.verify_receipt(
                                observer,
                                server_tick,
                                accepted_receipt,
                                weapon_edge,
                            ),
                            Ok(decision),
                            "duplicate input is one idempotent frozen decision"
                        );
                    }
                }

                assert_eq!(
                    motion.append(motion_sample(
                        first_segment,
                        server_tick.wrapping_add(1),
                        999,
                        999,
                    )),
                    HistoryAppendDisposition::RejectedRetiredSegment,
                    "an evicted pre-wrap segment cannot resume"
                );
            }
        }
    }

    #[test]
    fn reconnect_id_reuse_and_arena_reset_cannot_rebind_prior_generation() {
        let old_runtime = runtime(1, 55, 9);
        let new_runtime = runtime(1, 56, 9);
        let mut old = ObserverTimelineReference::new(
            old_runtime,
            3,
            ObserverNetworkProfile::CleanLan,
            41,
            1_000,
            10,
        );
        old.observe_emitted(7, 994);
        let old_receipt = receipt(old_runtime, 10, 41, 7, false);
        assert!(
            old.verify_receipt(old_runtime, 1_000, old_receipt, true)
                .is_ok()
        );

        let mut reconnect = ObserverTimelineReference::new(
            new_runtime,
            4,
            ObserverNetworkProfile::CleanLan,
            42,
            1_000,
            10,
        );
        reconnect.observe_emitted(7, 994);
        assert_eq!(
            reconnect.verify_receipt(new_runtime, 1_000, old_receipt, true),
            Err(TimelineRejection::UnknownGeneration),
            "same numeric player/life/snapshot ids do not authenticate an old session receipt"
        );
        let reconnect_receipt = receipt(new_runtime, 10, 42, 7, false);
        assert!(
            reconnect
                .verify_receipt(new_runtime, 1_000, reconnect_receipt, true)
                .is_ok()
        );

        let mut new_arena = ObserverTimelineReference::new(
            new_runtime,
            5,
            ObserverNetworkProfile::CleanLan,
            43,
            1_000,
            10,
        );
        new_arena.observe_emitted(7, 994);
        assert_eq!(
            new_arena.verify_receipt(new_runtime, 1_000, reconnect_receipt, true),
            Err(TimelineRejection::UnknownGeneration),
            "arena replacement gets a fresh server generation even on one transport"
        );
        assert_ne!(old.arena_stream_id, reconnect.arena_stream_id);
        assert_ne!(reconnect.arena_stream_id, new_arena.arena_stream_id);

        let mut generation_wrap = ObserverTimelineReference::new(
            new_runtime,
            6,
            ObserverNetworkProfile::CleanLan,
            u32::MAX,
            u32::MAX - 20,
            u16::MAX - 10,
        );
        let wrapped = generation_wrap
            .issue_delay_target(79, 100, OBSERVER_DELAY_MIN_TICKS)
            .expect("tick and input sequence wrap remain newer");
        assert_eq!(wrapped.generation, 1, "generation zero is never issued");
    }

    #[test]
    fn wire_policy_rejects_arbitrary_delay_even_when_checksum_shape_is_valid() {
        let observer = runtime(1, 55, 9);
        let model = ObserverTimelineReference::new(
            observer,
            3,
            ObserverNetworkProfile::CleanLan,
            1,
            1_000,
            10,
        );
        let mut directive = model.directives[0];
        directive.generation = 0;
        assert_eq!(
            directive.validate(),
            Err(ObserverTimelineWireError::Generation)
        );
        assert_eq!(
            ObserverTimelineDirectiveV1::decode(&directive.encode()),
            Err(ObserverTimelineWireError::Generation)
        );
        directive = model.directives[0];
        directive.target_delay_ticks = OBSERVER_DELAY_MAX_TICKS;
        assert_eq!(directive.validate(), Err(ObserverTimelineWireError::Delay));
        let bytes = directive.encode();
        assert_eq!(
            ObserverTimelineDirectiveV1::decode(&bytes),
            Err(ObserverTimelineWireError::Delay)
        );
    }

    #[test]
    fn capacity_budget_is_bounded_for_two_hundred_actors_and_observers() {
        let motion_bytes =
            200 * ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY * std::mem::size_of::<ActorMotionSampleV1>();
        // Count the actual retained representation and frame/container headers,
        // not an assumed sample size or purported reuse of emitted entity maps.
        let presentation_bytes =
            crate::combat_timeline::ObserverPresentationHistory::dense_retention_bytes(200, 200);
        let schedule_bytes = 200
            * OBSERVER_SCHEDULE_HISTORY_CAPACITY
            * std::mem::size_of::<ObserverTimelineDirectiveV1>();
        assert!(motion_bytes < 2 * 1024 * 1024);
        assert!(presentation_bytes <= 64 * 1024 * 1024);
        assert!(schedule_bytes < 2 * 1024 * 1024);
        assert!(motion_bytes + presentation_bytes + schedule_bytes < 68 * 1024 * 1024);
    }

    #[test]
    fn sample_clock_candidate_requires_exact_steps_and_explicit_stall_boundary() {
        let life = segment(7, 9, 3, 11, 1);
        let mut clock = AcceptedSampleClockReference::at_authorized_boundary(life, u16::MAX, 300);
        let initial = clock;
        assert_eq!(
            clock.append_proven_step(life, 1),
            SampleClockDisposition::MissingOrStaleStep
        );
        assert_eq!(
            clock, initial,
            "a two-step packet tail is not proof of its omitted input"
        );
        assert_eq!(
            clock.append_proven_step(life, 0),
            SampleClockDisposition::Recorded(305)
        );
        assert_eq!(
            clock.eligibility(303, 30),
            SampleClockDisposition::NotYetDue(305)
        );
        assert_eq!(
            clock.eligibility(306, 30),
            SampleClockDisposition::Eligible(305)
        );
        assert_eq!(
            clock.eligibility(274, 30),
            SampleClockDisposition::NeedsExplicitBoundary,
            "even recorded steps cannot create an unbounded future schedule",
        );
        let before_reorder = clock;
        assert_eq!(
            clock.append_proven_step(life, u16::MAX),
            SampleClockDisposition::MissingOrStaleStep
        );
        assert_eq!(clock, before_reorder);
        // Sixty exact steps advance one second regardless of packet grouping.
        for sequence in 1..60 {
            clock.append_proven_step(life, sequence);
        }
        assert_eq!(clock.source_quanta, 600);
        let before_pause = clock;
        assert_eq!(
            clock.eligibility(900, 30),
            SampleClockDisposition::NeedsExplicitBoundary
        );
        assert_eq!(
            clock, before_pause,
            "checking a pause cannot retime emitted history"
        );
        // A server-authorized boundary anchors time but consumes no movement.
        let resumed = segment(7, 9, 3, 11, 2);
        clock = AcceptedSampleClockReference::at_authorized_boundary(resumed, 60, 900);
        let boundary = clock;
        assert_eq!(
            clock.append_proven_step(life, 61),
            SampleClockDisposition::WrongSegment
        );
        assert_eq!(clock, boundary);
        assert_eq!(
            clock.append_proven_step(resumed, 61),
            SampleClockDisposition::Recorded(905)
        );
        assert_eq!(clock.source_quanta, 905);
    }

    #[test]
    fn sample_clock_distinguishes_delivery_gap_from_discarded_simulation_time() {
        let actor = segment(7, 9, 3, 11, 1);
        let initial = AcceptedSampleClockReference::at_authorized_boundary(actor, 100, 300);
        let mut delayed = initial;
        // 400 ms without a position report can still contain 24 exact replay
        // witnesses. Delivery loss/delay is not itself a clock discontinuity.
        for sequence in 101..=124 {
            assert!(matches!(
                delayed.append_proven_step(actor, sequence),
                SampleClockDisposition::Recorded(_)
            ));
        }
        assert_eq!(
            delayed.eligibility(420, 18),
            SampleClockDisposition::Eligible(420)
        );
        // A browser stall of the same duration can discard elapsed simulation
        // time: only its bounded eight catch-up steps actually occurred.
        // Both tails may be valid movement, but they cannot share a time anchor.
        let mut paused = initial;
        for sequence in 101..=108 {
            paused.append_proven_step(actor, sequence);
        }
        assert_eq!(paused.source_quanta, 340);
        assert_eq!(
            paused.eligibility(420, 18),
            SampleClockDisposition::NeedsExplicitBoundary
        );
        // Do not repair this by fabricating 16 missing steps or increasing
        // rewind: the accepted geometry and sequence remain exactly unchanged.
        let before = paused;
        assert_eq!(
            paused.append_proven_step(actor, 124),
            SampleClockDisposition::MissingOrStaleStep
        );
        assert_eq!(paused, before);
        // Timing-only recovery cannot retire the actor's physical motion
        // segment or life. The following actual input owns the next step.
        let resumed = actor;
        paused = AcceptedSampleClockReference::at_authorized_boundary(resumed, 108, 420);
        assert_eq!(
            paused.append_proven_step(resumed, 109),
            SampleClockDisposition::Recorded(425)
        );
    }

    #[test]
    fn sample_clock_recovery_uses_existing_presentation_window_after_complete_batch_only() {
        let actor = segment(7, 9, 3, 11, 1);
        let initial = AcceptedSampleClockReference::at_authorized_boundary(actor, 100, 300);
        for (steps, recovered) in [(24, false), (8, true)] {
            let mut clock = initial;
            for sequence in 101..=100 + steps {
                clock.append_proven_step(actor, sequence);
            }
            assert_eq!(clock.recover_after_accepted_batch(420), recovered);
            assert_eq!(clock.source_quanta, 420);
            assert_eq!(clock.segment, actor);
            assert_eq!(clock.sequence, 100 + steps);
            assert!(
                !clock.recover_after_accepted_batch(420),
                "idempotent recovery"
            );
            assert_eq!(
                clock.append_proven_step(actor, 101 + steps),
                SampleClockDisposition::Recorded(425)
            );
        }
        let window = u64::from(OBSERVER_DELAY_MIN_TICKS) * 3;
        for age in [0, window - 1, window, window + 1] {
            let mut clock = initial;
            assert_eq!(clock.recover_after_accepted_batch(300 + age), age > window);
        }
        let mut future = initial;
        assert!(!future.recover_after_accepted_batch(0));
        assert_eq!(
            future, initial,
            "future evidence cannot move backward to publish early"
        );

        let mut publication = SamplePublicationReference {
            clock: initial,
            emitted: VecDeque::new(),
            capacity: 32,
        };
        let emitted = publication.publish(1, 300, window, 123).unwrap();
        for sequence in 101..=108 {
            publication.clock.append_proven_step(actor, sequence);
        }
        assert!(publication.clock.recover_after_accepted_batch(420));
        assert_eq!(publication.emitted_sample(1), Some(emitted));
        assert_eq!(
            publication
                .publish(2, 420, window, 456)
                .unwrap()
                .source_quanta,
            420
        );
    }

    #[test]
    fn accepted_history_recovery_matches_independent_batch_schedule_through_wrap() {
        use crate::combat_timeline::ActorMotionHistory;
        for origin in [1_000_u32, 0x7fff_ffff - 50] {
            let mut history = ActorMotionHistory::new();
            let actor = segment(7, 9, 3, 11, 1);
            assert_eq!(
                history.begin_segment(actor.actor, actor.arena_stream_id),
                actor
            );
            let mut expected =
                AcceptedSampleClockReference::at_authorized_boundary(actor, u16::MAX - 100, 0);
            history
                .begin_accepted_samples(expected.sequence, motion_sample(actor, origin, 0, 0))
                .unwrap();
            let mut elapsed_ticks = 0_u64;
            let mut position = 0;
            let mut recovery_count = 0;
            for iteration in 0..1_000 {
                let (steps, ticks) =
                    [(2, 3), (2, 4), (1, 2), (8, 40), (24, 40), (2, 3)][iteration % 6];
                elapsed_ticks += ticks;
                let mut batch = Vec::new();
                for _ in 0..steps {
                    position += 2;
                    let sequence = expected.sequence.wrapping_add(1);
                    expected.append_proven_step(actor, sequence);
                    batch.push((
                        sequence,
                        motion_sample(
                            actor,
                            (u64::from(origin) + elapsed_ticks) as u32 & 0x7fff_ffff,
                            position,
                            0,
                        ),
                    ));
                }
                recovery_count +=
                    usize::from(expected.recover_after_accepted_batch(elapsed_ticks * 3));
                history.append_accepted_samples(&batch).unwrap();
                let actual = history.current_samples().next_back().unwrap();
                assert_eq!(
                    actual.sample_time.parts(),
                    (
                        (u64::from(origin) + expected.source_quanta / 3) as u32 & 0x7fff_ffff,
                        (expected.source_quanta % 3) as u8,
                    )
                );
                assert_eq!(actual.segment, actor);
                assert_eq!(actual.x_q8, position * 256);
                assert!(history.len() <= ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY);
            }
            assert!(
                recovery_count > 100,
                "schedule must actually exercise recovery"
            );
        }
    }

    #[test]
    fn implemented_sample_clock_matches_independent_hour_long_schedule() {
        use crate::combat_timeline::{
            AcceptedMotionSampleClock, PresentationSampleTime, SampleClockAdmission,
            SampleClockPublication,
        };
        let actor = segment(7, 9, 3, 11, 1);
        let origin = 0x7fff_ffff_u32 - 100;
        let mut reference =
            AcceptedSampleClockReference::at_authorized_boundary(actor, u16::MAX, 0);
        let mut actual = AcceptedMotionSampleClock::at_authorized_boundary(actor, u16::MAX, origin);
        let time = |quanta: u64| {
            PresentationSampleTime::from_parts(
                (u64::from(origin) + quanta / 3) as u32 & 0x7fff_ffff,
                (quanta % 3) as u8,
            )
            .unwrap()
        };
        for step in 1..=216_000_u64 {
            let sequence = (step - 1) as u16;
            let SampleClockDisposition::Recorded(quanta) =
                reference.append_proven_step(actor, sequence)
            else {
                panic!("reference rejected exact step");
            };
            assert_eq!(
                actual.append_proven_step(actor, sequence),
                SampleClockAdmission::Recorded(time(quanta))
            );
            if step % 4096 == 0 {
                for offset in [-12_i64, -1, 0, 1, 12] {
                    let now_ticks = (quanta.div_ceil(3) as i64 + offset) as u64;
                    let expected = match reference.eligibility(now_ticks * 3, 18) {
                        SampleClockDisposition::Eligible(t) => {
                            SampleClockPublication::Eligible(time(t))
                        }
                        SampleClockDisposition::NotYetDue(_) => SampleClockPublication::NotYetDue,
                        SampleClockDisposition::NeedsExplicitBoundary => {
                            SampleClockPublication::NeedsAuthorizedBoundary
                        }
                        other => panic!("unexpected model result {other:?}"),
                    };
                    assert_eq!(
                        actual.publication_at(
                            (u64::from(origin) + now_ticks) as u32 & 0x7fff_ffff,
                            6
                        ),
                        expected
                    );
                }
                let before = actual;
                assert_eq!(
                    actual.append_proven_step(actor, sequence),
                    SampleClockAdmission::MissingOrStaleStep
                );
                assert_eq!(
                    actual.append_proven_step(segment(7, 9, 4, 11, 1), sequence.wrapping_add(1)),
                    SampleClockAdmission::WrongSegment
                );
                assert_eq!(actual, before);
            }
        }
    }

    #[test]
    fn sample_clock_candidate_has_no_hour_long_rounding_drift_across_both_wraps() {
        let initial_tick = 0x7fff_ffff_u32 - 100;
        let actor = segment(7, 9, 3, 11, 1);
        let mut server = ServerTickUnwrapperReference {
            wire_tick: initial_tick,
            elapsed_ticks: 0,
        };
        let mut source = AcceptedSampleClockReference::at_authorized_boundary(actor, u16::MAX, 0);
        for step in 1..=216_000_u64 {
            let expected_quanta = step * 5;
            assert_eq!(
                source.append_proven_step(actor, (step - 1) as u16),
                SampleClockDisposition::Recorded(expected_quanta)
            );
            let elapsed_ticks = expected_quanta.div_ceil(3);
            let now = server
                .observe((u64::from(initial_tick) + elapsed_ticks) as u32 & 0x7fff_ffff)
                .unwrap();
            assert_eq!(
                source.eligibility(now, 2),
                SampleClockDisposition::Eligible(expected_quanta)
            );
            assert!(now - expected_quanta <= 2, "rounding must not accumulate");
            if step % 4096 == 0 {
                let before = server;
                assert_eq!(
                    server.observe(server.wire_tick.wrapping_sub(1) & 0x7fff_ffff),
                    None
                );
                assert_eq!(server, before);
                assert_eq!(
                    server.observe((server.wire_tick + (1 << 30)) & 0x7fff_ffff),
                    None
                );
                assert_eq!(server, before);
            }
        }
        assert_eq!(source.source_quanta, 3_600 * 300);
        assert_eq!(server.elapsed_ticks, 3_600 * 100);
    }

    #[test]
    fn sample_clock_recovery_does_not_retimestamp_retained_geometry() {
        let old = segment(7, 9, 3, 11, 1);
        let mut publication = SamplePublicationReference {
            clock: AcceptedSampleClockReference::at_authorized_boundary(old, 100, 300),
            emitted: VecDeque::new(),
            capacity: 32,
        };
        // Timestamped emitted state is a copied fact, never a reference to a
        // mutable clock origin. The local capacity is a fixture, not policy.
        for sequence in 101..=140 {
            let SampleClockDisposition::Recorded(time) =
                publication.clock.append_proven_step(old, sequence)
            else {
                panic!("exact sequence must record");
            };
            publication
                .publish(sequence, time, 30, i32::from(sequence) * 256)
                .unwrap();
        }
        let retained = publication.emitted.clone();
        let before = publication.clone();
        assert_eq!(
            publication.publish(141, 900, 30, 0),
            Err(SampleClockDisposition::NeedsExplicitBoundary)
        );
        assert_eq!(
            publication, before,
            "rejected emission cannot evict or rewrite history"
        );
        let new = segment(7, 9, 3, 11, 2);
        publication.apply_authorized_boundary(
            AcceptedSampleClockReference::at_authorized_boundary(new, 141, 900),
        );
        assert_eq!(
            publication.clock.append_proven_step(new, 142),
            SampleClockDisposition::Recorded(905)
        );
        assert_eq!(publication.emitted, retained);
        let before = publication.clone();
        assert_eq!(
            publication.publish(141, 903, 30, 0),
            Err(SampleClockDisposition::NotYetDue(905))
        );
        assert_eq!(publication, before);
        let current = publication.publish(141, 905, 30, 142 * 256).unwrap();
        assert_eq!(current.segment, new);
        assert_eq!(current.source_quanta, 905);
        assert_eq!(publication.emitted.len(), 32);
        assert_eq!(
            publication.emitted_sample(109),
            None,
            "exactly one oldest entry evicted"
        );
        for sample in retained.iter().skip(1) {
            assert_eq!(
                publication.emitted_sample(sample.snapshot_id),
                Some(*sample)
            );
        }
        let before = publication.clone();
        assert_eq!(
            publication.publish(140, 906, 30, 0),
            Err(SampleClockDisposition::MissingOrStaleStep)
        );
        assert_eq!(publication, before);
    }

    #[test]
    fn publication_clock_model_distinguishes_source_acceptance_and_emission_time() {
        // Independent constant-speed schedule, in microseconds: 60 Hz input,
        // 23 ms browser publication, and 60 Hz server snapshots. No production
        // replay, interpolation, or clock implementation is called here.
        // A timestamp-only move from emission to receipt is a tempting fix,
        // but cannot recover simulation duration from batched packet timing.
        const SPEED: f64 = 120.0;
        let source_position = |sequence: u64| sequence as f64 * SPEED / 60.0;
        let speed_between = |a: (u64, u64), b: (u64, u64)| {
            (source_position(b.1) - source_position(a.1)) * 1_000_000.0 / (b.0 - a.0) as f64
        };
        for phase_ms in 0..17 {
            for (jitter_us, omit_every) in [(0, 0), (8_000, 0), (32_000, 0), (32_000, 11)] {
                let mut deliveries = Vec::new();
                for packet in 1..40_u64 {
                    if omit_every != 0 && packet % omit_every == 0 {
                        continue;
                    }
                    let sent_us = packet * 23_000 + phase_ms * 1_000;
                    let input_sequence = sent_us * 60 / 1_000_000;
                    let received_us = sent_us + (packet % 2) * jitter_us;
                    deliveries.push((received_us, input_sequence));
                }
                deliveries.sort_unstable();
                let mut accepted = vec![(0_u64, 0_u64)];
                let mut overtaken = 0;
                for delivery in deliveries {
                    if delivery.1 <= accepted.last().unwrap().1 {
                        overtaken += 1;
                        continue;
                    }
                    accepted.push(delivery);
                }
                assert_eq!(overtaken > 0, jitter_us > 23_000);
                let emitted: Vec<_> = (1..60_u64)
                    .map(|frame| {
                        let tick_us = frame * 1_000_000 / 60;
                        let tail = accepted.iter().rev().find(|p| p.0 <= tick_us).unwrap();
                        (tick_us, tail.1)
                    })
                    .collect();
                let peak_emitted = emitted
                    .windows(2)
                    .map(|p| speed_between(p[0], p[1]))
                    .fold(0.0_f64, f64::max);
                let peak_accepted = accepted
                    .windows(2)
                    .map(|p| speed_between(p[0], p[1]))
                    .fold(0.0_f64, f64::max);
                assert!(
                    peak_emitted > SPEED * 1.5,
                    "phase={phase_ms} jitter={jitter_us}"
                );
                // Two steps in one 23 ms report; with alternating 8 ms
                // delay, that same interval contracts to 15 ms. Overtaking
                // discards odd reports instead, leaving three steps per
                // 46 ms. Derive the peaks rather than choosing a tolerance
                // that incorrectly assumes reordering always worsens them.
                if jitter_us < 23_000 {
                    let expected = 4_000_000.0 / (23_000 - jitter_us) as f64;
                    assert!((peak_accepted - expected).abs() < 1e-9);
                } else if omit_every == 0 {
                    assert!((peak_accepted - 6_000_000.0 / 46_000.0).abs() < 1e-9);
                }
                assert!(peak_accepted > SPEED, "receipt time is not source time");
                assert!(
                    emitted.windows(2).any(|p| p[0].1 == p[1].1),
                    "an emission can restamp an unchanged tail"
                );
                // Sequence duration explains the same accepted geometry without
                // inventing intermediate positions. This is NOT authorization
                // for trusting client time: anchoring, pause/recovery, life,
                // reordering, and bounded future eligibility remain necessary
                // before any production sample-clock policy can be selected.
                for pair in accepted.windows(2) {
                    let steps = pair[1].1 - pair[0].1;
                    let distance = source_position(pair[1].1) - source_position(pair[0].1);
                    assert!((distance / (steps as f64 / 60.0) - SPEED).abs() < 1e-9);
                }
            }
        }
    }
}
