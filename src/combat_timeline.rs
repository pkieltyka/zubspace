//! Frozen v1 policy and wire-shaped values for the server-owned combat view.
//!
//! These types bind accepted human weapons to a verified observer-wide render
//! schedule. They do not author outcomes themselves; projectile collision
//! consumes their server-validated projection as its sole human-view geometry.

#![allow(dead_code)]

use crate::authority_identity::PlayerRuntimeIdentity;
use std::collections::VecDeque;

#[path = "combat_presentation_storage.rs"]
mod presentation_storage;

/// Continuum protocol/server ticks are 10 ms (100 Hz).
pub(crate) const COMBAT_TIMELINE_TICK_MILLIS: u32 = 10;
pub(crate) const COMBAT_VIEW_PROTOCOL_VERSION: u8 = 1;
/// Minimum server-owned first-emission lead before a remote projectile can
/// harm its recipient. A visual birth recovery must finish within this floor.
pub(crate) const PROJECTILE_PRESENTATION_MIN_LEAD_TICKS: u32 = 28;

/// The current clean Chrome/Firefox x Chaos/Extreme evidence settles at
/// 60-70 ms. Seven ticks is therefore the source-measured v1 LAN floor.
pub(crate) const OBSERVER_DELAY_MIN_TICKS: u8 = 7;
/// Matches the source-reviewed browser interpolation ceiling of 280 ms.
pub(crate) const OBSERVER_DELAY_MAX_TICKS: u8 = 28;
/// A delay target changes by at most 10 ms per 100 ms of server time.
pub(crate) const OBSERVER_DELAY_SLEW_INTERVAL_TICKS: u32 = 10;
/// At short frame intervals, normal 100 Hz progression plus bounded recovery
/// follows the server-owned target at up to 200 Hz. Longer regular frames keep
/// their normal elapsed-time allowance but cap additional recovery separately,
/// so recovery still makes progress while the server keeps advancing. The browser reports
/// a hold while behind, so a scheduler stall cannot pair a newly accepted
/// weapon with a privately lagged presentation cursor.
pub(crate) const OBSERVER_PRESENTATION_RECOVERY_TICKS_PER_SECOND: u64 = 200;
/// Maximum elapsed time credited to additional recovery in one frame. A
/// suspension beyond the gameplay catch-up budget also uses this interval for
/// normal progression, preserving the existing five-tick hitch step.
pub(crate) const OBSERVER_PRESENTATION_MAX_STEP_SECONDS: f32 = 1.0 / 40.0;

pub(crate) fn bounded_observer_frame_seconds(render_dt_seconds: f32) -> f32 {
    if !render_dt_seconds.is_finite() || render_dt_seconds < 0.0 {
        return 0.0;
    }
    let gameplay_budget = crate::gameplay_rules::GAMEPLAY_STEP_SECONDS
        * f32::from(crate::gameplay_rules::MAX_GAMEPLAY_CATCH_UP_STEPS);
    if render_dt_seconds <= gameplay_budget {
        render_dt_seconds
    } else {
        OBSERVER_PRESENTATION_MAX_STEP_SECONDS
    }
}
/// Input is sampled later in the same browser frame than the presentation
/// cursor. Preserve the frozen three-tick sampling-phase allowance independently
/// of ordinary render cadence and recovery accounting. This allowance names the
/// already-drawn frame; it does not advance the cursor or enlarge server
/// rewind/hold authority.
pub(crate) const OBSERVER_PRESENTATION_INPUT_PHASE_TICKS: u32 = 3;
/// Do not issue another generation while a prior schedule is younger than one
/// second. A generation may contain a deterministic multi-step slew.
pub(crate) const OBSERVER_TIMELINE_MIN_GENERATION_TICKS: u32 = 100;
/// A scheduler hold is evidence, not permission to freeze a target forever.
/// New weapon edges fail closed during the hold; ordinary projection may hold
/// its last proven upper sample for at most this server-owned grace.
pub(crate) const OBSERVER_PRESENTATION_HOLD_GRACE_TICKS: u8 = 10;
/// Motion history also retains a complete 64-input replay at 60 Hz. Inputs
/// are not 100 Hz server ticks: 64 steps span 106 2/3 ticks. The sample cap
/// stays unchanged, so this fixes time coverage without increasing allocation.
pub(crate) const ACTOR_MOTION_HISTORY_RETENTION_TICKS: u32 =
    ((ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY as u32 - 1) * 5).div_ceil(3);
pub(crate) const ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY: usize = 65;
/// Server-native bot histories are arena-owned and never cross a bot roster
/// rebuild. The actor runtime still distinguishes every bot life.
pub(crate) const SERVER_BOT_MOTION_ARENA_STREAM_ID: u64 = 1;
/// Reuse the bounded per-recipient emitted snapshot ring. At 60 Hz, 32 frames
/// cover roughly 530 ms, which exceeds delay + hold + bracketing requirements.
pub(crate) const OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS: usize = 32;
/// The shared wire identity range is 0x0000..0x03ff: up to 768 connected
/// players followed by 256 server-owned bots. Keep the shared client/server
/// history independent of the server-only admission module.
pub(crate) const OBSERVER_PRESENTATION_MAX_ACTORS: usize = 0x0400;
pub(crate) const OBSERVER_PRESENTATION_SNAP_DISTANCE_Q8: i32 = 512 * 256;
/// Sparse policy generations are retained for the longest checked-in Chaos
/// bomb lifetime (80 s) plus one second. At the one-generation-per-second
/// bound, 128 entries leave 47 entries of headroom.
pub(crate) const OBSERVER_SCHEDULE_HISTORY_CAPACITY: usize = 128;
pub(crate) const OBSERVER_SCHEDULE_RETENTION_TICKS: u32 = 8_100;
/// Matches the server's complete reconstructed input window. Every first-seen
/// receipt, including a rejected one, is frozen so an input cannot choose a
/// different combat view after later lane arrivals reveal an outcome.
pub(crate) const OBSERVER_INPUT_DECISION_HISTORY_CAPACITY: usize = 64;
/// Projection disagreement is a defect/correction, never hitbox expansion.
pub(crate) const PRESENTATION_AUTHORITY_MAX_ERROR_Q8: i32 = 12 * 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObserverNetworkProfile {
    CleanLan,
    SupportedMildWan,
    SupportedSlowWan,
    RecoveryStress,
    /// Server-measured per-connection targets share one immutable WAN ceiling.
    /// Cold presentation starts at the twenty-tick ceiling and settles from
    /// trusted path evidence. Existing flights keep their admitted bound.
    AutomaticWan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObserverDelayEnvelope {
    pub(crate) target_ticks: u8,
    pub(crate) ceiling_ticks: u8,
}

impl ObserverNetworkProfile {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CleanLan => "clean-lan",
            Self::SupportedMildWan => "supported-mild-wan",
            Self::SupportedSlowWan => "supported-slow-wan",
            Self::RecoveryStress => "recovery-stress",
            Self::AutomaticWan => "auto",
        }
    }

    /// These are policy bounds, not client claims. Measured delivery quality
    /// may select a value inside the envelope but cannot exceed its ceiling.
    pub(crate) const fn delay_envelope(self) -> ObserverDelayEnvelope {
        match self {
            Self::CleanLan => ObserverDelayEnvelope {
                target_ticks: 7,
                ceiling_ticks: 9,
            },
            Self::SupportedMildWan => ObserverDelayEnvelope {
                target_ticks: 10,
                ceiling_ticks: 14,
            },
            Self::SupportedSlowWan => ObserverDelayEnvelope {
                target_ticks: 15,
                ceiling_ticks: 20,
            },
            Self::RecoveryStress => ObserverDelayEnvelope {
                target_ticks: 20,
                ceiling_ticks: OBSERVER_DELAY_MAX_TICKS,
            },
            Self::AutomaticWan => ObserverDelayEnvelope {
                // Before authenticated path evidence exists, render within
                // the supported worst-case envelope. A 70 ms initial cursor
                // can outrun every available frame on an otherwise healthy
                // asymmetric WAN, making its first real weapon edge held.
                target_ticks: 20,
                ceiling_ticks: 20,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MotionSegmentIdentity {
    pub(crate) actor: PlayerRuntimeIdentity,
    pub(crate) arena_stream_id: u64,
    pub(crate) discontinuity_generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActorMotionSourceKind {
    AcceptedHumanWitness,
    ServerBotSimulation,
}

/// Fixed-point Q8 coordinates keep model/parity fixtures deterministic. The
/// production history may choose an equivalent compact representation, but it
/// must retain this identity and precision contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActorMotionSampleV1 {
    pub(crate) segment: MotionSegmentIdentity,
    /// Admission/simulation clock retained for authority and eviction.
    pub(crate) server_tick: u32,
    /// Immutable presentation time belonging to this exact state. During the
    /// paired migration integral publishers retain their original tick here.
    pub(crate) sample_time: PresentationSampleTime,
    pub(crate) x_q8: i32,
    pub(crate) y_q8: i32,
    pub(crate) velocity_x_q8: i32,
    pub(crate) velocity_y_q8: i32,
    pub(crate) orientation_u16: u16,
    /// Source wire heading, captured before fixed-point angle rounding.
    /// Re-quantizing orientation_u16 can cross a 40-bin boundary.
    pub(crate) protocol_direction: u8,
    pub(crate) hull_radius_q8: u16,
    pub(crate) safe_or_attached: u8,
    pub(crate) source_kind: ActorMotionSourceKind,
}

impl ActorMotionSampleV1 {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_state(
        segment: MotionSegmentIdentity,
        server_tick: u32,
        x: f32,
        y: f32,
        velocity_x: f32,
        velocity_y: f32,
        orientation: f32,
        hull_radius: f32,
        safe_or_attached: u8,
        source_kind: ActorMotionSourceKind,
    ) -> Self {
        Self {
            segment,
            server_tick,
            sample_time: PresentationSampleTime::from_tick(server_tick),
            x_q8: q8_i32(x),
            y_q8: q8_i32(y),
            velocity_x_q8: q8_i32(velocity_x),
            velocity_y_q8: q8_i32(velocity_y),
            orientation_u16: orientation_u16(orientation),
            protocol_direction: crate::continuum_position::protocol_direction(orientation),
            hull_radius_q8: q8_u16(hull_radius),
            safe_or_attached,
            source_kind,
        }
    }

    pub(crate) fn position(self) -> (f32, f32) {
        (self.x_q8 as f32 / 256.0, self.y_q8 as f32 / 256.0)
    }

    pub(crate) fn velocity(self) -> (f32, f32) {
        (
            self.velocity_x_q8 as f32 / 256.0,
            self.velocity_y_q8 as f32 / 256.0,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryAppendDisposition {
    Applied,
    Duplicate,
    RejectedStaleTick,
    RejectedStaleSampleTime,
    RejectedClockedAppendRequired,
    RejectedRetiredSegment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryUnavailable {
    Empty,
    WrongSegment,
    BeforeRetainedWindow,
    NoSampleAtOrBefore,
}

/// One bounded, exact-life motion history for every authoritative combat
/// actor. A segment is replaced explicitly at every discontinuity; callers
/// can therefore never interpolate or silently fall back across a Warp,
/// correction, roster change, death, respawn, or arena boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActorMotionHistory {
    samples: VecDeque<ActorMotionSampleV1>,
    current_segment: Option<MotionSegmentIdentity>,
    // Last position discontinuity, retained across velocity-only segments.
    // A lost relocation followed by Repel must still reveal the relocation.
    relocation_generation: u32,
    retired_segments: VecDeque<MotionSegmentIdentity>,
    evictions: u64,
    accepted_sample_clock: Option<AcceptedMotionSampleClock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum ClockedHistoryError {
    BatchLength,
    MissingClock,
    WrongSegment,
    WrongSourceKind,
    MissingOrStaleStep,
    HistoryRejected(HistoryAppendDisposition),
    Recovery(SampleClockReanchorError),
}

impl Default for ActorMotionHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ActorMotionHistory {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY),
            current_segment: None,
            relocation_generation: 0,
            retired_segments: VecDeque::with_capacity(ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY),
            evictions: 0,
            accepted_sample_clock: None,
        }
    }

    /// Explicit accepted zero-motion baseline; never infer one from a missing
    /// replay witness. Physical segment allocation remains `begin_segment`'s job.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn begin_accepted_samples(
        &mut self,
        sequence: u16,
        mut sample: ActorMotionSampleV1,
    ) -> Result<(), ClockedHistoryError> {
        if self.current_segment != Some(sample.segment) {
            return Err(ClockedHistoryError::WrongSegment);
        }
        if sample.source_kind != ActorMotionSourceKind::AcceptedHumanWitness {
            return Err(ClockedHistoryError::WrongSourceKind);
        }
        let clock = AcceptedMotionSampleClock::at_authorized_boundary(
            sample.segment,
            sequence,
            sample.server_tick,
        );
        sample.sample_time = clock.time;
        match self.append_inner(sample) {
            HistoryAppendDisposition::Applied | HistoryAppendDisposition::Duplicate => {
                self.accepted_sample_clock = Some(clock);
                Ok(())
            }
            rejected => Err(ClockedHistoryError::HistoryRejected(rejected)),
        }
    }

    /// Commit all exact replay steps or none. Both staging and retention are
    /// bounded; the caller supplies validated states, not an inferred tail gap.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn append_accepted_samples(
        &mut self,
        steps: &[(u16, ActorMotionSampleV1)],
    ) -> Result<(), ClockedHistoryError> {
        if steps.is_empty() || steps.len() > ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY {
            return Err(ClockedHistoryError::BatchLength);
        }
        let mut clock = self
            .accepted_sample_clock
            .ok_or(ClockedHistoryError::MissingClock)?;
        let mut staged = self.clone();
        for &(sequence, mut sample) in steps {
            if self.current_segment != Some(sample.segment) {
                return Err(ClockedHistoryError::WrongSegment);
            }
            if sample.source_kind != ActorMotionSourceKind::AcceptedHumanWitness {
                return Err(ClockedHistoryError::WrongSourceKind);
            }
            sample.sample_time = match clock.append_proven_step(sample.segment, sequence) {
                SampleClockAdmission::Recorded(time) => time,
                SampleClockAdmission::WrongSegment => {
                    return Err(ClockedHistoryError::WrongSegment);
                }
                SampleClockAdmission::MissingOrStaleStep => {
                    return Err(ClockedHistoryError::MissingOrStaleStep);
                }
            };
            match staged.append_inner(sample) {
                HistoryAppendDisposition::Applied | HistoryAppendDisposition::Duplicate => {}
                rejected => return Err(ClockedHistoryError::HistoryRejected(rejected)),
            }
        }
        let tail = *staged.samples.back().expect("nonempty accepted batch");
        if let Some(time) = clock
            .recover_after_accepted_batch(tail.server_tick)
            .map_err(ClockedHistoryError::Recovery)?
        {
            // Keep every original proven sample immutable. A timing recovery
            // adds the same accepted tail at the new anchor; it does not retire
            // its physical segment or fabricate another movement step.
            let recovered = ActorMotionSampleV1 {
                sample_time: time,
                ..tail
            };
            match staged.append_inner(recovered) {
                HistoryAppendDisposition::Applied | HistoryAppendDisposition::Duplicate => {}
                rejected => return Err(ClockedHistoryError::HistoryRejected(rejected)),
            }
        }
        staged.accepted_sample_clock = Some(clock);
        *self = staged;
        Ok(())
    }

    /// Return one immutable state/time pair that has both been admitted and
    /// become due. Repeated snapshot emission cannot advance its source time,
    /// and a future tail cannot be paired with an older sample's timestamp.
    pub(crate) fn publication_sample_at(
        &self,
        server_tick: u32,
    ) -> Result<ActorMotionSampleV1, HistoryUnavailable> {
        if self.samples.is_empty() {
            return Err(HistoryUnavailable::Empty);
        }
        let now = PresentationSampleTime::from_tick(server_tick);
        self.current_samples()
            .rev()
            .find(|sample| {
                crate::continuum::continuum_tick_reached(server_tick, sample.server_tick)
                    && now
                        .quanta_since(sample.sample_time)
                        .is_some_and(|age| age >= 0)
            })
            .ok_or(HistoryUnavailable::NoSampleAtOrBefore)
    }

    pub(crate) fn current_segment(&self) -> Option<MotionSegmentIdentity> {
        self.current_segment
    }

    pub(crate) fn relocation_generation(&self) -> u32 {
        self.relocation_generation
    }

    /// Repel changes velocity without relocating the hull. It still retires
    /// the exact collision/presentation segment, but does not manufacture a
    /// warp. Preserve the last relocation across any number of impulses.
    pub(crate) fn begin_impulse_segment(
        &mut self,
        actor: PlayerRuntimeIdentity,
        arena_stream_id: u64,
    ) -> MotionSegmentIdentity {
        let prior_relocation = self
            .current_segment
            .filter(|segment| segment.actor == actor && segment.arena_stream_id == arena_stream_id)
            .map(|_| self.relocation_generation);
        let next = self.begin_segment(actor, arena_stream_id);
        if let Some(generation) = prior_relocation {
            self.relocation_generation = generation;
        }
        next
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(crate) fn evictions(&self) -> u64 {
        self.evictions
    }

    /// End the current continuity interval and allocate the exact next
    /// segment. Generations are scoped by actor runtime + arena stream and do
    /// not inherit across a new life or arena.
    pub(crate) fn begin_segment(
        &mut self,
        actor: PlayerRuntimeIdentity,
        arena_stream_id: u64,
    ) -> MotionSegmentIdentity {
        let generation = self
            .current_segment
            .filter(|current| current.actor == actor && current.arena_stream_id == arena_stream_id)
            .map_or(1, |current| {
                current.discontinuity_generation.wrapping_add(1).max(1)
            });
        let next = MotionSegmentIdentity {
            actor,
            arena_stream_id,
            discontinuity_generation: generation,
        };
        self.replace_current_segment(next);
        next
    }

    /// Append an exact state to the active segment, establishing its first
    /// segment when this actor/life has no history yet.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_state(
        &mut self,
        actor: PlayerRuntimeIdentity,
        arena_stream_id: u64,
        server_tick: u32,
        x: f32,
        y: f32,
        velocity_x: f32,
        velocity_y: f32,
        orientation: f32,
        hull_radius: f32,
        safe_or_attached: u8,
        source_kind: ActorMotionSourceKind,
    ) -> HistoryAppendDisposition {
        let segment = match self.current_segment {
            Some(current)
                if current.actor == actor && current.arena_stream_id == arena_stream_id =>
            {
                current
            }
            _ => self.begin_segment(actor, arena_stream_id),
        };
        self.append(ActorMotionSampleV1::from_state(
            segment,
            server_tick,
            x,
            y,
            velocity_x,
            velocity_y,
            orientation,
            hull_radius,
            safe_or_attached,
            source_kind,
        ))
    }

    pub(crate) fn append(&mut self, sample: ActorMotionSampleV1) -> HistoryAppendDisposition {
        if self.accepted_sample_clock.is_some() && self.current_segment == Some(sample.segment) {
            return HistoryAppendDisposition::RejectedClockedAppendRequired;
        }
        self.append_inner(sample)
    }

    fn append_inner(&mut self, sample: ActorMotionSampleV1) -> HistoryAppendDisposition {
        if self.retired_segments.contains(&sample.segment) {
            return HistoryAppendDisposition::RejectedRetiredSegment;
        }
        if let Some(last) = self.samples.back().copied() {
            if sample == last {
                return HistoryAppendDisposition::Duplicate;
            }
            // Tick monotonicity is scoped to one continuity segment. A life,
            // arena, Warp, correction, or other explicit boundary may be
            // established from an independently wrapped/rebased clock and
            // must never compare against the retired segment's tail.
            if sample.segment == last.segment
                && !crate::continuum::continuum_tick_reached(sample.server_tick, last.server_tick)
            {
                return HistoryAppendDisposition::RejectedStaleTick;
            }
            if sample.segment == last.segment
                && !sample
                    .sample_time
                    .quanta_since(last.sample_time)
                    .is_some_and(|delta| delta >= 0)
            {
                return HistoryAppendDisposition::RejectedStaleSampleTime;
            }
        }
        if self.current_segment != Some(sample.segment) {
            self.replace_current_segment(sample.segment);
        }
        self.samples.push_back(sample);
        self.evict_for(sample.server_tick);
        HistoryAppendDisposition::Applied
    }

    pub(crate) fn at_or_before(
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
        let oldest = self
            .samples
            .iter()
            .find(|sample| sample.segment == segment)
            .expect("segment membership checked");
        if crate::continuum::continuum_tick_diff(target_tick, oldest.server_tick) < 0 {
            return Err(HistoryUnavailable::BeforeRetainedWindow);
        }
        self.samples
            .iter()
            .rev()
            .copied()
            .find(|sample| {
                sample.segment == segment
                    && crate::continuum::continuum_tick_diff(target_tick, sample.server_tick) >= 0
            })
            .ok_or(HistoryUnavailable::NoSampleAtOrBefore)
    }

    pub(crate) fn current_samples(
        &self,
    ) -> impl DoubleEndedIterator<Item = ActorMotionSampleV1> + '_ {
        let current = self.current_segment;
        self.samples
            .iter()
            .copied()
            .filter(move |sample| Some(sample.segment) == current)
    }

    fn replace_current_segment(&mut self, next: MotionSegmentIdentity) {
        if self.current_segment == Some(next) {
            return;
        }
        self.accepted_sample_clock = None;
        self.relocation_generation = next.discontinuity_generation;
        if let Some(retired) = self.current_segment.replace(next) {
            self.retired_segments.push_back(retired);
            while self.retired_segments.len() > ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY {
                self.retired_segments.pop_front();
            }
        }
    }

    fn evict_for(&mut self, newest_tick: u32) {
        while self.samples.len() > ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY
            || self.samples.front().is_some_and(|oldest| {
                crate::continuum::continuum_tick_diff(newest_tick, oldest.server_tick)
                    > ACTOR_MOTION_HISTORY_RETENTION_TICKS as i32
            })
        {
            self.samples.pop_front();
            self.evictions = self.evictions.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObserverPresentationSampleV1 {
    pub(crate) sample_time: PresentationSampleTime,
    pub(crate) actor: PlayerRuntimeIdentity,
    pub(crate) motion_segment: MotionSegmentIdentity,
    pub(crate) x_q8: i32,
    pub(crate) y_q8: i32,
    pub(crate) velocity_x_q8: i32,
    pub(crate) velocity_y_q8: i32,
    pub(crate) orientation_u16: u16,
    pub(crate) snap_discontinuity: bool,
}

impl ObserverPresentationSampleV1 {
    pub(crate) fn position(self) -> (f32, f32) {
        (self.x_q8 as f32 / 256.0, self.y_q8 as f32 / 256.0)
    }

    pub(crate) const fn kinematics(self) -> PresentationKinematicSampleV1 {
        PresentationKinematicSampleV1 {
            x_q8: self.x_q8,
            y_q8: self.y_q8,
            velocity_x_q8: self.velocity_x_q8,
            velocity_y_q8: self.velocity_y_q8,
        }
    }
}

/// Shared fixed-point interpolation input consumed by both the browser render
/// path and the server collision resolver. Identity and discontinuity remain
/// outside this geometry-only value so neither consumer can silently select a
/// different sampling policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PresentationKinematicSampleV1 {
    pub(crate) x_q8: i32,
    pub(crate) y_q8: i32,
    pub(crate) velocity_x_q8: i32,
    pub(crate) velocity_y_q8: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObserverPresentationFrameV1 {
    pub(crate) observer: PlayerRuntimeIdentity,
    pub(crate) arena_stream_id: u64,
    pub(crate) snapshot_id: u16,
    pub(crate) server_tick: u32,
    pub(crate) actors: Vec<ObserverPresentationSampleV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObserverPresentationAppendDisposition {
    Applied,
    Duplicate,
    RejectedScope,
    RejectedStaleSnapshot,
    RejectedMalformedFrame,
}

/// Exact render-grade frames successfully emitted to one authenticated
/// observer. Frames that lose pacing, simulated loss, or the outbound queue
/// are never admitted. The current observer runtime and arena stream are part
/// of the scope, so reconnect or arena reuse cannot rebind an old snapshot id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObserverPresentationHistory {
    observer: Option<PlayerRuntimeIdentity>,
    arena_stream_id: Option<u64>,
    frames: VecDeque<presentation_storage::StoredFrame>,
    evictions: u64,
}

impl Default for ObserverPresentationHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ObserverPresentationHistory {
    #[cfg(test)]
    pub(crate) fn dense_retention_bytes(actors: usize, observers: usize) -> usize {
        observers
            * (std::mem::size_of::<Self>()
                + OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS
                    * (std::mem::size_of::<presentation_storage::StoredFrame>()
                        + 2 * std::mem::size_of::<usize>() // Arc strong/weak counts
                        + actors * std::mem::size_of::<presentation_storage::StoredSample>()))
    }

    pub(crate) fn new() -> Self {
        Self {
            observer: None,
            arena_stream_id: None,
            frames: VecDeque::with_capacity(OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS),
            evictions: 0,
        }
    }

    pub(crate) fn begin_scope(&mut self, observer: PlayerRuntimeIdentity, arena_stream_id: u64) {
        if self.observer == Some(observer) && self.arena_stream_id == Some(arena_stream_id) {
            return;
        }
        self.observer = Some(observer);
        self.arena_stream_id = Some(arena_stream_id);
        self.frames.clear();
    }

    /// Keep the bounded emitted-frame ring for admitted old-life flight in
    /// the same session/arena. Each frame retains its actual observer life,
    /// so new-life input receipts still cannot claim an old-life frame.
    pub(crate) fn advance_life(&mut self, observer: PlayerRuntimeIdentity, arena_stream_id: u64) {
        if self
            .observer
            .is_some_and(|previous| previous.session == observer.session)
            && self.arena_stream_id == Some(arena_stream_id)
        {
            self.observer = Some(observer);
        } else {
            self.begin_scope(observer, arena_stream_id);
        }
    }

    pub(crate) fn observe_emitted(
        &mut self,
        mut frame: ObserverPresentationFrameV1,
    ) -> ObserverPresentationAppendDisposition {
        if self.observer != Some(frame.observer)
            || self.arena_stream_id != Some(frame.arena_stream_id)
        {
            return ObserverPresentationAppendDisposition::RejectedScope;
        }
        frame.actors.sort_unstable_by_key(|sample| sample.actor);
        let malformed = frame.snapshot_id == 0
            || frame.actors.len() > OBSERVER_PRESENTATION_MAX_ACTORS
            || frame
                .actors
                .windows(2)
                .any(|pair| pair[0].actor == pair[1].actor)
            || frame.actors.iter().any(|sample| {
                sample.motion_segment.actor != sample.actor
                    || sample.motion_segment.arena_stream_id == 0
            });
        if malformed {
            return ObserverPresentationAppendDisposition::RejectedMalformedFrame;
        }
        if let Some(last) = self.frames.back() {
            if last.matches(&frame) {
                return ObserverPresentationAppendDisposition::Duplicate;
            }
            if !snapshot_id_is_strictly_newer(frame.snapshot_id, last.snapshot_id) {
                return ObserverPresentationAppendDisposition::RejectedStaleSnapshot;
            }
        }
        self.frames
            .push_back(presentation_storage::StoredFrame::from_validated(frame));
        while self.frames.len() > OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS {
            self.frames.pop_front();
            self.evictions = self.evictions.saturating_add(1);
        }
        ObserverPresentationAppendDisposition::Applied
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn evictions(&self) -> u64 {
        self.evictions
    }

    pub(crate) fn frames(
        &self,
    ) -> impl DoubleEndedIterator<Item = &presentation_storage::StoredFrame> + '_ {
        self.frames.iter()
    }

    /// Whether snapshot `snapshot_id` was emitted to exactly `observer`.
    #[allow(dead_code)] // server only
    pub(crate) fn emitted_to(&self, observer: PlayerRuntimeIdentity, snapshot_id: u16) -> bool {
        self.frames
            .iter()
            .any(|frame| frame.snapshot_id == snapshot_id && frame.observer == observer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationProjectionDisposition {
    Exact,
    Interpolated,
    HeldNoUpperSample,
    SnappedDiscontinuity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationProjectionUnavailable {
    NoAppliedSample,
    BeforeFirstSample,
    WrongSegment,
    AuthorityHistory(HistoryUnavailable),
    AuthorityErrorExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PresentationGeometryAnchorV1 {
    pub(crate) disposition: PresentationProjectionDisposition,
    pub(crate) render_tick: u32,
    pub(crate) motion_segment: MotionSegmentIdentity,
    pub(crate) lower_snapshot_id: u16,
    pub(crate) upper_snapshot_id: u16,
    pub(crate) lower_server_tick: u32,
    pub(crate) upper_server_tick: u32,
    pub(crate) x_q8: i32,
    pub(crate) y_q8: i32,
    pub(crate) authority_x_q8: i32,
    pub(crate) authority_y_q8: i32,
    pub(crate) authority_error_q8: i32,
}

pub(crate) struct PresentationGeometryResolver;

impl PresentationGeometryResolver {
    pub(crate) fn resolve(
        history: &ObserverPresentationHistory,
        motion_segment: MotionSegmentIdentity,
        render_tick: u32,
        applied_snapshot_id: u16,
        motion: &ActorMotionHistory,
    ) -> Result<PresentationGeometryAnchorV1, PresentationProjectionUnavailable> {
        Self::resolve_with_authority(
            history,
            motion_segment,
            render_tick,
            applied_snapshot_id,
            actor_motion_sample_at_or_before(
                motion.samples.iter().copied(),
                motion_segment,
                render_tick,
            ),
        )
    }

    /// Production projectile evaluation already owns a bounded copy of the
    /// target's exact current-segment samples in `PlayerHullSnapshot`. Resolve
    /// directly over that slice so collision does not clone or allocate
    /// another motion history for every target lookup.
    pub(crate) fn resolve_from_samples(
        history: &ObserverPresentationHistory,
        motion_segment: MotionSegmentIdentity,
        render_tick: u32,
        applied_snapshot_id: u16,
        motion_samples: &[ActorMotionSampleV1],
    ) -> Result<PresentationGeometryAnchorV1, PresentationProjectionUnavailable> {
        Self::resolve_with_authority(
            history,
            motion_segment,
            render_tick,
            applied_snapshot_id,
            actor_motion_sample_at_or_before(
                motion_samples.iter().copied(),
                motion_segment,
                render_tick,
            ),
        )
    }

    fn resolve_with_authority(
        history: &ObserverPresentationHistory,
        motion_segment: MotionSegmentIdentity,
        render_tick: u32,
        applied_snapshot_id: u16,
        authority: Result<ActorMotionSampleV1, HistoryUnavailable>,
    ) -> Result<PresentationGeometryAnchorV1, PresentationProjectionUnavailable> {
        if applied_snapshot_id == 0 {
            return Err(PresentationProjectionUnavailable::NoAppliedSample);
        }
        let mut eligible = history
            .frames()
            .filter(|frame| snapshot_id_reached(applied_snapshot_id, frame.snapshot_id))
            .filter_map(|frame| {
                frame
                    .stored_sample_for_segment(motion_segment)
                    .map(|sample| (frame.snapshot_id, frame.server_tick, sample))
            });
        let mut lower = None;
        let mut upper = None;
        let render_time = PresentationSampleTime::from_tick(render_tick);
        for candidate in eligible.by_ref() {
            let Some(age) = render_time.quanta_since(candidate.2.sample_time()) else {
                continue;
            };
            if age >= 0
                && lower.is_none_or(|current: (u16, u32, &presentation_storage::StoredSample)| {
                    render_time
                        .quanta_since(current.2.sample_time())
                        .is_some_and(|current_age| current_age > age)
                })
            {
                lower = Some(candidate);
            }
            let lead = -age;
            if lead >= 0
                && upper.is_none_or(|current: (u16, u32, &presentation_storage::StoredSample)| {
                    current
                        .2
                        .sample_time()
                        .quanta_since(render_time)
                        .is_some_and(|current_lead| current_lead > lead)
                })
            {
                upper = Some(candidate);
            }
        }
        if lower.is_none() && upper.is_none() {
            let any_segment = history
                .frames()
                .any(|frame| frame.stored_sample_for_segment(motion_segment).is_some());
            return if any_segment {
                Err(PresentationProjectionUnavailable::NoAppliedSample)
            } else {
                Err(PresentationProjectionUnavailable::WrongSegment)
            };
        }
        let decode = |(id, tick, sample): (u16, u32, &presentation_storage::StoredSample)| {
            (id, tick, sample.sample())
        };
        let lower = lower
            .map(decode)
            .ok_or(PresentationProjectionUnavailable::BeforeFirstSample)?;
        let upper = upper.map(decode);
        let (upper, disposition, x_q8, y_q8) = match upper {
            Some(upper) if upper.2.sample_time == lower.2.sample_time => (
                upper,
                PresentationProjectionDisposition::Exact,
                lower.2.x_q8,
                lower.2.y_q8,
            ),
            Some(upper)
                if upper.2.snap_discontinuity
                    || presentation_distance_q8(lower.2, upper.2)
                        > OBSERVER_PRESENTATION_SNAP_DISTANCE_Q8 =>
            {
                (
                    upper,
                    PresentationProjectionDisposition::SnappedDiscontinuity,
                    upper.2.x_q8,
                    upper.2.y_q8,
                )
            }
            Some(upper) => {
                let (x, y) = presentation_hermite_sample_time_q8(
                    lower.2.sample_time,
                    lower.2.kinematics(),
                    upper.2.sample_time,
                    upper.2.kinematics(),
                    render_time,
                )
                .expect("ordered distinct source samples");
                (upper, PresentationProjectionDisposition::Interpolated, x, y)
            }
            None => (
                lower,
                PresentationProjectionDisposition::HeldNoUpperSample,
                lower.2.x_q8,
                lower.2.y_q8,
            ),
        };
        let authority = authority.map_err(PresentationProjectionUnavailable::AuthorityHistory)?;
        let dx = i64::from(x_q8) - i64::from(authority.x_q8);
        let dy = i64::from(y_q8) - i64::from(authority.y_q8);
        let authority_error_q8 = ((dx * dx + dy * dy) as f64).sqrt().round() as i32;
        if authority_error_q8 > PRESENTATION_AUTHORITY_MAX_ERROR_Q8 {
            return Err(PresentationProjectionUnavailable::AuthorityErrorExceeded);
        }
        Ok(PresentationGeometryAnchorV1 {
            disposition,
            render_tick,
            motion_segment,
            lower_snapshot_id: lower.0,
            upper_snapshot_id: upper.0,
            lower_server_tick: lower.1,
            upper_server_tick: upper.1,
            x_q8,
            y_q8,
            authority_x_q8: authority.x_q8,
            authority_y_q8: authority.y_q8,
            authority_error_q8,
        })
    }
}

fn actor_motion_sample_at_or_before(
    samples: impl DoubleEndedIterator<Item = ActorMotionSampleV1>,
    segment: MotionSegmentIdentity,
    target_tick: u32,
) -> Result<ActorMotionSampleV1, HistoryUnavailable> {
    let target = PresentationSampleTime::from_tick(target_tick);
    let mut any = false;
    let mut scoped = false;
    // The contract selects the last eligible retained sample, not the sample
    // with the greatest timestamp. Walking backward preserves that contract
    // (including repeated/nonmonotonic times) and stops at the usual recent
    // match instead of visiting every older sample in the retained ring.
    for sample in samples.rev() {
        any = true;
        if sample.segment != segment {
            continue;
        }
        scoped = true;
        if target
            .quanta_since(sample.sample_time)
            .is_some_and(|age| age >= 0)
        {
            return Ok(sample);
        }
    }
    Err(if !any {
        HistoryUnavailable::Empty
    } else if !scoped {
        HistoryUnavailable::WrongSegment
    } else {
        HistoryUnavailable::BeforeRetainedWindow
    })
}

fn snapshot_id_is_strictly_newer(candidate: u16, predecessor: u16) -> bool {
    let distance = candidate.wrapping_sub(predecessor);
    distance != 0 && distance < (1 << 15)
}

fn snapshot_id_reached(current: u16, candidate: u16) -> bool {
    current == candidate || snapshot_id_is_strictly_newer(current, candidate)
}

fn presentation_distance_q8(
    lower: ObserverPresentationSampleV1,
    upper: ObserverPresentationSampleV1,
) -> i32 {
    let dx = i64::from(upper.x_q8) - i64::from(lower.x_q8);
    let dy = i64::from(upper.y_q8) - i64::from(lower.y_q8);
    ((dx * dx + dy * dy) as f64).sqrt().round() as i32
}

/// Server-owned sample time with exact thirds of a 100 Hz tick. Five quanta
/// represent one 60 Hz simulation step. This is geometry, not a client clock
/// claim; actor scope and publication eligibility remain caller-owned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PresentationSampleTime {
    tick: u32,
    third: u8,
}

/// Bounded server-side mapping of proven movement steps to presentation time.
/// This does not accept client timestamps or infer omitted steps from a tail.
/// The admission owner must supply each canonical replay witness in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct AcceptedMotionSampleClock {
    segment: MotionSegmentIdentity,
    sequence: u16,
    time: PresentationSampleTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum SampleClockAdmission {
    Recorded(PresentationSampleTime),
    WrongSegment,
    MissingOrStaleStep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum SampleClockPublication {
    Eligible(PresentationSampleTime),
    NotYetDue,
    NeedsAuthorizedBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum SampleClockReanchorError {
    WrongSegment,
    WrongSequence,
    BackwardOrAmbiguousTime,
}

#[cfg_attr(not(test), allow(dead_code))]
impl AcceptedMotionSampleClock {
    pub(crate) fn at_authorized_boundary(
        segment: MotionSegmentIdentity,
        sequence: u16,
        server_tick: u32,
    ) -> Self {
        Self {
            segment,
            sequence,
            time: PresentationSampleTime::from_tick(server_tick),
        }
    }

    pub(crate) fn append_proven_step(
        &mut self,
        segment: MotionSegmentIdentity,
        sequence: u16,
    ) -> SampleClockAdmission {
        if segment != self.segment {
            return SampleClockAdmission::WrongSegment;
        }
        if sequence != self.sequence.wrapping_add(1) {
            return SampleClockAdmission::MissingOrStaleStep;
        }
        let quanta = self.time.third + 5;
        self.time.tick =
            crate::continuum::continuum_tick_add(self.time.tick, u32::from(quanta / 3));
        self.time.third = quanta % 3;
        self.sequence = sequence;
        SampleClockAdmission::Recorded(self.time)
    }

    /// A timing-only recovery at an already-accepted tail. Unlike an actual
    /// movement/lifecycle boundary, this cannot change actor identity, physical
    /// motion segment, or input sequence. Previously emitted times are values
    /// and must not be recomputed from this mutable current anchor.
    pub(crate) fn reanchor_accepted_tail(
        &mut self,
        segment: MotionSegmentIdentity,
        sequence: u16,
        server_tick: u32,
    ) -> Result<PresentationSampleTime, SampleClockReanchorError> {
        if segment != self.segment {
            return Err(SampleClockReanchorError::WrongSegment);
        }
        if sequence != self.sequence {
            return Err(SampleClockReanchorError::WrongSequence);
        }
        let next = PresentationSampleTime::from_tick(server_tick);
        if !next.quanta_since(self.time).is_some_and(|delta| delta >= 0) {
            return Err(SampleClockReanchorError::BackwardOrAmbiguousTime);
        }
        self.time = next;
        Ok(next)
    }

    pub(crate) fn publication_at(
        &self,
        server_tick: u32,
        max_skew_ticks: u32,
    ) -> SampleClockPublication {
        let now = PresentationSampleTime::from_tick(server_tick);
        let Some(skew) = now.quanta_since(self.time) else {
            return SampleClockPublication::NeedsAuthorizedBoundary;
        };
        if skew.unsigned_abs() > u64::from(max_skew_ticks) * 3 {
            SampleClockPublication::NeedsAuthorizedBoundary
        } else if skew < 0 {
            SampleClockPublication::NotYetDue
        } else {
            SampleClockPublication::Eligible(self.time)
        }
    }

    /// Called only after a complete, proven batch. Recover if its tail is
    /// older than even the least-delayed observer's window. The bound is the
    /// existing presentation floor, not a new delay or a client clock claim.
    /// Future samples remain future; absent packets cannot call this path.
    fn recover_after_accepted_batch(
        &mut self,
        server_tick: u32,
    ) -> Result<Option<PresentationSampleTime>, SampleClockReanchorError> {
        let now = PresentationSampleTime::from_tick(server_tick);
        let age = now
            .quanta_since(self.time)
            .ok_or(SampleClockReanchorError::BackwardOrAmbiguousTime)?;
        if age <= i64::from(OBSERVER_DELAY_MIN_TICKS) * 3 {
            return Ok(None);
        }
        self.reanchor_accepted_tail(self.segment, self.sequence, server_tick)
            .map(Some)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl PresentationSampleTime {
    pub(crate) const fn from_tick(tick: u32) -> Self {
        Self {
            tick: tick & crate::continuum::CONTINUUM_TICK_MASK,
            third: 0,
        }
    }

    pub(crate) const fn parts(self) -> (u32, u8) {
        (self.tick, self.third)
    }

    pub(crate) fn from_parts(tick: u32, third: u8) -> Option<Self> {
        (third < 3 && tick <= crate::continuum::CONTINUUM_TICK_MASK).then_some(Self { tick, third })
    }

    pub(crate) fn quanta_since(self, earlier: Self) -> Option<i64> {
        let ticks = crate::continuum::continuum_tick_diff(self.tick, earlier.tick);
        (ticks != -(1 << 30))
            .then_some(i64::from(ticks) * 3 + i64::from(self.third) - i64::from(earlier.third))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn presentation_hermite_sample_time_q8(
    lower_time: PresentationSampleTime,
    lower: PresentationKinematicSampleV1,
    upper_time: PresentationSampleTime,
    upper: PresentationKinematicSampleV1,
    render_time: PresentationSampleTime,
) -> Option<(i32, i32)> {
    let span = upper_time.quanta_since(lower_time)?;
    if span <= 0 {
        return None;
    }
    let elapsed = render_time.quanta_since(lower_time)?.clamp(0, span);
    Some(presentation_hermite_with_timing_q8(
        lower,
        upper,
        elapsed as f64 / span as f64,
        span as f64 / 300.0,
    ))
}

pub(crate) fn presentation_hermite_q8(
    lower_tick: u32,
    lower: PresentationKinematicSampleV1,
    upper_tick: u32,
    upper: PresentationKinematicSampleV1,
    render_tick: u32,
) -> (i32, i32) {
    let span_ticks = crate::continuum::continuum_tick_diff(upper_tick, lower_tick).max(1) as f64;
    let elapsed_ticks = crate::continuum::continuum_tick_diff(render_tick, lower_tick)
        .clamp(0, span_ticks as i32) as f64;
    let t = (elapsed_ticks / span_ticks).clamp(0.0, 1.0);
    let seconds = span_ticks * f64::from(COMBAT_TIMELINE_TICK_MILLIS) / 1_000.0;
    presentation_hermite_with_timing_q8(lower, upper, t, seconds)
}

fn presentation_hermite_with_timing_q8(
    lower: PresentationKinematicSampleV1,
    upper: PresentationKinematicSampleV1,
    t: f64,
    seconds: f64,
) -> (i32, i32) {
    let p0 = (f64::from(lower.x_q8), f64::from(lower.y_q8));
    let p1 = (f64::from(upper.x_q8), f64::from(upper.y_q8));
    let m0 = clamp_presentation_tangent(
        f64::from(lower.velocity_x_q8) * seconds,
        f64::from(lower.velocity_y_q8) * seconds,
        p1.0 - p0.0,
        p1.1 - p0.1,
    );
    let m1 = clamp_presentation_tangent(
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

fn clamp_presentation_tangent(
    velocity_x: f64,
    velocity_y: f64,
    chord_x: f64,
    chord_y: f64,
) -> (f64, f64) {
    let chord = (chord_x * chord_x + chord_y * chord_y).sqrt();
    let length = (velocity_x * velocity_x + velocity_y * velocity_y).sqrt();
    if chord <= f64::EPSILON || length <= f64::EPSILON {
        return (0.0, 0.0);
    }
    if length <= chord {
        (velocity_x, velocity_y)
    } else {
        (velocity_x * chord / length, velocity_y * chord / length)
    }
}

fn q8_i32(value: f32) -> i32 {
    (value * 256.0)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

fn q8_u16(value: f32) -> u16 {
    (value.max(0.0) * 256.0).round().clamp(0.0, u16::MAX as f32) as u16
}

fn orientation_u16(orientation: f32) -> u16 {
    // Gameplay orientations are normalized turns, never radians. Cast via
    // u32 so rounding just below one turn wraps instead of saturating.
    (orientation.rem_euclid(1.0) * 65_536.0).round() as u32 as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObserverTimelineDirectiveV1 {
    pub(crate) generation: u32,
    pub(crate) issued_server_tick: u32,
    pub(crate) effective_server_tick: u32,
    pub(crate) effective_from_input_sequence: u16,
    pub(crate) from_delay_ticks: u8,
    pub(crate) target_delay_ticks: u8,
    pub(crate) ceiling_delay_ticks: u8,
    pub(crate) slew_interval_ticks: u8,
    pub(crate) hold_grace_ticks: u8,
    pub(crate) profile: ObserverNetworkProfile,
}

impl ObserverTimelineDirectiveV1 {
    pub(crate) const ENCODED_LEN: usize = 23;

    pub(crate) fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut bytes = [0_u8; Self::ENCODED_LEN];
        bytes[0] = COMBAT_VIEW_PROTOCOL_VERSION;
        bytes[1] = match self.profile {
            ObserverNetworkProfile::CleanLan => 0,
            ObserverNetworkProfile::SupportedMildWan => 1,
            ObserverNetworkProfile::SupportedSlowWan => 2,
            ObserverNetworkProfile::RecoveryStress => 3,
            ObserverNetworkProfile::AutomaticWan => 4,
        };
        bytes[2..6].copy_from_slice(&self.generation.to_le_bytes());
        bytes[6..10].copy_from_slice(&self.issued_server_tick.to_le_bytes());
        bytes[10..14].copy_from_slice(&self.effective_server_tick.to_le_bytes());
        bytes[14..16].copy_from_slice(&self.effective_from_input_sequence.to_le_bytes());
        bytes[16] = self.from_delay_ticks;
        bytes[17] = self.target_delay_ticks;
        bytes[18] = self.ceiling_delay_ticks;
        bytes[19] = self.slew_interval_ticks;
        bytes[20] = self.hold_grace_ticks;
        // Reserved bytes are required to be zero in v1 and make compatible
        // extension explicit rather than inferring shape from packet length.
        bytes[21] = 0;
        bytes[22] = 0;
        bytes
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, ObserverTimelineWireError> {
        if bytes.len() != Self::ENCODED_LEN {
            return Err(ObserverTimelineWireError::Length);
        }
        if bytes[0] != COMBAT_VIEW_PROTOCOL_VERSION {
            return Err(ObserverTimelineWireError::Version);
        }
        if bytes[21] != 0 || bytes[22] != 0 {
            return Err(ObserverTimelineWireError::ReservedBits);
        }
        let profile = match bytes[1] {
            0 => ObserverNetworkProfile::CleanLan,
            1 => ObserverNetworkProfile::SupportedMildWan,
            2 => ObserverNetworkProfile::SupportedSlowWan,
            3 => ObserverNetworkProfile::RecoveryStress,
            4 => ObserverNetworkProfile::AutomaticWan,
            _ => return Err(ObserverTimelineWireError::Profile),
        };
        let directive = Self {
            generation: u32::from_le_bytes(bytes[2..6].try_into().expect("fixed slice")),
            issued_server_tick: u32::from_le_bytes(bytes[6..10].try_into().expect("fixed slice")),
            effective_server_tick: u32::from_le_bytes(
                bytes[10..14].try_into().expect("fixed slice"),
            ),
            effective_from_input_sequence: u16::from_le_bytes(
                bytes[14..16].try_into().expect("fixed slice"),
            ),
            from_delay_ticks: bytes[16],
            target_delay_ticks: bytes[17],
            ceiling_delay_ticks: bytes[18],
            slew_interval_ticks: bytes[19],
            hold_grace_ticks: bytes[20],
            profile,
        };
        directive.validate()?;
        Ok(directive)
    }

    pub(crate) fn validate(self) -> Result<(), ObserverTimelineWireError> {
        if self.generation == 0 {
            return Err(ObserverTimelineWireError::Generation);
        }
        let envelope = self.profile.delay_envelope();
        if self.from_delay_ticks < OBSERVER_DELAY_MIN_TICKS
            || self.target_delay_ticks < OBSERVER_DELAY_MIN_TICKS
            || self.from_delay_ticks > self.ceiling_delay_ticks
            || self.target_delay_ticks > self.ceiling_delay_ticks
            || self.ceiling_delay_ticks != envelope.ceiling_ticks
            || self.ceiling_delay_ticks > OBSERVER_DELAY_MAX_TICKS
        {
            return Err(ObserverTimelineWireError::Delay);
        }
        if self.slew_interval_ticks != OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8 {
            return Err(ObserverTimelineWireError::Slew);
        }
        if self.hold_grace_ticks != OBSERVER_PRESENTATION_HOLD_GRACE_TICKS {
            return Err(ObserverTimelineWireError::HoldGrace);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObserverTimelineWireError {
    Length,
    Version,
    ReservedBits,
    Profile,
    Generation,
    Delay,
    Slew,
    HoldGrace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObserverTimelineScopeError {
    ZeroArenaStream,
    ZeroGeneration,
    WrongSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObserverScheduleRejection {
    NoScope,
    InvalidWire,
    ProfileChanged,
    GenerationNotNewer,
    GenerationTooSoon,
    EffectiveTime,
    EffectiveSequence,
    FromDelayMismatch,
    HistoryCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientObserverTimelineDisposition {
    Applied,
    Pending,
    Duplicate,
    RejectedConflict,
    RejectedStale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientAppliedSnapshotDisposition {
    Applied,
    Duplicate,
    RejectedUnavailable,
    RejectedStale,
}

/// Browser-side consumer of the server-owned observer schedule. It stores no
/// client-selected delay: the only render clock comes from a validated wire
/// directive, and the only receipt watermark comes from a snapshot that has
/// crossed the Game/NetworkWorld application boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientObserverCombatTimeline {
    active: Option<ObserverTimelineDirectiveV1>,
    pending: Option<ObserverTimelineDirectiveV1>,
    latest_game_applied_snapshot: Option<(u16, u32)>,
    presentation_render_tick: Option<u32>,
    presentation_scheduled_tick: Option<u32>,
    presentation_tick_budget_remainder: u64,
}

impl ClientObserverCombatTimeline {
    pub(crate) fn reset(&mut self) {
        self.active = None;
        self.pending = None;
        self.latest_game_applied_snapshot = None;
        self.presentation_render_tick = None;
        self.presentation_scheduled_tick = None;
        self.presentation_tick_budget_remainder = 0;
    }

    pub(crate) fn observe_directive(
        &mut self,
        directive: ObserverTimelineDirectiveV1,
        current_input_sequence: u16,
    ) -> ClientObserverTimelineDisposition {
        if directive.validate().is_err() {
            return ClientObserverTimelineDisposition::RejectedConflict;
        }
        if self.active == Some(directive) || self.pending == Some(directive) {
            return ClientObserverTimelineDisposition::Duplicate;
        }
        if self
            .active
            .is_some_and(|active| active.generation == directive.generation)
            || self
                .pending
                .is_some_and(|pending| pending.generation == directive.generation)
        {
            return ClientObserverTimelineDisposition::RejectedConflict;
        }
        let newest_generation = self.pending.or(self.active).map(|known| known.generation);
        if newest_generation
            .is_some_and(|known| !u32_serial_is_forward(directive.generation, known))
        {
            return ClientObserverTimelineDisposition::RejectedStale;
        }
        self.pending = Some(directive);
        if input_sequence_reached(
            current_input_sequence,
            directive.effective_from_input_sequence,
        ) {
            self.activate_pending();
            ClientObserverTimelineDisposition::Applied
        } else {
            ClientObserverTimelineDisposition::Pending
        }
    }

    pub(crate) fn advance_input(
        &mut self,
        input_sequence: u16,
    ) -> ClientObserverTimelineDisposition {
        let Some(pending) = self.pending else {
            return ClientObserverTimelineDisposition::Duplicate;
        };
        if !input_sequence_reached(input_sequence, pending.effective_from_input_sequence) {
            return ClientObserverTimelineDisposition::Pending;
        }
        self.activate_pending();
        ClientObserverTimelineDisposition::Applied
    }

    pub(crate) fn active_directive(&self) -> Option<ObserverTimelineDirectiveV1> {
        self.active
    }

    pub(crate) fn delay_ticks_at(&self, server_tick: u32) -> Option<u8> {
        self.active
            .map(|directive| observer_delay_ticks_at(directive, server_tick))
    }

    pub(crate) fn render_tick_at(&self, server_tick: u32) -> Option<u32> {
        self.delay_ticks_at(server_tick)
            .map(|delay| crate::continuum::continuum_tick_sub(server_tick, u32::from(delay)))
    }

    /// Advance the actually drawn observer cursor toward the server-owned
    /// schedule without collapsing a browser scheduler hitch into one visible
    /// teleport. The target remains wholly server-derived. Local render time
    /// controls only bounded recovery toward it and cannot run ahead.
    pub(crate) fn advance_presentation_render_tick(
        &mut self,
        server_tick: u32,
        render_dt_seconds: f32,
    ) -> Option<u32> {
        let target = self.render_tick_at(server_tick)?;
        let previous_scheduled_tick = self.presentation_scheduled_tick;
        self.presentation_scheduled_tick = Some(target);
        let Some(current) = self.presentation_render_tick else {
            self.presentation_render_tick = Some(target);
            self.presentation_tick_budget_remainder = 0;
            return Some(target);
        };
        let was_caught_up = previous_scheduled_tick == Some(current);
        let ordinary_dt = bounded_observer_frame_seconds(render_dt_seconds);
        let recovery_dt = ordinary_dt.min(OBSERVER_PRESENTATION_MAX_STEP_SECONDS);
        let ordinary_nanos = (f64::from(ordinary_dt) * 1_000_000_000.0)
            .round()
            .clamp(0.0, u64::MAX as f64) as u64;
        let recovery_nanos = (f64::from(recovery_dt) * 1_000_000_000.0).round() as u64;
        // Continuum source ticks are centiseconds. Do not cap ordinary time
        // with the extra recovery allowance: at 20 Hz that left no budget to
        // retire a backlog, and below 20 Hz it created backlog indefinitely.
        let budget = self
            .presentation_tick_budget_remainder
            .saturating_add(ordinary_nanos.saturating_mul(100))
            .saturating_add(
                recovery_nanos
                    .saturating_mul(OBSERVER_PRESENTATION_RECOVERY_TICKS_PER_SECOND - 100),
            );
        let available_ticks = budget / 1_000_000_000;
        self.presentation_tick_budget_remainder = budget % 1_000_000_000;

        let distance = crate::continuum::continuum_tick_diff(target, current);
        if distance <= 0 {
            if distance < 0 {
                // A hard scope reset or a backward source clock must not leave
                // the renderer on a cursor the active schedule cannot name.
                self.presentation_render_tick = Some(target);
                self.presentation_tick_budget_remainder = 0;
            }
            // Preserve sub-tick elapsed time while caught up. Discarding it
            // here made high-refresh clients report a one-frame false hold
            // whenever the 100 Hz target advanced between 4 ms render frames.
            return self.presentation_render_tick;
        }

        if distance == 1 && was_caught_up {
            // The server-owned 100 Hz target normally advances by one tick
            // between render frames. Following that target is ordinary clock
            // progression, not recovery from a browser scheduler hitch. In
            // particular, a 200+ Hz renderer must not manufacture a one-frame
            // hold merely because less than one 200 Hz recovery quantum has
            // elapsed since the preceding frame.
            self.presentation_render_tick = Some(target);
            return self.presentation_render_tick;
        }

        let advance = available_ticks.min(distance as u64) as u32;
        if advance != 0 {
            self.presentation_render_tick =
                Some(crate::continuum::continuum_tick_add(current, advance));
        }
        self.presentation_render_tick
    }

    pub(crate) fn presentation_render_tick(&self) -> Option<u32> {
        self.presentation_render_tick
    }

    pub(crate) fn presentation_scheduled_tick(&self) -> Option<u32> {
        self.presentation_scheduled_tick
    }

    pub(crate) fn latest_game_applied_snapshot(&self) -> Option<(u16, u32)> {
        self.latest_game_applied_snapshot
    }

    pub(crate) fn observe_game_applied_snapshot(
        &mut self,
        snapshot_id: u16,
        server_tick: u32,
    ) -> ClientAppliedSnapshotDisposition {
        if self.active.is_none() || snapshot_id == 0 {
            return ClientAppliedSnapshotDisposition::RejectedUnavailable;
        }
        if let Some((current_id, current_tick)) = self.latest_game_applied_snapshot {
            if current_id == snapshot_id && current_tick == server_tick {
                return ClientAppliedSnapshotDisposition::Duplicate;
            }
            if !snapshot_id_is_strictly_newer(snapshot_id, current_id)
                || crate::continuum::continuum_tick_diff(server_tick, current_tick) < 0
            {
                return ClientAppliedSnapshotDisposition::RejectedStale;
            }
        }
        self.latest_game_applied_snapshot = Some((snapshot_id, server_tick));
        ClientAppliedSnapshotDisposition::Applied
    }

    pub(crate) fn hold_or_stall_at(&self, server_tick: u32) -> bool {
        let Some(render_tick) = self.render_tick_at(server_tick) else {
            return true;
        };
        self.presentation_render_tick != Some(render_tick)
            || self
                .latest_game_applied_snapshot
                .is_none_or(|(_, applied_tick)| {
                    crate::continuum::continuum_tick_diff(render_tick, applied_tick) > 0
                })
    }

    /// Readiness for the next logical input is evaluated against the schedule
    /// this browser frame actually drew, not a fresh 100 Hz clock sample taken
    /// a few milliseconds later. A phase crossing inside the bounded 25 ms
    /// frame remains current; a later/multi-frame input still fails closed.
    pub(crate) fn input_hold_or_stall_at(&self, server_tick: u32) -> bool {
        let (Some(current_tick), Some(drawn_schedule_tick)) = (
            self.render_tick_at(server_tick),
            self.presentation_scheduled_tick,
        ) else {
            return true;
        };
        let phase_ticks = crate::continuum::continuum_tick_diff(current_tick, drawn_schedule_tick);
        !(0..=OBSERVER_PRESENTATION_INPUT_PHASE_TICKS as i32).contains(&phase_ticks)
            || self.presentation_render_tick != Some(drawn_schedule_tick)
            || self
                .latest_game_applied_snapshot
                .is_none_or(|(_, applied_tick)| {
                    crate::continuum::continuum_tick_diff(drawn_schedule_tick, applied_tick) > 0
                })
    }

    pub(crate) fn receipt(
        &mut self,
        observer_life_seq: Option<u32>,
        input_sequence: u16,
        server_tick: Option<u32>,
    ) -> CombatViewReceiptV1 {
        self.advance_input(input_sequence);
        let (Some(observer_life_seq), Some(server_tick), Some(active), Some((snapshot_id, _))) = (
            observer_life_seq.filter(|life| *life != 0),
            server_tick,
            self.active,
            self.latest_game_applied_snapshot,
        ) else {
            return CombatViewReceiptV1::unavailable(input_sequence);
        };
        CombatViewReceiptV1 {
            observer_life_seq,
            input_sequence,
            timeline_generation: active.generation,
            latest_game_applied_snapshot_id: snapshot_id,
            flags: if self.input_hold_or_stall_at(server_tick) {
                COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL
            } else {
                0
            },
        }
    }

    fn activate_pending(&mut self) {
        self.active = self.pending.take();
        self.latest_game_applied_snapshot = None;
    }
}

fn input_sequence_reached(current: u16, candidate: u16) -> bool {
    current == candidate || current.wrapping_sub(candidate) < (1_u16 << 15)
}

fn u32_serial_is_forward(candidate: u32, current: u32) -> bool {
    let distance = candidate.wrapping_sub(current);
    distance != 0 && distance < (1_u32 << 31)
}

fn observer_delay_ticks_at(directive: ObserverTimelineDirectiveV1, server_tick: u32) -> u8 {
    let elapsed =
        crate::continuum::continuum_tick_diff(server_tick, directive.effective_server_tick).max(0)
            as u32;
    let steps = elapsed / u32::from(directive.slew_interval_ticks.max(1));
    let from = i32::from(directive.from_delay_ticks);
    let target = i32::from(directive.target_delay_ticks);
    let signed_steps = (steps.min(from.abs_diff(target)) as i32) * (target - from).signum();
    (from + signed_steps) as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObserverTimelineInputDecisionV1 {
    pub(crate) generation: u32,
    pub(crate) input_sequence: u16,
    pub(crate) accepted_server_tick: u32,
    pub(crate) render_tick: u32,
    pub(crate) delay_ticks: u8,
    pub(crate) applied_snapshot_id: u16,
    pub(crate) hold_or_stall: bool,
}

impl ObserverTimelineInputDecisionV1 {
    pub(crate) const fn weapon_edge_allowed(self) -> bool {
        !self.hold_or_stall
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObserverTimelineReceiptRejection {
    Unavailable,
    WrongRuntime,
    WrongLife,
    UnknownGeneration,
    GenerationNotEffective,
    SupersededGeneration,
    UnknownAppliedSnapshot,
    AppliedSnapshotFromOtherRuntime,
    AppliedSnapshotAheadOfInput,
    ApplicationTooFarBehind,
    HoldExpired,
    StaleInput,
    DuplicateMismatch,
}

impl ObserverTimelineReceiptRejection {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::Unavailable => "combat_view_unavailable",
            Self::WrongRuntime => "combat_view_wrong_runtime",
            Self::WrongLife => "combat_view_wrong_life",
            Self::UnknownGeneration => "combat_view_unknown_generation",
            Self::GenerationNotEffective => "combat_view_generation_not_effective",
            Self::SupersededGeneration => "combat_view_superseded_generation",
            Self::UnknownAppliedSnapshot => "combat_view_unknown_applied_snapshot",
            Self::AppliedSnapshotFromOtherRuntime => "combat_view_snapshot_wrong_runtime",
            Self::AppliedSnapshotAheadOfInput => "combat_view_snapshot_ahead",
            Self::ApplicationTooFarBehind => "combat_view_application_too_far_behind",
            Self::HoldExpired => "combat_view_hold_expired",
            Self::StaleInput => "combat_view_stale_input",
            Self::DuplicateMismatch => "combat_view_duplicate_mismatch",
        }
    }

    pub(crate) const fn is_security_violation(self) -> bool {
        matches!(
            self,
            Self::WrongRuntime
                | Self::WrongLife
                | Self::UnknownGeneration
                | Self::GenerationNotEffective
                | Self::AppliedSnapshotFromOtherRuntime
                | Self::AppliedSnapshotAheadOfInput
                | Self::DuplicateMismatch
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObserverTimelineObservedReceipt {
    receipt: CombatViewReceiptV1,
    result: Result<ObserverTimelineInputDecisionV1, ObserverTimelineReceiptRejection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObserverInputProgress {
    sequence: u16,
    index: u64,
}

impl ObserverInputProgress {
    fn index_for(self, sequence: u16) -> Option<u64> {
        let distance = sequence.wrapping_sub(self.sequence);
        (distance < (1 << 15))
            .then(|| self.index.checked_add(u64::from(distance)))
            .flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScheduledObserverDirective {
    directive: ObserverTimelineDirectiveV1,
    effective_input_index: u64,
}

impl std::ops::Deref for ScheduledObserverDirective {
    type Target = ObserverTimelineDirectiveV1;
    fn deref(&self) -> &Self::Target {
        &self.directive
    }
}

/// Server-owned schedule for one authenticated observer arena stream. Soft
/// life changes replace the exact observer runtime while preserving the one
/// observer-wide clock; hard arena/session changes allocate a new generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObserverCombatTimeline {
    observer: Option<PlayerRuntimeIdentity>,
    arena_stream_id: Option<u64>,
    profile: ObserverNetworkProfile,
    directives: VecDeque<ScheduledObserverDirective>,
    observed_receipts: VecDeque<ObserverTimelineObservedReceipt>,
    last_input_sequence: Option<u16>,
    input_progress: Option<ObserverInputProgress>,
    /// One monotonic valid application watermark survives eviction of the
    /// per-input audit ring. In-flight projectiles therefore never depend on
    /// whether a later run of rejected/stalled inputs consumed 64 diagnostics.
    latest_verified_projection_decision: Option<ObserverTimelineInputDecisionV1>,
    latest_verified_input_index: Option<u64>,
}

impl Default for ObserverCombatTimeline {
    fn default() -> Self {
        Self {
            observer: None,
            arena_stream_id: None,
            profile: ObserverNetworkProfile::CleanLan,
            directives: VecDeque::with_capacity(OBSERVER_SCHEDULE_HISTORY_CAPACITY),
            observed_receipts: VecDeque::with_capacity(OBSERVER_INPUT_DECISION_HISTORY_CAPACITY),
            last_input_sequence: None,
            input_progress: None,
            latest_verified_projection_decision: None,
            latest_verified_input_index: None,
        }
    }
}

impl ObserverCombatTimeline {
    pub(crate) fn begin_scope(
        &mut self,
        observer: PlayerRuntimeIdentity,
        arena_stream_id: u64,
        profile: ObserverNetworkProfile,
        generation: u32,
        server_tick: u32,
        effective_from_input_sequence: u16,
    ) -> Result<ObserverTimelineDirectiveV1, ObserverTimelineScopeError> {
        if arena_stream_id == 0 {
            return Err(ObserverTimelineScopeError::ZeroArenaStream);
        }
        if generation == 0 {
            return Err(ObserverTimelineScopeError::ZeroGeneration);
        }
        let envelope = profile.delay_envelope();
        let directive = ObserverTimelineDirectiveV1 {
            generation,
            issued_server_tick: server_tick,
            effective_server_tick: server_tick,
            effective_from_input_sequence,
            from_delay_ticks: envelope.target_ticks,
            target_delay_ticks: envelope.target_ticks,
            ceiling_delay_ticks: envelope.ceiling_ticks,
            slew_interval_ticks: OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
            hold_grace_ticks: OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
            profile,
        };
        directive
            .validate()
            .expect("trusted timeline scope creates a valid directive");
        self.observer = Some(observer);
        self.arena_stream_id = Some(arena_stream_id);
        self.profile = profile;
        self.directives.clear();
        self.directives.push_back(ScheduledObserverDirective {
            directive,
            effective_input_index: 0,
        });
        self.input_progress = Some(ObserverInputProgress {
            sequence: effective_from_input_sequence,
            index: 0,
        });
        self.observed_receipts.clear();
        self.last_input_sequence = None;
        self.latest_verified_projection_decision = None;
        self.latest_verified_input_index = None;
        Ok(directive)
    }

    /// Retain only the schedule and monotonic watermark needed by admitted
    /// flight, not a second input-receipt audit ring for every retired life.
    pub(crate) fn retained_projectile_context(&self) -> Self {
        Self {
            observed_receipts: VecDeque::new(),
            last_input_sequence: None,
            ..self.clone()
        }
    }

    pub(crate) fn advance_life(
        &mut self,
        observer: PlayerRuntimeIdentity,
    ) -> Result<(), ObserverTimelineScopeError> {
        if self.observer.is_some_and(|current| {
            current.session != observer.session || current.player_id() != observer.player_id()
        }) {
            return Err(ObserverTimelineScopeError::WrongSession);
        }
        self.observer = Some(observer);
        self.observed_receipts.clear();
        self.last_input_sequence = None;
        self.latest_verified_projection_decision = None;
        self.latest_verified_input_index = None;
        Ok(())
    }

    pub(crate) fn current_directive(&self) -> Option<ObserverTimelineDirectiveV1> {
        self.directives.back().map(|entry| entry.directive)
    }

    /// Install a trusted server schedule, not a client request. Keep profile
    /// and ceiling fixed for the entire arena stream so existing projectile
    /// bindings remain valid. Future tick/sequence boundaries permit delivery
    /// before activation; the existing receipt path freezes each input's
    /// first observation and never upgrades an old failed weapon decision.
    pub(crate) fn issue_delay_schedule(
        &mut self,
        directive: ObserverTimelineDirectiveV1,
    ) -> Result<(), ObserverScheduleRejection> {
        self.issue_delay_schedule_inner(directive, false)
    }

    /// Server caller must be an admitted spectator. Spectators consume no
    /// gameplay inputs, so repeated policy offers name their stopped input
    /// boundary. This path cannot replace a verified player input decision.
    pub(crate) fn issue_observer_delay_schedule(
        &mut self,
        directive: ObserverTimelineDirectiveV1,
    ) -> Result<(), ObserverScheduleRejection> {
        if self.profile != ObserverNetworkProfile::AutomaticWan
            || self.last_input_sequence.is_some()
        {
            return Err(ObserverScheduleRejection::EffectiveSequence);
        }
        self.issue_delay_schedule_inner(directive, true)
    }

    fn issue_delay_schedule_inner(
        &mut self,
        directive: ObserverTimelineDirectiveV1,
        stopped_observer: bool,
    ) -> Result<(), ObserverScheduleRejection> {
        use ObserverScheduleRejection::*;
        let previous = self.current_directive().ok_or(NoScope)?;
        directive.validate().map_err(|_| InvalidWire)?;
        if directive.profile != self.profile
            || directive.ceiling_delay_ticks != previous.ceiling_delay_ticks
        {
            return Err(ProfileChanged);
        }
        if !u32_serial_is_forward(directive.generation, previous.generation) {
            return Err(GenerationNotNewer);
        }
        if crate::continuum::continuum_tick_diff(
            directive.issued_server_tick,
            previous.issued_server_tick,
        ) < OBSERVER_TIMELINE_MIN_GENERATION_TICKS as i32
        {
            return Err(GenerationTooSoon);
        }
        let lead = crate::continuum::continuum_tick_diff(
            directive.effective_server_tick,
            directive.issued_server_tick,
        );
        if !(0..=200).contains(&lead)
            || crate::continuum::continuum_tick_diff(
                directive.issued_server_tick,
                previous.effective_server_tick,
            ) < 0
        {
            return Err(EffectiveTime);
        }
        let cursor = self.input_progress.ok_or(NoScope)?;
        let effective_input_index = cursor
            .index_for(directive.effective_from_input_sequence)
            .ok_or(EffectiveSequence)?;
        let previous_input_index = self
            .directives
            .back()
            .expect("scoped directive")
            .effective_input_index;
        if effective_input_index < previous_input_index
            || (effective_input_index == previous_input_index && !stopped_observer)
            || (self.last_input_sequence.is_some() && effective_input_index <= cursor.index)
        {
            return Err(EffectiveSequence);
        }
        if self.delay_ticks_at(directive.effective_server_tick) != Some(directive.from_delay_ticks)
            || (lead > 0
                && self.delay_ticks_at(directive.issued_server_tick)
                    != Some(directive.from_delay_ticks))
        {
            return Err(FromDelayMismatch);
        }
        // An old generation might have remained active for hours before this
        // update. Its ISSUE age says nothing about the age of its last shot.
        // Evict only at capacity and only after its successor has been active
        // for the full retained-flight horizon; otherwise skip the update.
        if self.directives.len() >= OBSERVER_SCHEDULE_HISTORY_CAPACITY {
            let safe_to_evict = self.directives.get(1).is_some_and(|next| {
                crate::continuum::continuum_tick_diff(
                    directive.issued_server_tick,
                    next.effective_server_tick,
                ) > OBSERVER_SCHEDULE_RETENTION_TICKS as i32
            });
            if !safe_to_evict {
                return Err(HistoryCapacity);
            }
        }
        self.directives.push_back(ScheduledObserverDirective {
            directive,
            effective_input_index,
        });
        while self.directives.len() > OBSERVER_SCHEDULE_HISTORY_CAPACITY {
            self.directives.pop_front();
        }
        Ok(())
    }

    pub(crate) fn observer(&self) -> Option<PlayerRuntimeIdentity> {
        self.observer
    }

    pub(crate) fn arena_stream_id(&self) -> Option<u64> {
        self.arena_stream_id
    }

    pub(crate) fn profile(&self) -> ObserverNetworkProfile {
        self.profile
    }

    pub(crate) fn delay_ticks_at(&self, server_tick: u32) -> Option<u8> {
        let directive = self
            .directives
            .iter()
            .rev()
            .map(|entry| entry.directive)
            .find(|directive| {
                crate::continuum::continuum_tick_diff(server_tick, directive.effective_server_tick)
                    >= 0
            })
            .or_else(|| self.directives.front().map(|entry| entry.directive))?;
        Some(observer_delay_ticks_at(directive, server_tick))
    }

    pub(crate) fn render_tick_at(&self, server_tick: u32) -> Option<u32> {
        self.delay_ticks_at(server_tick)
            .map(|delay| crate::continuum::continuum_tick_sub(server_tick, u32::from(delay)))
    }

    pub(crate) fn verify_receipt(
        &mut self,
        authenticated_runtime: PlayerRuntimeIdentity,
        server_tick: u32,
        receipt: CombatViewReceiptV1,
        presentation_history: &ObserverPresentationHistory,
    ) -> Result<ObserverTimelineInputDecisionV1, ObserverTimelineReceiptRejection> {
        if self.observer != Some(authenticated_runtime) {
            if receipt.observer_life_seq == 0
                && receipt.timeline_generation == 0
                && receipt.latest_game_applied_snapshot_id == 0
            {
                return Err(ObserverTimelineReceiptRejection::Unavailable);
            }
            return Err(ObserverTimelineReceiptRejection::WrongRuntime);
        }
        if let Some(existing) = self
            .observed_receipts
            .iter()
            .find(|observed| observed.receipt.input_sequence == receipt.input_sequence)
            .copied()
        {
            return if existing.receipt == receipt {
                existing.result
            } else {
                Err(ObserverTimelineReceiptRejection::DuplicateMismatch)
            };
        }

        let result = self.verify_new_receipt(
            authenticated_runtime,
            server_tick,
            receipt,
            presentation_history,
        );
        let input_index = self
            .input_progress
            .and_then(|cursor| cursor.index_for(receipt.input_sequence));
        if let (Ok(decision), Some(index)) = (result, input_index) {
            if self
                .latest_verified_input_index
                .is_none_or(|previous| index > previous)
            {
                self.latest_verified_projection_decision = Some(decision);
                self.latest_verified_input_index = Some(index);
            }
        }
        if let Some(index) = input_index {
            if self.last_input_sequence.is_none_or(|previous| {
                snapshot_id_is_strictly_newer(receipt.input_sequence, previous)
            }) {
                self.last_input_sequence = Some(receipt.input_sequence);
                self.input_progress = Some(ObserverInputProgress {
                    sequence: receipt.input_sequence,
                    index,
                });
            }
        }
        self.observed_receipts
            .push_back(ObserverTimelineObservedReceipt { receipt, result });
        while self.observed_receipts.len() > OBSERVER_INPUT_DECISION_HISTORY_CAPACITY {
            self.observed_receipts.pop_front();
        }
        result
    }

    pub(crate) fn decision_for_sequence(
        &self,
        input_sequence: u16,
    ) -> Option<ObserverTimelineInputDecisionV1> {
        self.observed_receipts.iter().find_map(|observed| {
            (observed.receipt.input_sequence == input_sequence)
                .then_some(observed.result.ok())
                .flatten()
        })
    }

    /// Read-only attribution for an input's frozen first observation. This
    /// cannot replace a rejected receipt or grant weapon permission later.
    pub(crate) fn receipt_rejection_for_sequence(
        &self,
        input_sequence: u16,
    ) -> Option<ObserverTimelineReceiptRejection> {
        self.observed_receipts.iter().find_map(|observed| {
            (observed.receipt.input_sequence == input_sequence)
                .then_some(observed.result.err())
                .flatten()
        })
    }

    /// Legacy relay/AOI fixtures predate the input wire path and exercise
    /// unrelated position/weapon rules. Give those fixtures an explicit
    /// server-owned decision without teaching production to accept an
    /// unavailable receipt. Focused receipt and browser tests always use
    /// `verify_receipt`.
    #[cfg(test)]
    pub(crate) fn seed_input_decision_for_test(
        &mut self,
        input_sequence: u16,
        accepted_server_tick: u32,
    ) -> Option<ObserverTimelineInputDecisionV1> {
        if let Some(existing) = self.decision_for_sequence(input_sequence) {
            return Some(existing);
        }
        let observer = self.observer?;
        // These older fixtures choose arbitrary raw sequences independently
        // of the shared test thread's sequence allocator. Production never
        // bypasses the directive's effective interval.
        let directive = self.current_directive()?;
        let delay_ticks = observer_delay_ticks_at(directive, accepted_server_tick);
        let decision = ObserverTimelineInputDecisionV1 {
            generation: directive.generation,
            input_sequence,
            accepted_server_tick,
            render_tick: crate::continuum::continuum_tick_sub(
                accepted_server_tick,
                u32::from(delay_ticks),
            ),
            delay_ticks,
            applied_snapshot_id: 1,
            hold_or_stall: false,
        };
        self.observed_receipts
            .push_back(ObserverTimelineObservedReceipt {
                receipt: CombatViewReceiptV1 {
                    observer_life_seq: observer.life.life_seq,
                    input_sequence,
                    timeline_generation: directive.generation,
                    latest_game_applied_snapshot_id: 1,
                    flags: 0,
                },
                result: Ok(decision),
            });
        self.latest_verified_projection_decision = Some(decision);
        let index = self
            .input_progress
            .and_then(|cursor| cursor.index_for(input_sequence))
            .unwrap_or_else(|| {
                self.input_progress
                    .map_or(0, |cursor| cursor.index.saturating_add(1))
            });
        self.input_progress = Some(ObserverInputProgress {
            sequence: input_sequence,
            index,
        });
        self.latest_verified_input_index = Some(index);
        while self.observed_receipts.len() > OBSERVER_INPUT_DECISION_HISTORY_CAPACITY {
            self.observed_receipts.pop_front();
        }
        if self
            .last_input_sequence
            .is_none_or(|previous| snapshot_id_is_strictly_newer(input_sequence, previous))
        {
            self.last_input_sequence = Some(input_sequence);
        }
        Some(decision)
    }

    pub(crate) fn bind_projectile(
        &self,
        authenticated_runtime: PlayerRuntimeIdentity,
        input_sequence: u16,
        accepted_server_tick: u32,
    ) -> Result<ProjectileCombatTimelineV1, ProjectileTimelineBindingRejection> {
        if self.observer != Some(authenticated_runtime) {
            return Err(ProjectileTimelineBindingRejection::WrongRuntime);
        }
        let decision = self
            .decision_for_sequence(input_sequence)
            .ok_or(ProjectileTimelineBindingRejection::UnverifiedInput)?;
        if !decision.weapon_edge_allowed() {
            return Err(ProjectileTimelineBindingRejection::HeldOrStalled);
        }
        let directive = self
            .directives
            .iter()
            .find(|directive| directive.generation == decision.generation)
            .map(|entry| entry.directive)
            .ok_or(ProjectileTimelineBindingRejection::UnknownGeneration)?;
        Ok(ProjectileCombatTimelineV1 {
            owner: authenticated_runtime,
            owner_input_sequence: input_sequence,
            accepted_timeline_generation: decision.generation,
            accepted_game_applied_snapshot_id: decision.applied_snapshot_id,
            accepted_server_tick,
            maximum_rewind_ticks: directive.ceiling_delay_ticks,
        })
    }

    /// Resolve the server-owned observer schedule and bounded application
    /// watermark for an already-admitted human projectile at one collision
    /// tick. The client cannot choose a target, render tick, or delay here.
    /// A withheld watermark may hold only for the issued grace; afterward the
    /// server advances to its latest emitted frame so an in-flight projectile
    /// cannot be turned into victim-controlled immunity.
    pub(crate) fn projectile_presentation_query(
        &self,
        binding: ProjectileCombatTimelineV1,
        collision_tick: u32,
        presentation_history: &ObserverPresentationHistory,
    ) -> Result<ProjectilePresentationQueryV1, ProjectilePresentationQueryRejection> {
        if self.observer != Some(binding.owner) {
            return Err(ProjectilePresentationQueryRejection::WrongRuntime);
        }
        if crate::continuum::continuum_tick_diff(collision_tick, binding.accepted_server_tick) < 0 {
            return Err(ProjectilePresentationQueryRejection::CollisionBeforeAcceptance);
        }
        if !self
            .directives
            .iter()
            .any(|directive| directive.generation == binding.accepted_timeline_generation)
        {
            return Err(ProjectilePresentationQueryRejection::UnknownAcceptedGeneration);
        }
        let directive = self
            .directives
            .iter()
            .rev()
            .map(|entry| entry.directive)
            .find(|directive| {
                crate::continuum::continuum_tick_diff(
                    collision_tick,
                    directive.effective_server_tick,
                ) >= 0
            })
            .ok_or(ProjectilePresentationQueryRejection::NoEffectiveSchedule)?;
        let delay_ticks = observer_delay_ticks_at(directive, collision_tick);
        if delay_ticks > binding.maximum_rewind_ticks {
            return Err(ProjectilePresentationQueryRejection::MaximumRewindExceeded);
        }
        let render_tick =
            crate::continuum::continuum_tick_sub(collision_tick, u32::from(delay_ticks));
        let decision = self
            .latest_verified_projection_decision
            .filter(|decision| {
                input_sequence_reached(decision.input_sequence, binding.owner_input_sequence)
            })
            .ok_or(ProjectilePresentationQueryRejection::AcceptedDecisionEvicted)?;
        let mut applied_snapshot_id = decision.applied_snapshot_id;
        let mut server_advanced_watermark = false;
        let applied_frame = presentation_history
            .frames()
            .find(|frame| frame.snapshot_id == applied_snapshot_id);
        let watermark_expired = applied_frame.is_none_or(|frame| {
            crate::continuum::continuum_tick_diff(render_tick, frame.server_tick)
                > i32::from(directive.hold_grace_ticks)
        });
        if watermark_expired {
            applied_snapshot_id = presentation_history
                .frames()
                .next_back()
                .map(|frame| frame.snapshot_id)
                .ok_or(ProjectilePresentationQueryRejection::NoEmittedPresentationFrame)?;
            server_advanced_watermark = true;
        }
        Ok(ProjectilePresentationQueryV1 {
            accepted_timeline_generation: binding.accepted_timeline_generation,
            effective_timeline_generation: directive.generation,
            render_tick,
            applied_snapshot_id,
            server_advanced_watermark,
        })
    }

    fn verify_new_receipt(
        &self,
        authenticated_runtime: PlayerRuntimeIdentity,
        server_tick: u32,
        receipt: CombatViewReceiptV1,
        presentation_history: &ObserverPresentationHistory,
    ) -> Result<ObserverTimelineInputDecisionV1, ObserverTimelineReceiptRejection> {
        if receipt.observer_life_seq == 0
            || receipt.timeline_generation == 0
            || receipt.latest_game_applied_snapshot_id == 0
        {
            return Err(ObserverTimelineReceiptRejection::Unavailable);
        }
        if receipt.observer_life_seq != authenticated_runtime.life.life_seq {
            return Err(ObserverTimelineReceiptRejection::WrongLife);
        }
        if self.last_input_sequence.is_some_and(|previous| {
            !snapshot_id_is_strictly_newer(receipt.input_sequence, previous)
        }) {
            return Err(ObserverTimelineReceiptRejection::StaleInput);
        }
        let input_index = self
            .input_progress
            .and_then(|cursor| cursor.index_for(receipt.input_sequence))
            .ok_or(ObserverTimelineReceiptRejection::GenerationNotEffective)?;
        let (schedule_index, selected) = self
            .directives
            .iter()
            .enumerate()
            .rev()
            .find(|(_, entry)| entry.effective_input_index <= input_index)
            .ok_or(ObserverTimelineReceiptRejection::GenerationNotEffective)?;
        let mut directive = selected.directive;
        if receipt.timeline_generation != directive.generation {
            // During an auto offer's delivery lead, the immediately previous
            // generation can describe EXACTLY the same server-owned clock.
            // Accept only that equivalence, never a client-selected delay or
            // an unknown/newer generation. Once the clocks diverge, the old
            // description cannot authorize a new weapon edge.
            let equivalent = self.profile == ObserverNetworkProfile::AutomaticWan
                && schedule_index > 0
                && self
                    .directives
                    .get(schedule_index - 1)
                    .is_some_and(|previous| {
                        previous.generation == receipt.timeline_generation
                            && previous.effective_input_index <= input_index
                            && self.delay_ticks_at(server_tick)
                                == Some(observer_delay_ticks_at(previous.directive, server_tick))
                            && observer_delay_ticks_at(directive, server_tick)
                                == observer_delay_ticks_at(previous.directive, server_tick)
                    });
            if equivalent {
                directive = self.directives[schedule_index - 1].directive;
            } else {
                return match self
                    .directives
                    .iter()
                    .position(|candidate| candidate.generation == receipt.timeline_generation)
                {
                    // The adaptive offer can overtake a paused browser's
                    // already captured input. Once the clocks diverge that
                    // old proof stays unusable, just like an aged snapshot;
                    // it is not evidence of a forged future schedule. This
                    // changes attribution only, never the admitted decision.
                    Some(index)
                        if self.profile == ObserverNetworkProfile::AutomaticWan
                            && index < schedule_index =>
                    {
                        Err(ObserverTimelineReceiptRejection::SupersededGeneration)
                    }
                    Some(_) => Err(ObserverTimelineReceiptRejection::GenerationNotEffective),
                    None => Err(ObserverTimelineReceiptRejection::UnknownGeneration),
                };
            }
        }
        let emitted = presentation_history
            .frames()
            .find(|frame| frame.snapshot_id == receipt.latest_game_applied_snapshot_id)
            .ok_or(ObserverTimelineReceiptRejection::UnknownAppliedSnapshot)?;
        if emitted.observer != authenticated_runtime {
            return Err(ObserverTimelineReceiptRejection::AppliedSnapshotFromOtherRuntime);
        }
        if crate::continuum::continuum_tick_diff(server_tick, emitted.server_tick) < 0 {
            return Err(ObserverTimelineReceiptRejection::AppliedSnapshotAheadOfInput);
        }
        let delay_ticks = observer_delay_ticks_at(directive, server_tick);
        let render_tick = crate::continuum::continuum_tick_sub(server_tick, u32::from(delay_ticks));
        let application_age =
            crate::continuum::continuum_tick_diff(render_tick, emitted.server_tick);
        // Load pressure makes the server skip a recipient's whole snapshot frame to protect its tick budget
        // (`snapshot.rs`, `LOAD_SHED_FRAME_THRESHOLD`), and a skipped frame is never appended to the observer's
        // presentation history. The ceiling below is measured in ticks while shedding is decided in wall clock, so
        // an unmodified age check refuses a recipient for being behind frames the server itself declined to send:
        // the client freezes while the server considers it healthy. Measured at 200 native peers, the applied
        // snapshot equalled the newest published one at 32/32 and 20/20 while the input was still refused.
        //
        // So refuse only what the client is actually behind. If it has applied the newest frame it was ever sent,
        // it is current with this server's delivery and there is nothing for the ceiling to punish. If it is behind
        // the newest sent frame, the ceiling applies unchanged - this narrows the rejection rather than relaxing it.
        let applied_is_newest_sent = presentation_history
            .frames()
            .next_back()
            .is_some_and(|newest| newest.snapshot_id == emitted.snapshot_id);
        if receipt.hold_or_stall() {
            if application_age > i32::from(directive.hold_grace_ticks) {
                return Err(ObserverTimelineReceiptRejection::HoldExpired);
            }
        } else if application_age > i32::from(directive.hold_grace_ticks) && !applied_is_newest_sent
        {
            return Err(ObserverTimelineReceiptRejection::ApplicationTooFarBehind);
        }
        Ok(ObserverTimelineInputDecisionV1 {
            generation: directive.generation,
            input_sequence: receipt.input_sequence,
            accepted_server_tick: server_tick,
            render_tick,
            delay_ticks,
            applied_snapshot_id: receipt.latest_game_applied_snapshot_id,
            hold_or_stall: receipt.hold_or_stall(),
        })
    }
}

pub(crate) const COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL: u8 = 1 << 0;
const COMBAT_VIEW_RECEIPT_KNOWN_FLAGS: u8 = COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL;

/// Compact C2S receipt/reference. Session, player and arena are intentionally
/// absent because the authenticated connection owns them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CombatViewReceiptV1 {
    pub(crate) observer_life_seq: u32,
    pub(crate) input_sequence: u16,
    pub(crate) timeline_generation: u32,
    pub(crate) latest_game_applied_snapshot_id: u16,
    pub(crate) flags: u8,
}

impl CombatViewReceiptV1 {
    pub(crate) const ENCODED_LEN: usize = 14;

    /// Versioned fail-closed placeholder used before the first server
    /// directive and Game-applied snapshot exist. Movement may still recover;
    /// weapon admission must reject generation/snapshot zero.
    pub(crate) const fn unavailable(input_sequence: u16) -> Self {
        Self {
            observer_life_seq: 0,
            input_sequence,
            timeline_generation: 0,
            latest_game_applied_snapshot_id: 0,
            flags: 0,
        }
    }

    pub(crate) fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut bytes = [0_u8; Self::ENCODED_LEN];
        bytes[0] = COMBAT_VIEW_PROTOCOL_VERSION;
        bytes[1] = self.flags;
        bytes[2..6].copy_from_slice(&self.observer_life_seq.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.input_sequence.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.timeline_generation.to_le_bytes());
        bytes[12..14].copy_from_slice(&self.latest_game_applied_snapshot_id.to_le_bytes());
        bytes
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CombatViewReceiptWireError> {
        if bytes.len() != Self::ENCODED_LEN {
            return Err(CombatViewReceiptWireError::Length);
        }
        if bytes[0] != COMBAT_VIEW_PROTOCOL_VERSION {
            return Err(CombatViewReceiptWireError::Version);
        }
        if bytes[1] & !COMBAT_VIEW_RECEIPT_KNOWN_FLAGS != 0 {
            return Err(CombatViewReceiptWireError::Flags);
        }
        Ok(Self {
            observer_life_seq: u32::from_le_bytes(bytes[2..6].try_into().expect("fixed slice")),
            input_sequence: u16::from_le_bytes(bytes[6..8].try_into().expect("fixed slice")),
            timeline_generation: u32::from_le_bytes(bytes[8..12].try_into().expect("fixed slice")),
            latest_game_applied_snapshot_id: u16::from_le_bytes(
                bytes[12..14].try_into().expect("fixed slice"),
            ),
            flags: bytes[1],
        })
    }

    pub(crate) const fn hold_or_stall(self) -> bool {
        self.flags & COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL != 0
    }

    /// Local prediction may materialize a weapon only when the exact receipt
    /// that will accompany this logical input can be bound by the server.
    /// An unavailable receipt deliberately has zero identity fields even
    /// though it carries no hold bit, so checking the flag alone is unsafe.
    pub(crate) const fn weapon_submission_ready(self) -> bool {
        self.observer_life_seq != 0
            && self.timeline_generation != 0
            && self.latest_game_applied_snapshot_id != 0
            && !self.hold_or_stall()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CombatViewReceiptWireError {
    Length,
    Version,
    Flags,
}

/// Immutable provenance copied into every accepted authoritative projectile.
/// Collision resolves the server-owned schedule at the impact tick; none of
/// these fields lets the client select a target or position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectileCombatTimelineV1 {
    pub(crate) owner: PlayerRuntimeIdentity,
    pub(crate) owner_input_sequence: u16,
    pub(crate) accepted_timeline_generation: u32,
    pub(crate) accepted_game_applied_snapshot_id: u16,
    pub(crate) accepted_server_tick: u32,
    pub(crate) maximum_rewind_ticks: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectilePresentationQueryV1 {
    pub(crate) accepted_timeline_generation: u32,
    pub(crate) effective_timeline_generation: u32,
    pub(crate) render_tick: u32,
    pub(crate) applied_snapshot_id: u16,
    pub(crate) server_advanced_watermark: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectilePresentationQueryRejection {
    WrongRuntime,
    CollisionBeforeAcceptance,
    UnknownAcceptedGeneration,
    NoEffectiveSchedule,
    MaximumRewindExceeded,
    AcceptedDecisionEvicted,
    NoEmittedPresentationFrame,
}

impl ProjectilePresentationQueryRejection {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::WrongRuntime => "presentation_query_wrong_runtime",
            Self::CollisionBeforeAcceptance => "presentation_query_before_acceptance",
            Self::UnknownAcceptedGeneration => "presentation_query_unknown_accepted_generation",
            Self::NoEffectiveSchedule => "presentation_query_no_effective_schedule",
            Self::MaximumRewindExceeded => "presentation_query_maximum_rewind_exceeded",
            Self::AcceptedDecisionEvicted => "presentation_query_decision_evicted",
            Self::NoEmittedPresentationFrame => "presentation_query_no_emitted_frame",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectilePresentationStatusV1 {
    NotEvaluated,
    ServerNative,
    LocalSelf,
    Resolved(PresentationProjectionDisposition),
    QueryUnavailable(ProjectilePresentationQueryRejection),
    ProjectionUnavailable(PresentationProjectionUnavailable),
}

impl ProjectilePresentationStatusV1 {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NotEvaluated => "not_evaluated",
            Self::ServerNative => "server_native",
            Self::LocalSelf => "local_self",
            Self::Resolved(PresentationProjectionDisposition::Exact) => "resolved_exact",
            Self::Resolved(PresentationProjectionDisposition::Interpolated) => {
                "resolved_interpolated"
            }
            Self::Resolved(PresentationProjectionDisposition::HeldNoUpperSample) => "resolved_held",
            Self::Resolved(PresentationProjectionDisposition::SnappedDiscontinuity) => {
                "resolved_snapped"
            }
            Self::QueryUnavailable(reason) => reason.reason(),
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::NoAppliedSample) => {
                "projection_no_applied_sample"
            }
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::BeforeFirstSample) => {
                "projection_before_first_sample"
            }
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::WrongSegment) => {
                "projection_wrong_segment"
            }
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::AuthorityHistory(
                HistoryUnavailable::Empty,
            )) => "projection_authority_empty",
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::AuthorityHistory(
                HistoryUnavailable::WrongSegment,
            )) => "projection_authority_wrong_segment",
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::AuthorityHistory(
                HistoryUnavailable::BeforeRetainedWindow,
            )) => "projection_authority_before_retained_window",
            Self::ProjectionUnavailable(PresentationProjectionUnavailable::AuthorityHistory(
                HistoryUnavailable::NoSampleAtOrBefore,
            )) => "projection_authority_no_sample",
            Self::ProjectionUnavailable(
                PresentationProjectionUnavailable::AuthorityErrorExceeded,
            ) => "projection_authority_error_exceeded",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectilePresentationWitnessV1 {
    pub(crate) status: ProjectilePresentationStatusV1,
    pub(crate) accepted_timeline_generation: u32,
    pub(crate) effective_timeline_generation: u32,
    pub(crate) render_tick: u32,
    pub(crate) applied_snapshot_id: u16,
    pub(crate) lower_snapshot_id: u16,
    pub(crate) upper_snapshot_id: u16,
    pub(crate) target_x_q8: i32,
    pub(crate) target_y_q8: i32,
    pub(crate) authority_x_q8: i32,
    pub(crate) authority_y_q8: i32,
    pub(crate) authority_error_q8: i32,
    pub(crate) server_advanced_watermark: bool,
}

impl ProjectilePresentationWitnessV1 {
    pub(crate) const fn not_evaluated() -> Self {
        Self::empty(ProjectilePresentationStatusV1::NotEvaluated)
    }

    pub(crate) const fn server_native() -> Self {
        Self::empty(ProjectilePresentationStatusV1::ServerNative)
    }

    pub(crate) fn local_self(
        binding: ProjectileCombatTimelineV1,
        collision_tick: u32,
        target_x_q8: i32,
        target_y_q8: i32,
    ) -> Self {
        Self {
            status: ProjectilePresentationStatusV1::LocalSelf,
            accepted_timeline_generation: binding.accepted_timeline_generation,
            effective_timeline_generation: binding.accepted_timeline_generation,
            render_tick: collision_tick,
            target_x_q8,
            target_y_q8,
            authority_x_q8: target_x_q8,
            authority_y_q8: target_y_q8,
            ..Self::empty(ProjectilePresentationStatusV1::LocalSelf)
        }
    }

    pub(crate) fn resolved(
        query: ProjectilePresentationQueryV1,
        anchor: PresentationGeometryAnchorV1,
    ) -> Self {
        Self {
            status: ProjectilePresentationStatusV1::Resolved(anchor.disposition),
            accepted_timeline_generation: query.accepted_timeline_generation,
            effective_timeline_generation: query.effective_timeline_generation,
            render_tick: query.render_tick,
            applied_snapshot_id: query.applied_snapshot_id,
            lower_snapshot_id: anchor.lower_snapshot_id,
            upper_snapshot_id: anchor.upper_snapshot_id,
            target_x_q8: anchor.x_q8,
            target_y_q8: anchor.y_q8,
            authority_x_q8: anchor.authority_x_q8,
            authority_y_q8: anchor.authority_y_q8,
            authority_error_q8: anchor.authority_error_q8,
            server_advanced_watermark: query.server_advanced_watermark,
        }
    }

    pub(crate) fn query_unavailable(
        binding: ProjectileCombatTimelineV1,
        rejection: ProjectilePresentationQueryRejection,
    ) -> Self {
        Self {
            accepted_timeline_generation: binding.accepted_timeline_generation,
            status: ProjectilePresentationStatusV1::QueryUnavailable(rejection),
            ..Self::empty(ProjectilePresentationStatusV1::QueryUnavailable(rejection))
        }
    }

    pub(crate) fn projection_unavailable(
        query: ProjectilePresentationQueryV1,
        rejection: PresentationProjectionUnavailable,
    ) -> Self {
        Self {
            status: ProjectilePresentationStatusV1::ProjectionUnavailable(rejection),
            accepted_timeline_generation: query.accepted_timeline_generation,
            effective_timeline_generation: query.effective_timeline_generation,
            render_tick: query.render_tick,
            applied_snapshot_id: query.applied_snapshot_id,
            server_advanced_watermark: query.server_advanced_watermark,
            ..Self::empty(ProjectilePresentationStatusV1::ProjectionUnavailable(
                rejection,
            ))
        }
    }

    pub(crate) const fn target_anchor_q8(self) -> Option<(i32, i32)> {
        match self.status {
            ProjectilePresentationStatusV1::LocalSelf
            | ProjectilePresentationStatusV1::Resolved(_) => {
                Some((self.target_x_q8, self.target_y_q8))
            }
            _ => None,
        }
    }

    const fn empty(status: ProjectilePresentationStatusV1) -> Self {
        Self {
            status,
            accepted_timeline_generation: 0,
            effective_timeline_generation: 0,
            render_tick: 0,
            applied_snapshot_id: 0,
            lower_snapshot_id: 0,
            upper_snapshot_id: 0,
            target_x_q8: 0,
            target_y_q8: 0,
            authority_x_q8: 0,
            authority_y_q8: 0,
            authority_error_q8: 0,
            server_advanced_watermark: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectileTimelineBindingRejection {
    WrongRuntime,
    UnverifiedInput,
    HeldOrStalled,
    UnknownGeneration,
}

impl ProjectileTimelineBindingRejection {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::WrongRuntime => "projectile_timeline_wrong_runtime",
            Self::UnverifiedInput => "projectile_timeline_unverified_input",
            Self::HeldOrStalled => "projectile_timeline_held_or_stalled",
            Self::UnknownGeneration => "projectile_timeline_unknown_generation",
        }
    }
}

#[cfg(test)]
#[path = "combat_timeline_lookup_bench.rs"]
mod lookup_bench;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_bullet_broadphase_model_retains_authorized_rounding_boundaries() {
        // Independent geometric lemma, not production collision admission.
        // The resolver rounds distance in q8 units before enforcing its bound.
        // Outward integer boxes include that half-unit edge and floor/float
        // rounding. They may retain extra candidates but may never reject a hit.
        let candidate = |authority: (f64, f64), projectile: (f32, f32), radius: f32| {
            let extent = f64::from(radius.ceil())
                + ((f64::from(PRESENTATION_AUTHORITY_MAX_ERROR_Q8) + 0.5) / 256.0).ceil()
                + 2.0;
            (authority.0.floor() - f64::from(projectile.0).floor()).abs() <= extent
                && (authority.1.floor() - f64::from(projectile.1).floor()).abs() <= extent
        };
        let mut checked = 0;
        for origin in [0.0_f32, 4096.00390625, 16383.99609375, 65535.0] {
            for radius in [1.0_f32, 14.0, 31.5, 64.0] {
                for dx in -13_i32..=13 {
                    for dy in -13_i32..=13 {
                        for fraction in [0.0_f32, 1.0 / 256.0, 127.0 / 256.0] {
                            let anchor = (origin + dx as f32 + fraction, origin + dy as f32);
                            let ax = f64::from(anchor.0) - f64::from(origin);
                            let ay = f64::from(anchor.1) - f64::from(origin);
                            if ((ax * ax + ay * ay).sqrt() * 256.0).round()
                                > f64::from(PRESENTATION_AUTHORITY_MAX_ERROR_Q8)
                            {
                                continue;
                            }
                            for (px, py) in
                                [(radius, 0.0), (-radius, 0.0), (0.0, radius), (0.0, -radius)]
                            {
                                let projectile = (anchor.0 + px, anchor.1 + py);
                                let hit_dx = anchor.0 - projectile.0;
                                let hit_dy = anchor.1 - projectile.1;
                                if hit_dx * hit_dx + hit_dy * hit_dy <= radius * radius {
                                    assert!(candidate(
                                        (f64::from(origin), f64::from(origin)),
                                        projectile,
                                        radius
                                    ));
                                    checked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 10_000);
        assert!(!candidate((0.0, 0.0), (1000.0, 1000.0), 14.0));
    }

    #[test]
    fn source_time_crosses_the_actual_continuum_31_bit_rollover() {
        let before = PresentationSampleTime::from_parts(0x7fff_ffff, 2).unwrap();
        let after = PresentationSampleTime::from_parts(0, 1).unwrap();
        assert_eq!(after.quanta_since(before), Some(2));
        assert_eq!(before.quanta_since(after), Some(-2));
        let ambiguous = PresentationSampleTime::from_tick(1 << 30);
        assert_eq!(
            ambiguous.quanta_since(PresentationSampleTime::from_tick(0)),
            None
        );
        assert_eq!(PresentationSampleTime::from_parts(1 << 31, 0), None);
        let actor = runtime(7, 9, 3);
        let mut history = ActorMotionHistory::new();
        append_sample(&mut history, actor, 11, 0x7fff_ffff, 10.0);
        let baseline = *history.samples.back().unwrap();
        history.begin_accepted_samples(u16::MAX, baseline).unwrap();
        history
            .append_accepted_samples(&[(
                0,
                ActorMotionSampleV1 {
                    server_tick: 1,
                    x_q8: 12 * 256,
                    ..baseline
                },
            )])
            .unwrap();
        let due = history.publication_sample_at(1).unwrap();
        assert_eq!(due.sample_time.parts(), (0, 2));
        assert_eq!(due.x_q8, 12 * 256);
    }

    #[test]
    fn publication_selects_due_state_time_pairs_and_never_relabels_future_tails() {
        for origin in [100_u32, crate::continuum::CONTINUUM_TICK_MASK - 1] {
            let actor = runtime(7, 9, 3);
            let mut history = ActorMotionHistory::new();
            append_sample(&mut history, actor, 11, origin, 10.0);
            let baseline = *history.samples.back().unwrap();
            history.begin_accepted_samples(u16::MAX, baseline).unwrap();
            let first = ActorMotionSampleV1 {
                x_q8: 12 * 256,
                ..baseline
            };
            let second = ActorMotionSampleV1 {
                x_q8: 14 * 256,
                ..baseline
            };
            history
                .append_accepted_samples(&[(0, first), (1, second)])
                .unwrap();
            let before = history.clone();
            assert_eq!(history.publication_sample_at(origin).unwrap(), baseline);
            let due_first = history
                .publication_sample_at(origin.wrapping_add(2))
                .unwrap();
            assert_eq!(due_first.position(), (12.0, baseline.position().1));
            assert_eq!(
                due_first.sample_time.parts(),
                (crate::continuum::continuum_tick_add(origin, 1), 2)
            );
            let due_second = history
                .publication_sample_at(origin.wrapping_add(4))
                .unwrap();
            assert_eq!(due_second.position(), (14.0, baseline.position().1));
            assert_eq!(
                due_second.sample_time.parts(),
                (crate::continuum::continuum_tick_add(origin, 3), 1)
            );
            assert_eq!(
                history
                    .publication_sample_at(origin.wrapping_add(20))
                    .unwrap(),
                due_second
            );
            assert_eq!(
                history, before,
                "emission neither retimes nor mutates history"
            );
        }
    }

    #[test]
    fn accepted_batch_recovery_preserves_proven_samples_segment_and_next_step() {
        for (steps, recovers) in [(24_u16, false), (8, true)] {
            let actor = runtime(7, 9, 3);
            let mut history = ActorMotionHistory::new();
            append_sample(&mut history, actor, 11, 100, 10.0);
            let baseline = *history.samples.back().unwrap();
            history.begin_accepted_samples(100, baseline).unwrap();
            let batch: Vec<_> = (1..=steps)
                .map(|i| {
                    (
                        100 + i,
                        ActorMotionSampleV1 {
                            server_tick: 140,
                            x_q8: (10 + i32::from(i) * 2) * 256,
                            ..baseline
                        },
                    )
                })
                .collect();
            history.append_accepted_samples(&batch).unwrap();
            let samples: Vec<_> = history.current_samples().collect();
            assert_eq!(
                samples.len(),
                usize::from(steps) + 1 + usize::from(recovers)
            );
            for (index, sample) in samples.iter().take(usize::from(steps) + 1).enumerate() {
                assert_eq!(
                    sample.sample_time.quanta_since(baseline.sample_time),
                    Some(index as i64 * 5)
                );
                assert_eq!(sample.segment, baseline.segment);
            }
            let tail = history.publication_sample_at(140).unwrap();
            assert_eq!(tail.sample_time.parts(), (140, 0));
            assert_eq!(tail.x_q8, (10 + i32::from(steps) * 2) * 256);
            assert_eq!(tail.segment, baseline.segment);
            history
                .append_accepted_samples(&[(
                    101 + steps,
                    ActorMotionSampleV1 {
                        server_tick: 142,
                        x_q8: tail.x_q8 + 512,
                        ..tail
                    },
                )])
                .unwrap();
            assert_eq!(
                history
                    .publication_sample_at(142)
                    .unwrap()
                    .sample_time
                    .parts(),
                (141, 2)
            );
        }
    }

    #[test]
    fn clocked_history_commits_replay_batches_atomically_and_resets_on_physical_boundary() {
        let actor = runtime(7, 9, 3);
        let mut history = ActorMotionHistory::new();
        append_sample(&mut history, actor, 11, 100, 10.0);
        let baseline = *history.samples.back().unwrap();
        history.begin_accepted_samples(u16::MAX, baseline).unwrap();
        let step = |x| ActorMotionSampleV1 {
            server_tick: 105,
            x_q8: x * 256,
            ..baseline
        };
        let before = history.clone();
        assert_eq!(
            history.append_accepted_samples(&[(0, step(12)), (2, step(14))]),
            Err(ClockedHistoryError::MissingOrStaleStep)
        );
        assert_eq!(
            history, before,
            "bad second step must not commit first step or clock"
        );
        assert_eq!(
            history.append_accepted_samples(&[]),
            Err(ClockedHistoryError::BatchLength)
        );
        assert_eq!(
            history.append_accepted_samples(&vec![
                (0, step(12));
                ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY + 1
            ]),
            Err(ClockedHistoryError::BatchLength)
        );
        assert_eq!(history, before);
        history
            .append_accepted_samples(&[(0, step(12)), (1, step(14))])
            .unwrap();
        let samples: Vec<_> = history.current_samples().collect();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[1].sample_time.parts(), (101, 2));
        assert_eq!(samples[2].sample_time.parts(), (103, 1));
        assert_eq!(samples[1].server_tick, 105);
        assert_eq!(samples[2].server_tick, 105);
        let before = history.clone();
        assert_eq!(
            history.append(step(16)),
            HistoryAppendDisposition::RejectedClockedAppendRequired
        );
        assert_eq!(
            history, before,
            "unsequenced append cannot bypass clock ownership"
        );
        assert_eq!(
            history.append_accepted_samples(&[(
                2,
                ActorMotionSampleV1 {
                    source_kind: ActorMotionSourceKind::ServerBotSimulation,
                    ..step(16)
                }
            )]),
            Err(ClockedHistoryError::WrongSourceKind)
        );
        assert_eq!(history, before);
        assert_eq!(
            history.append_accepted_samples(&[(
                2,
                ActorMotionSampleV1 {
                    server_tick: 104,
                    ..step(16)
                }
            )]),
            Err(ClockedHistoryError::HistoryRejected(
                HistoryAppendDisposition::RejectedStaleTick
            ))
        );
        assert_eq!(history, before);
        history.begin_segment(actor, 11);
        assert_eq!(
            history.append_accepted_samples(&[(2, step(16))]),
            Err(ClockedHistoryError::MissingClock)
        );
    }

    #[test]
    fn actor_history_retains_fractional_state_time_and_rejects_retiming_backwards() {
        let actor = runtime(7, 9, 3);
        let mut history = ActorMotionHistory::new();
        append_sample(&mut history, actor, 11, 100, 10.0);
        let first = *history.samples.back().unwrap();
        let next = ActorMotionSampleV1 {
            server_tick: 102,
            sample_time: PresentationSampleTime::from_parts(101, 2).unwrap(),
            x_q8: 12 * 256,
            ..first
        };
        assert_eq!(history.append(next), HistoryAppendDisposition::Applied);
        assert_eq!(history.at_or_before(next.segment, 102), Ok(next));
        let before = history.clone();
        assert_eq!(
            history.append(ActorMotionSampleV1 {
                server_tick: 103,
                sample_time: PresentationSampleTime::from_tick(101),
                ..next
            }),
            HistoryAppendDisposition::RejectedStaleSampleTime
        );
        assert_eq!(history, before);
        assert_eq!(
            history.samples.front(),
            Some(&first),
            "later samples cannot retimestamp retained state"
        );
    }

    #[test]
    fn sample_clock_timing_recovery_preserves_exact_actor_segment_and_tail() {
        let actor = runtime(7, 9, 3);
        let segment = MotionSegmentIdentity {
            actor,
            arena_stream_id: 11,
            discontinuity_generation: 1,
        };
        let mut clock = AcceptedMotionSampleClock::at_authorized_boundary(segment, 108, 113);
        let previous_time = clock.time;
        let before = clock;
        let other = MotionSegmentIdentity {
            discontinuity_generation: 2,
            ..segment
        };
        assert_eq!(
            clock.reanchor_accepted_tail(other, 108, 140),
            Err(SampleClockReanchorError::WrongSegment)
        );
        assert_eq!(
            clock.reanchor_accepted_tail(segment, 109, 140),
            Err(SampleClockReanchorError::WrongSequence)
        );
        assert_eq!(
            clock.reanchor_accepted_tail(segment, 108, 112),
            Err(SampleClockReanchorError::BackwardOrAmbiguousTime)
        );
        assert_eq!(
            clock.reanchor_accepted_tail(segment, 108, 113 + (1 << 30)),
            Err(SampleClockReanchorError::BackwardOrAmbiguousTime)
        );
        assert_eq!(clock, before);
        let resumed = PresentationSampleTime::from_parts(140, 0).unwrap();
        assert_eq!(clock.reanchor_accepted_tail(segment, 108, 140), Ok(resumed));
        assert_eq!(clock.segment, before.segment);
        assert_eq!(clock.sequence, before.sequence);
        assert_eq!(
            previous_time,
            PresentationSampleTime::from_parts(113, 0).unwrap()
        );
        assert_eq!(
            clock.reanchor_accepted_tail(segment, 108, 140),
            Ok(resumed),
            "idempotent exact retry"
        );
        assert_eq!(
            clock.append_proven_step(segment, 109),
            SampleClockAdmission::Recorded(PresentationSampleTime::from_parts(141, 2).unwrap())
        );
    }

    #[test]
    fn fractional_sample_geometry_preserves_sixty_hz_duration_and_wrap() {
        let sample = |x| PresentationKinematicSampleV1 {
            x_q8: x * 256,
            y_q8: 0,
            velocity_x_q8: 120 * 256,
            velocity_y_q8: 0,
        };
        let time = |tick, third| PresentationSampleTime::from_parts(tick, third).unwrap();
        for start in [0, 100, crate::continuum::CONTINUUM_TICK_MASK] {
            let lower = time(start, 0);
            let upper = time(crate::continuum::continuum_tick_add(start, 1), 2);
            // 120 px/s for 1/60 s advances exactly 2 px. At 1/100 s,
            // independently computed constant-speed geometry is 1.2 px.
            assert_eq!(
                presentation_hermite_sample_time_q8(
                    lower,
                    sample(0),
                    upper,
                    sample(2),
                    time(crate::continuum::continuum_tick_add(start, 1), 0)
                ),
                Some((307, 0))
            );
            assert_eq!(
                presentation_hermite_sample_time_q8(
                    lower,
                    sample(0),
                    upper,
                    sample(2),
                    time(crate::continuum::continuum_tick_add(start, 3), 0)
                ),
                Some((512, 0)),
                "never extrapolate"
            );
            assert_eq!(
                presentation_hermite_sample_time_q8(upper, sample(2), lower, sample(0), lower),
                None
            );
            assert_eq!(
                presentation_hermite_sample_time_q8(lower, sample(0), lower, sample(0), lower),
                None
            );
            assert_eq!(
                presentation_hermite_sample_time_q8(
                    lower,
                    sample(0),
                    time(crate::continuum::continuum_tick_add(start, 1 << 30), 0),
                    sample(2),
                    lower
                ),
                None
            );
        }
        assert_eq!(PresentationSampleTime::from_parts(0, 3), None);
    }

    #[test]
    fn fractional_sample_geometry_matches_existing_integral_tick_projection() {
        let time = |tick| PresentationSampleTime::from_parts(tick, 0).unwrap();
        for span in 1..=28 {
            for velocity in [-300, -63, 0, 120, 300] {
                let lower = PresentationKinematicSampleV1 {
                    x_q8: 8_000 * 256,
                    y_q8: 8_100 * 256,
                    velocity_x_q8: velocity * 256,
                    velocity_y_q8: -velocity * 256,
                };
                let upper = PresentationKinematicSampleV1 {
                    x_q8: lower.x_q8 + 7 * 256,
                    y_q8: lower.y_q8 - 3 * 256,
                    velocity_x_q8: -lower.velocity_x_q8,
                    ..lower
                };
                for offset in 0..=span + 1 {
                    let start = crate::continuum::CONTINUUM_TICK_MASK - 10;
                    let end = crate::continuum::continuum_tick_add(start, span);
                    let render = crate::continuum::continuum_tick_add(start, offset);
                    assert_eq!(
                        presentation_hermite_sample_time_q8(
                            time(start),
                            lower,
                            time(end),
                            upper,
                            time(render)
                        ),
                        Some(presentation_hermite_q8(start, lower, end, upper, render))
                    );
                }
            }
        }
    }

    #[test]
    fn retained_human_batch_cadence_projects_exact_emitted_midpoints() {
        // core-combat/failure-1 in final-verified-ziv8oh.
        // These are explicit wire coordinates, not reconstructed delta bases.
        // Sender input 18685 -> 18687 advanced two 60 Hz steps in one report;
        // snapshots 1850 -> 1851 published that tail at ticks 3670 -> 3671.
        // Pin what the shared projection actually does, NOT a smoothness pass:
        // the browser's >6 px motion failure remains open. A client-only
        // easing correction here would also change the combat-view geometry.
        let sample = |x, y| PresentationKinematicSampleV1 {
            x_q8: x * 256,
            y_q8: y * 256,
            velocity_x_q8: (-63.6_f32 * 256.0).round() as i32,
            velocity_y_q8: (139.2_f32 * 256.0).round() as i32,
        };
        let before =
            presentation_hermite_q8(3668, sample(8155, 8207), 3670, sample(8154, 8209), 3669);
        let after =
            presentation_hermite_q8(3671, sample(8151, 8214), 3673, sample(8150, 8216), 3672);
        // Equal endpoint velocities cancel at t=1/2, independently of their
        // magnitude. Both results match the actual retained sprite draws.
        assert_eq!(before, (8154 * 256 + 128, 8208 * 256));
        assert_eq!(after, (8150 * 256 + 128, 8215 * 256));
        // A repeated published position must hold, not invent displacement
        // from a nonzero velocity while waiting for another accepted tail.
        let held = sample(8155, 8207);
        assert_eq!(
            presentation_hermite_q8(3666, held, 3668, held, 3667),
            (held.x_q8, held.y_q8),
        );
    }

    #[test]
    fn motion_history_heading_uses_turns_not_radians() {
        for (turns, packed) in [
            (0.0, 0),
            (0.25, 16_384),
            (0.5, 32_768),
            (0.75, 49_152),
            (1.0, 0),
            (-0.25, 49_152),
        ] {
            assert_eq!(orientation_u16(turns), packed, "heading {turns} turns");
        }
    }

    #[test]
    fn motion_history_preserves_source_wire_heading_before_angle_rounding() {
        let mut history = ActorMotionHistory::new();
        let segment = history.begin_segment(runtime(768, 1, 1), 1);
        for direction in 0..40_u8 {
            let turns = f32::from(direction) / 40.0;
            // Include both interiors and values close to bin boundaries.
            for offset in [0.0, 0.000_001, 0.0125, 0.024_999] {
                let angle = turns + offset;
                let sample = ActorMotionSampleV1::from_state(
                    segment,
                    1,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    angle,
                    14.0,
                    0,
                    ActorMotionSourceKind::ServerBotSimulation,
                );
                assert_eq!(sample.protocol_direction, direction, "angle {angle}");
            }
        }
    }

    fn runtime(player_id: u16, session_id: u64, life_seq: u32) -> PlayerRuntimeIdentity {
        PlayerRuntimeIdentity::new(
            crate::authority_identity::PlayerSessionIdentity::new(
                player_id,
                crate::authority_identity::PlayerSessionId::new(session_id),
            ),
            crate::authority_identity::LifeIdentity::new(player_id, life_seq),
        )
    }

    fn append_sample(
        history: &mut ActorMotionHistory,
        actor: PlayerRuntimeIdentity,
        arena_stream_id: u64,
        server_tick: u32,
        x: f32,
    ) -> HistoryAppendDisposition {
        history.append_state(
            actor,
            arena_stream_id,
            server_tick,
            x,
            200.0,
            3.0,
            -4.0,
            std::f32::consts::FRAC_PI_2,
            14.0,
            0,
            ActorMotionSourceKind::AcceptedHumanWitness,
        )
    }

    fn presentation_sample(
        actor: PlayerRuntimeIdentity,
        motion_segment: MotionSegmentIdentity,
        x: i32,
        velocity_x: i32,
        snap_discontinuity: bool,
    ) -> ObserverPresentationSampleV1 {
        ObserverPresentationSampleV1 {
            sample_time: PresentationSampleTime::default(),
            actor,
            motion_segment,
            x_q8: x * 256,
            y_q8: 200 * 256,
            velocity_x_q8: velocity_x * 256,
            velocity_y_q8: 0,
            orientation_u16: 0,
            snap_discontinuity,
        }
    }

    fn presentation_frame(
        observer: PlayerRuntimeIdentity,
        arena_stream_id: u64,
        snapshot_id: u16,
        server_tick: u32,
        mut actors: Vec<ObserverPresentationSampleV1>,
    ) -> ObserverPresentationFrameV1 {
        for actor in &mut actors {
            actor.sample_time = PresentationSampleTime::from_tick(server_tick);
        }
        ObserverPresentationFrameV1 {
            observer,
            arena_stream_id,
            snapshot_id,
            server_tick,
            actors,
        }
    }

    fn append_motion_at(
        history: &mut ActorMotionHistory,
        actor: PlayerRuntimeIdentity,
        arena_stream_id: u64,
        server_tick: u32,
        x: i32,
        velocity_x: i32,
    ) -> MotionSegmentIdentity {
        assert_eq!(
            history.append_state(
                actor,
                arena_stream_id,
                server_tick,
                x as f32,
                200.0,
                velocity_x as f32,
                0.0,
                0.0,
                14.0,
                0,
                ActorMotionSourceKind::AcceptedHumanWitness,
            ),
            HistoryAppendDisposition::Applied
        );
        history.current_segment().expect("motion segment")
    }

    fn directive(profile: ObserverNetworkProfile) -> ObserverTimelineDirectiveV1 {
        let envelope = profile.delay_envelope();
        ObserverTimelineDirectiveV1 {
            generation: u32::MAX,
            issued_server_tick: u32::MAX - 20,
            effective_server_tick: 4,
            effective_from_input_sequence: u16::MAX,
            from_delay_ticks: envelope.target_ticks,
            target_delay_ticks: envelope.target_ticks,
            ceiling_delay_ticks: envelope.ceiling_ticks,
            slew_interval_ticks: OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
            hold_grace_ticks: OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
            profile,
        }
    }

    // These schedule/old-flight fixtures begin AFTER a fast path has settled,
    // rather than replaying the unrelated initial nonce-measurement phase.
    // The separate cold-first-shot regression exercises the production default.
    fn settled_fast_auto_fixture(
        timeline: &mut ObserverCombatTimeline,
        mut initial: ObserverTimelineDirectiveV1,
    ) -> ObserverTimelineDirectiveV1 {
        assert_eq!(initial.profile, ObserverNetworkProfile::AutomaticWan);
        assert_eq!(
            (initial.target_delay_ticks, initial.ceiling_delay_ticks),
            (20, 20)
        );
        assert!(timeline.observed_receipts.is_empty());
        assert!(timeline.last_input_sequence.is_none());
        assert_eq!(timeline.current_directive(), Some(initial));
        initial.from_delay_ticks = 7;
        initial.target_delay_ticks = 7;
        initial.validate().unwrap();
        timeline.directives.front_mut().unwrap().directive = initial;
        initial
    }

    #[test]
    fn v1_directive_shape_round_trips_every_profile_and_rejects_extension_bits() {
        for profile in [
            ObserverNetworkProfile::CleanLan,
            ObserverNetworkProfile::SupportedMildWan,
            ObserverNetworkProfile::SupportedSlowWan,
            ObserverNetworkProfile::RecoveryStress,
            ObserverNetworkProfile::AutomaticWan,
        ] {
            let value = directive(profile);
            let bytes = value.encode();
            assert_eq!(bytes.len(), ObserverTimelineDirectiveV1::ENCODED_LEN);
            assert_eq!(ObserverTimelineDirectiveV1::decode(&bytes), Ok(value));

            let mut extended = bytes;
            extended[22] = 1;
            assert_eq!(
                ObserverTimelineDirectiveV1::decode(&extended),
                Err(ObserverTimelineWireError::ReservedBits)
            );
        }
    }

    #[test]
    fn automatic_wan_initial_view_admits_first_shot_on_supported_asymmetric_paths() {
        for (s2c_ticks, c2s_ticks) in [(6, 4), (12, 7), (18, 2)] {
            let observer = runtime(3, 99, 4);
            let mut server = ObserverCombatTimeline::default();
            let offer = server
                .begin_scope(
                    observer,
                    77,
                    ObserverNetworkProfile::AutomaticWan,
                    1,
                    900,
                    10,
                )
                .unwrap();
            let mut client = ClientObserverCombatTimeline::default();
            client.observe_directive(offer, 10);
            let received_snapshot_tick = 1000 - s2c_ticks;
            let mut emitted = ObserverPresentationHistory::new();
            emitted.begin_scope(observer, 77);
            assert_eq!(
                emitted.observe_emitted(presentation_frame(
                    observer,
                    77,
                    1,
                    received_snapshot_tick,
                    vec![]
                )),
                ObserverPresentationAppendDisposition::Applied
            );
            // This newer frame has been sent by receipt arrival, but was not
            // yet available at the client's genuine initial weapon edge.
            assert_eq!(
                emitted.observe_emitted(presentation_frame(
                    observer,
                    77,
                    2,
                    1000 + c2s_ticks - 1,
                    vec![]
                )),
                ObserverPresentationAppendDisposition::Applied
            );
            client.observe_game_applied_snapshot(1, received_snapshot_tick);
            client.advance_presentation_render_tick(1000, 1.0 / 60.0);
            let receipt = client.receipt(Some(4), 10, Some(1000));
            assert!(
                !receipt.hold_or_stall(),
                "an unknown initial path of {s2c_ticks}/{c2s_ticks} ticks must not draw ahead of its available snapshot"
            );
            let decision = server
                .verify_receipt(observer, 1000 + c2s_ticks, receipt, &emitted)
                .expect("a supported initial WAN view retains an exact server-verifiable receipt");
            assert!(decision.weapon_edge_allowed());
            let binding = server
                .bind_projectile(observer, 10, 1000 + c2s_ticks)
                .expect("the first legitimate shot must not wait for later probe adaptation");
            assert_eq!(binding.maximum_rewind_ticks, 20);
            assert_eq!(binding.owner, observer);
        }
    }

    #[test]
    fn v1_receipt_is_compact_authenticated_scope_free_and_fail_closed() {
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: u32::MAX,
            input_sequence: u16::MAX,
            timeline_generation: u32::MAX - 1,
            latest_game_applied_snapshot_id: u16::MAX - 1,
            flags: COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL,
        };
        let bytes = receipt.encode();
        assert_eq!(bytes.len(), CombatViewReceiptV1::ENCODED_LEN);
        assert_eq!(CombatViewReceiptV1::decode(&bytes), Ok(receipt));
        assert!(receipt.hold_or_stall());

        let mut forged = bytes;
        forged[1] |= 0x80;
        assert_eq!(
            CombatViewReceiptV1::decode(&forged),
            Err(CombatViewReceiptWireError::Flags)
        );
    }

    #[test]
    fn local_weapon_submission_requires_complete_unheld_receipt_identity() {
        let ready = CombatViewReceiptV1 {
            observer_life_seq: 3,
            input_sequence: 7,
            timeline_generation: 11,
            latest_game_applied_snapshot_id: 13,
            flags: 0,
        };
        assert!(ready.weapon_submission_ready());
        assert!(!CombatViewReceiptV1::unavailable(7).weapon_submission_ready());
        assert!(
            !CombatViewReceiptV1 {
                flags: COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL,
                ..ready
            }
            .weapon_submission_ready()
        );
    }

    #[test]
    fn production_observer_timeline_is_hard_scope_owned_and_soft_life_stable() {
        let first_life = runtime(3, 99, 1);
        let next_life = runtime(3, 99, 2);
        let replacement_session = runtime(3, 100, 1);
        let mut timeline = ObserverCombatTimeline::default();
        assert_eq!(
            timeline.begin_scope(
                first_life,
                0,
                ObserverNetworkProfile::CleanLan,
                1,
                100,
                u16::MAX,
            ),
            Err(ObserverTimelineScopeError::ZeroArenaStream)
        );
        let directive = timeline
            .begin_scope(
                first_life,
                77,
                ObserverNetworkProfile::SupportedMildWan,
                u32::MAX,
                u32::MAX - 5,
                u16::MAX,
            )
            .expect("valid hard scope");
        assert_eq!(timeline.current_directive(), Some(directive));
        assert_eq!(timeline.observer(), Some(first_life));
        assert_eq!(timeline.arena_stream_id(), Some(77));
        assert_eq!(timeline.profile(), ObserverNetworkProfile::SupportedMildWan);

        assert_eq!(timeline.advance_life(next_life), Ok(()));
        assert_eq!(timeline.observer(), Some(next_life));
        assert_eq!(
            timeline.current_directive(),
            Some(directive),
            "soft life change preserves the observer-wide schedule"
        );
        assert_eq!(
            timeline.advance_life(replacement_session),
            Err(ObserverTimelineScopeError::WrongSession)
        );
        assert_eq!(timeline.observer(), Some(next_life));
    }

    #[test]
    fn automatic_wan_schedule_preserves_old_flight_after_long_idle_and_matches_client_slew() {
        let observer = runtime(3, 99, 4);
        let mut timeline = ObserverCombatTimeline::default();
        let initial = timeline
            .begin_scope(
                observer,
                77,
                ObserverNetworkProfile::AutomaticWan,
                7,
                1_000,
                10,
            )
            .unwrap();
        let initial = settled_fast_auto_fixture(&mut timeline, initial);
        assert_eq!(
            (initial.target_delay_ticks, initial.ceiling_delay_ticks),
            (7, 20)
        );
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 77);
        presentation.observe_emitted(presentation_frame(observer, 77, 40, 199_995, Vec::new()));
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: 4,
            input_sequence: 10,
            timeline_generation: 7,
            latest_game_applied_snapshot_id: 40,
            flags: 0,
        };
        timeline
            .verify_receipt(observer, 199_999, receipt, &presentation)
            .unwrap();
        let binding = timeline.bind_projectile(observer, 10, 199_999).unwrap();
        let offered = ObserverTimelineDirectiveV1 {
            generation: 8,
            issued_server_tick: 200_000,
            effective_server_tick: 200_100,
            effective_from_input_sequence: 70,
            from_delay_ticks: 7,
            target_delay_ticks: 15,
            ..initial
        };
        timeline.issue_delay_schedule(offered).unwrap();
        assert_eq!(
            timeline.directives.len(),
            2,
            "an hours-old generation can have a freshly fired bomb"
        );
        presentation.observe_emitted(presentation_frame(observer, 77, 41, 200_175, Vec::new()));
        let query = timeline
            .projectile_presentation_query(binding, 200_180, &presentation)
            .unwrap();
        assert_eq!(
            (
                query.accepted_timeline_generation,
                query.effective_timeline_generation
            ),
            (7, 8)
        );
        assert_eq!(query.render_tick, 200_165);
        assert_eq!(binding.maximum_rewind_ticks, 20);
        assert_eq!(
            timeline.projectile_presentation_query(
                ProjectileCombatTimelineV1 {
                    maximum_rewind_ticks: 9,
                    ..binding
                },
                200_180,
                &presentation
            ),
            Err(ProjectilePresentationQueryRejection::MaximumRewindExceeded)
        );

        let mut client = ClientObserverCombatTimeline::default();
        client.observe_directive(initial, 10);
        assert_eq!(
            client.observe_directive(offered, 20),
            ClientObserverTimelineDisposition::Pending
        );
        assert_eq!(
            client.advance_input(69),
            ClientObserverTimelineDisposition::Pending
        );
        assert_eq!(
            client.advance_input(70),
            ClientObserverTimelineDisposition::Applied
        );
        for tick in 200_099..=200_190 {
            assert_eq!(client.delay_ticks_at(tick), timeline.delay_ticks_at(tick));
        }
        let stale = CombatViewReceiptV1 {
            input_sequence: 70,
            ..receipt
        };
        assert_eq!(
            timeline.verify_receipt(observer, 200_180, stale, &presentation),
            Err(ObserverTimelineReceiptRejection::SupersededGeneration)
        );
        assert_eq!(
            timeline.verify_receipt(
                observer,
                200_180,
                CombatViewReceiptV1 {
                    timeline_generation: 8,
                    latest_game_applied_snapshot_id: 41,
                    ..stale
                },
                &presentation
            ),
            Err(ObserverTimelineReceiptRejection::DuplicateMismatch),
            "a rejected input cannot gain weapon authority after a later directive arrives"
        );
        let next = CombatViewReceiptV1 {
            input_sequence: 71,
            timeline_generation: 8,
            latest_game_applied_snapshot_id: 41,
            ..receipt
        };
        assert!(
            timeline
                .verify_receipt(observer, 200_180, next, &presentation)
                .is_ok()
        );
    }

    #[test]
    fn combat_receipt_schedule_survives_multiple_complete_input_wraps() {
        let observer = runtime(3, 99, 4);
        let mut timeline = ObserverCombatTimeline::default();
        timeline
            .begin_scope(observer, 77, ObserverNetworkProfile::CleanLan, 7, 1_000, 10)
            .unwrap();
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 77);
        for step in 0_u32..140_000 {
            let sequence = 10_u16.wrapping_add(step as u16);
            let tick = 1_000 + step * 2;
            let snapshot_id = ((step % 65_535) + 1) as u16;
            presentation.observe_emitted(presentation_frame(
                observer,
                77,
                snapshot_id,
                tick,
                Vec::new(),
            ));
            let receipt = CombatViewReceiptV1 {
                observer_life_seq: 4,
                input_sequence: sequence,
                timeline_generation: 7,
                latest_game_applied_snapshot_id: snapshot_id,
                flags: 0,
            };
            assert!(
                timeline
                    .verify_receipt(observer, tick, receipt, &presentation)
                    .is_ok(),
                "valid sequential input at step {step}, raw sequence {sequence}, must keep its original schedule"
            );
        }
        let cursor = timeline.input_progress;
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: 4,
            input_sequence: 10_u16.wrapping_add(139_899_u32 as u16),
            timeline_generation: 7,
            latest_game_applied_snapshot_id: 1,
            flags: 0,
        };
        assert_eq!(
            timeline.verify_receipt(observer, 281_000, receipt, &presentation),
            Err(ObserverTimelineReceiptRejection::StaleInput)
        );
        assert_eq!(
            timeline.input_progress, cursor,
            "an old raw sequence cannot move the era cursor"
        );
    }

    #[test]
    fn automatic_schedule_can_change_after_a_complete_input_wrap() {
        let observer = runtime(3, 99, 4);
        let mut timeline = ObserverCombatTimeline::default();
        let initial = timeline
            .begin_scope(
                observer,
                77,
                ObserverNetworkProfile::AutomaticWan,
                7,
                1_000,
                10,
            )
            .unwrap();
        let initial = settled_fast_auto_fixture(&mut timeline, initial);
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 77);
        for step in 0_u32..70_000 {
            let sequence = 10_u16.wrapping_add(step as u16);
            let tick = 1_000 + step * 2;
            let snapshot_id = ((step % 65_535) + 1) as u16;
            presentation.observe_emitted(presentation_frame(
                observer,
                77,
                snapshot_id,
                tick,
                Vec::new(),
            ));
            timeline
                .verify_receipt(
                    observer,
                    tick,
                    CombatViewReceiptV1 {
                        observer_life_seq: 4,
                        input_sequence: sequence,
                        timeline_generation: 7,
                        latest_game_applied_snapshot_id: snapshot_id,
                        flags: 0,
                    },
                    &presentation,
                )
                .unwrap();
        }
        let next_sequence = 10_u16.wrapping_add(70_000_u32 as u16);
        let directive = ObserverTimelineDirectiveV1 {
            generation: 8,
            issued_server_tick: 141_000,
            effective_server_tick: 141_050,
            effective_from_input_sequence: next_sequence,
            from_delay_ticks: 7,
            target_delay_ticks: 15,
            ..initial
        };
        timeline.issue_delay_schedule(directive).unwrap();
        presentation.observe_emitted(presentation_frame(observer, 77, 5_000, 141_055, Vec::new()));
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: 4,
            input_sequence: next_sequence,
            timeline_generation: 8,
            latest_game_applied_snapshot_id: 5_000,
            flags: 0,
        };
        assert_eq!(
            timeline
                .verify_receipt(observer, 141_055, receipt, &presentation)
                .unwrap()
                .generation,
            8
        );
        let next_life = runtime(3, 99, 5);
        timeline.advance_life(next_life).unwrap();
        presentation.advance_life(next_life, 77);
        presentation.observe_emitted(presentation_frame(
            next_life,
            77,
            5_001,
            141_057,
            Vec::new(),
        ));
        assert!(
            timeline
                .verify_receipt(
                    next_life,
                    141_057,
                    CombatViewReceiptV1 {
                        observer_life_seq: 5,
                        input_sequence: next_sequence.wrapping_add(1),
                        latest_game_applied_snapshot_id: 5_001,
                        ..receipt
                    },
                    &presentation
                )
                .is_ok()
        );
    }

    #[test]
    fn server_schedule_rejections_are_atomic_and_cannot_change_flight_ceiling() {
        use ObserverScheduleRejection::*;
        let mut timeline = ObserverCombatTimeline::default();
        let initial = timeline
            .begin_scope(
                runtime(3, 99, 4),
                77,
                ObserverNetworkProfile::AutomaticWan,
                7,
                1_000,
                10,
            )
            .unwrap();
        let initial = settled_fast_auto_fixture(&mut timeline, initial);
        let valid = ObserverTimelineDirectiveV1 {
            generation: 8,
            issued_server_tick: 1_100,
            effective_server_tick: 1_200,
            effective_from_input_sequence: 70,
            target_delay_ticks: 15,
            ..initial
        };
        for (proposed, rejection) in [
            (
                ObserverTimelineDirectiveV1 {
                    generation: 0,
                    ..valid
                },
                InvalidWire,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    generation: 7,
                    ..valid
                },
                GenerationNotNewer,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    issued_server_tick: 1_099,
                    ..valid
                },
                GenerationTooSoon,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    effective_server_tick: 1_099,
                    ..valid
                },
                EffectiveTime,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    effective_server_tick: 1_301,
                    ..valid
                },
                EffectiveTime,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    effective_from_input_sequence: 10,
                    ..valid
                },
                EffectiveSequence,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    from_delay_ticks: 8,
                    ..valid
                },
                FromDelayMismatch,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    target_delay_ticks: 21,
                    ..valid
                },
                InvalidWire,
            ),
            (
                ObserverTimelineDirectiveV1 {
                    profile: ObserverNetworkProfile::CleanLan,
                    ceiling_delay_ticks: 9,
                    target_delay_ticks: 9,
                    ..valid
                },
                ProfileChanged,
            ),
        ] {
            let before = timeline.clone();
            assert_eq!(timeline.issue_delay_schedule(proposed), Err(rejection));
            assert_eq!(timeline, before);
        }
        timeline.issue_delay_schedule(valid).unwrap();
        assert_eq!(timeline.current_directive(), Some(valid));
    }

    #[test]
    fn delayed_preoffer_view_stays_unavailable_without_becoming_weapon_authority() {
        // A server offer cannot retroactively change the view captured by an
        // honest browser that has not received it. Reproduce the observed
        // 760 ms delivery/pause window using the real client receipt builder,
        // including an input-sequence wrap. No client timestamp is trusted.
        for (base, capture_tick) in [
            (21_736_u16, 861),
            (21_736, 936),
            (u16::MAX, 861),
            (u16::MAX, 936),
        ] {
            let observer = runtime(3, 99, 4);
            let mut server = ObserverCombatTimeline::default();
            let initial = server
                .begin_scope(
                    observer,
                    77,
                    ObserverNetworkProfile::AutomaticWan,
                    2,
                    700,
                    base,
                )
                .unwrap();
            let mut emitted = ObserverPresentationHistory::new();
            emitted.begin_scope(observer, 77);
            emitted.observe_emitted(presentation_frame(observer, 77, 95, 859, vec![]));
            let mut client = ClientObserverCombatTimeline::default();
            client.observe_directive(initial, base);
            client.observe_game_applied_snapshot(95, 859);
            client.advance_presentation_render_tick(861, 1.0 / 60.0);
            let sequence = base.wrapping_add(1);
            let old = client.receipt(Some(4), sequence, Some(capture_tick));
            assert_eq!(old.weapon_submission_ready(), capture_tick == 861);
            assert_eq!(old.timeline_generation, 2);
            server
                .issue_delay_schedule(ObserverTimelineDirectiveV1 {
                    generation: 4,
                    issued_server_tick: 860,
                    effective_server_tick: 910,
                    effective_from_input_sequence: sequence,
                    target_delay_ticks: 7,
                    ..initial
                })
                .unwrap();

            let mut prompt = server.clone();
            assert!(prompt.verify_receipt(observer, 861, old, &emitted).is_ok());
            let rejection = server
                .verify_receipt(observer, 936, old, &emitted)
                .expect_err("a delayed old clock must never authorize a weapon");
            assert!(server.decision_for_sequence(sequence).is_none());
            assert_eq!(
                server.verify_receipt(observer, 940, old, &emitted),
                Err(rejection),
                "a rolling replay preserves the first rejected decision"
            );
            assert_eq!(
                server.verify_receipt(
                    observer,
                    940,
                    CombatViewReceiptV1 {
                        timeline_generation: 4,
                        ..old
                    },
                    &emitted,
                ),
                Err(ObserverTimelineReceiptRejection::DuplicateMismatch),
                "learning the new offer cannot upgrade an old input"
            );
            assert!(
                !rejection.is_security_violation(),
                "a genuine pre-offer view is unavailable after delivery stalls: {rejection:?}"
            );
        }
    }

    #[test]
    fn superseded_auto_view_never_accepts_unknown_or_premature_generation() {
        for base in [10_u16, u16::MAX - 1] {
            for generation in [2_u32, u32::MAX] {
                for flags in [0, COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL] {
                    let observer = runtime(3, 99, 4);
                    let mut server = ObserverCombatTimeline::default();
                    let initial = server
                        .begin_scope(
                            observer,
                            77,
                            ObserverNetworkProfile::AutomaticWan,
                            generation,
                            700,
                            base,
                        )
                        .unwrap();
                    let offered_generation = generation.wrapping_add(1).max(1);
                    server
                        .issue_delay_schedule(ObserverTimelineDirectiveV1 {
                            generation: offered_generation,
                            issued_server_tick: 860,
                            effective_server_tick: 910,
                            effective_from_input_sequence: base.wrapping_add(2),
                            target_delay_ticks: 7,
                            ..initial
                        })
                        .unwrap();
                    let mut emitted = ObserverPresentationHistory::new();
                    emitted.begin_scope(observer, 77);
                    emitted.observe_emitted(presentation_frame(observer, 77, 96, 935, vec![]));
                    for (sequence, claimed_generation, expected) in [
                        (
                            base.wrapping_add(1),
                            offered_generation,
                            ObserverTimelineReceiptRejection::GenerationNotEffective,
                        ),
                        (
                            base.wrapping_add(2),
                            999,
                            ObserverTimelineReceiptRejection::UnknownGeneration,
                        ),
                        (
                            base.wrapping_add(2),
                            generation,
                            ObserverTimelineReceiptRejection::SupersededGeneration,
                        ),
                    ] {
                        let mut trial = server.clone();
                        assert_eq!(
                            trial.verify_receipt(
                                observer,
                                936,
                                CombatViewReceiptV1 {
                                    observer_life_seq: 4,
                                    input_sequence: sequence,
                                    timeline_generation: claimed_generation,
                                    latest_game_applied_snapshot_id: 96,
                                    flags,
                                },
                                &emitted
                            ),
                            Err(expected)
                        );
                        assert!(trial.decision_for_sequence(sequence).is_none());
                        assert_eq!(
                            expected.is_security_violation(),
                            expected != ObserverTimelineReceiptRejection::SupersededGeneration
                        );
                    }
                    let sequence = base.wrapping_add(2);
                    let valid = CombatViewReceiptV1 {
                        observer_life_seq: 4,
                        input_sequence: sequence,
                        timeline_generation: offered_generation,
                        latest_game_applied_snapshot_id: 96,
                        flags,
                    };
                    let decision = server
                        .verify_receipt(observer, 936, valid, &emitted)
                        .unwrap();
                    assert_eq!(decision.weapon_edge_allowed(), flags == 0);
                }
            }
        }
    }

    #[test]
    fn auto_offer_accepts_only_identical_previous_clock_during_delivery_lead() {
        for profile in [
            ObserverNetworkProfile::AutomaticWan,
            ObserverNetworkProfile::CleanLan,
        ] {
            let observer = runtime(3, 99, 4);
            let mut timeline = ObserverCombatTimeline::default();
            let initial = timeline
                .begin_scope(observer, 77, profile, 7, 1_000, 10)
                .unwrap();
            let initial = if profile == ObserverNetworkProfile::AutomaticWan {
                settled_fast_auto_fixture(&mut timeline, initial)
            } else {
                initial
            };
            let target = if profile == ObserverNetworkProfile::AutomaticWan {
                15
            } else {
                9
            };
            let offer = ObserverTimelineDirectiveV1 {
                generation: 8,
                issued_server_tick: 1_100,
                effective_server_tick: 1_150,
                effective_from_input_sequence: 11,
                from_delay_ticks: 7,
                target_delay_ticks: target,
                ..initial
            };
            timeline.issue_delay_schedule(offer).unwrap();
            let mut presentation = ObserverPresentationHistory::new();
            presentation.begin_scope(observer, 77);
            for (sequence, tick, generation, expected) in [
                (
                    11,
                    1_110,
                    7,
                    if profile == ObserverNetworkProfile::AutomaticWan {
                        Ok(7)
                    } else {
                        Err(ObserverTimelineReceiptRejection::GenerationNotEffective)
                    },
                ),
                (
                    12,
                    1_115,
                    99,
                    Err(ObserverTimelineReceiptRejection::UnknownGeneration),
                ),
                (
                    13,
                    1_159,
                    7,
                    if profile == ObserverNetworkProfile::AutomaticWan {
                        Ok(7)
                    } else {
                        Err(ObserverTimelineReceiptRejection::GenerationNotEffective)
                    },
                ),
                (
                    14,
                    1_160,
                    7,
                    Err(if profile == ObserverNetworkProfile::AutomaticWan {
                        ObserverTimelineReceiptRejection::SupersededGeneration
                    } else {
                        ObserverTimelineReceiptRejection::GenerationNotEffective
                    }),
                ),
                (15, 1_160, 8, Ok(8)),
            ] {
                presentation.observe_emitted(presentation_frame(
                    observer,
                    77,
                    sequence,
                    tick,
                    Vec::new(),
                ));
                let receipt = CombatViewReceiptV1 {
                    observer_life_seq: 4,
                    input_sequence: sequence,
                    timeline_generation: generation,
                    latest_game_applied_snapshot_id: sequence,
                    flags: 0,
                };
                assert_eq!(
                    timeline
                        .verify_receipt(observer, tick, receipt, &presentation)
                        .map(|decision| decision.generation),
                    expected
                );
                if sequence == 14 {
                    assert_eq!(
                        timeline.verify_receipt(
                            observer,
                            tick,
                            CombatViewReceiptV1 {
                                timeline_generation: 8,
                                ..receipt
                            },
                            &presentation
                        ),
                        Err(ObserverTimelineReceiptRejection::DuplicateMismatch)
                    );
                }
            }
            let before = timeline.clone();
            assert_eq!(
                timeline.issue_observer_delay_schedule(ObserverTimelineDirectiveV1 {
                    generation: 9,
                    issued_server_tick: 1_300,
                    effective_server_tick: 1_350,
                    effective_from_input_sequence: 15,
                    from_delay_ticks: target,
                    target_delay_ticks: 7,
                    ..offer
                }),
                Err(ObserverScheduleRejection::EffectiveSequence)
            );
            assert_eq!(
                timeline, before,
                "a player input clock cannot use the stopped-observer exception"
            );
        }
    }

    #[test]
    fn future_offer_cannot_jump_a_client_past_an_unfinished_slew() {
        let mut timeline = ObserverCombatTimeline::default();
        let initial = timeline
            .begin_scope(
                runtime(3, 99, 4),
                77,
                ObserverNetworkProfile::AutomaticWan,
                7,
                1_000,
                10,
            )
            .unwrap();
        let initial = settled_fast_auto_fixture(&mut timeline, initial);
        let first = ObserverTimelineDirectiveV1 {
            generation: 8,
            issued_server_tick: 1_100,
            effective_server_tick: 1_150,
            effective_from_input_sequence: 11,
            from_delay_ticks: 7,
            target_delay_ticks: 20,
            ..initial
        };
        timeline.issue_delay_schedule(first).unwrap();
        let from_at_future = timeline.delay_ticks_at(1_250).unwrap();
        assert_ne!(timeline.delay_ticks_at(1_200), Some(from_at_future));
        let before = timeline.clone();
        assert_eq!(
            timeline.issue_delay_schedule(ObserverTimelineDirectiveV1 {
                generation: 9,
                issued_server_tick: 1_200,
                effective_server_tick: 1_250,
                effective_from_input_sequence: 12,
                from_delay_ticks: from_at_future,
                target_delay_ticks: 7,
                ..first
            }),
            Err(ObserverScheduleRejection::FromDelayMismatch)
        );
        assert_eq!(timeline, before);
    }

    #[test]
    fn server_schedule_history_is_bounded_and_wrap_safe_without_evicting_recent_generations() {
        let observer = runtime(3, 99, 4);
        let mut timeline = ObserverCombatTimeline::default();
        let mut previous = timeline
            .begin_scope(
                observer,
                77,
                ObserverNetworkProfile::AutomaticWan,
                u32::MAX,
                0x7fff_fff0,
                u16::MAX - 30,
            )
            .unwrap();
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 77);
        for step in 0..1_000 {
            let now = crate::continuum::continuum_tick_add(previous.issued_server_tick, 100);
            // Advance the actual observer input clock between schedule
            // offers. A thousand future boundaries without any observed
            // input cannot establish which u16 era the observer has reached.
            let snapshot_id = step as u16 + 1;
            presentation.observe_emitted(presentation_frame(
                observer,
                77,
                snapshot_id,
                now,
                Vec::new(),
            ));
            timeline
                .verify_receipt(
                    observer,
                    now,
                    CombatViewReceiptV1 {
                        observer_life_seq: 4,
                        input_sequence: previous.effective_from_input_sequence,
                        timeline_generation: previous.generation,
                        latest_game_applied_snapshot_id: snapshot_id,
                        flags: 0,
                    },
                    &presentation,
                )
                .unwrap();
            let directive = ObserverTimelineDirectiveV1 {
                generation: previous.generation.wrapping_add(1).max(1),
                issued_server_tick: now,
                effective_server_tick: now,
                effective_from_input_sequence: previous
                    .effective_from_input_sequence
                    .wrapping_add(60),
                from_delay_ticks: timeline.delay_ticks_at(now).unwrap(),
                target_delay_ticks: if step % 2 == 0 { 20 } else { 7 },
                ..previous
            };
            timeline.issue_delay_schedule(directive).unwrap();
            assert!(timeline.directives.len() <= OBSERVER_SCHEDULE_HISTORY_CAPACITY);
            assert_eq!(timeline.current_directive(), Some(directive));
            assert_eq!(directive.ceiling_delay_ticks, 20);
            if step == 0 {
                assert_eq!(directive.generation, 1);
            }
            previous = directive;
        }
        assert_eq!(
            timeline.directives.len(),
            OBSERVER_SCHEDULE_HISTORY_CAPACITY
        );
        assert!(
            timeline
                .directives
                .iter()
                .all(|directive| crate::continuum::continuum_tick_diff(
                    previous.issued_server_tick,
                    directive.issued_server_tick
                ) <= 12_700)
        );
    }

    #[test]
    fn production_observer_receipts_are_exact_idempotent_and_fail_closed() {
        let observer = runtime(3, 99, 4);
        let mut timeline = ObserverCombatTimeline::default();
        timeline
            .begin_scope(observer, 77, ObserverNetworkProfile::CleanLan, 7, 1_000, 10)
            .expect("valid scope");
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 77);
        assert_eq!(
            presentation.observe_emitted(presentation_frame(observer, 77, 40, 994, Vec::new(),)),
            ObserverPresentationAppendDisposition::Applied
        );
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: 4,
            input_sequence: 10,
            timeline_generation: 7,
            latest_game_applied_snapshot_id: 40,
            flags: 0,
        };
        let decision = timeline
            .verify_receipt(observer, 1_000, receipt, &presentation)
            .expect("exact emitted view");
        assert_eq!(decision.render_tick, 993);
        assert_eq!(decision.delay_ticks, 7);
        assert!(decision.weapon_edge_allowed());
        assert_eq!(
            timeline.verify_receipt(observer, 1_001, receipt, &presentation),
            Ok(decision),
            "an exact duplicate reproduces the frozen first-arrival decision"
        );
        assert_eq!(
            timeline.verify_receipt(
                observer,
                1_001,
                CombatViewReceiptV1 {
                    latest_game_applied_snapshot_id: 41,
                    ..receipt
                },
                &presentation,
            ),
            Err(ObserverTimelineReceiptRejection::DuplicateMismatch)
        );
        assert_eq!(timeline.decision_for_sequence(10), Some(decision));

        let held = CombatViewReceiptV1 {
            input_sequence: 11,
            flags: COMBAT_VIEW_RECEIPT_FLAG_HOLD_OR_STALL,
            ..receipt
        };
        let held_decision = timeline
            .verify_receipt(observer, 1_002, held, &presentation)
            .expect("bounded hold is valid movement evidence");
        assert!(!held_decision.weapon_edge_allowed());
        let binding = timeline
            .bind_projectile(observer, 10, 1_003)
            .expect("verified non-held input binds a projectile");
        assert_eq!(binding.owner, observer);
        assert_eq!(binding.owner_input_sequence, 10);
        assert_eq!(binding.accepted_timeline_generation, 7);
        assert_eq!(binding.accepted_game_applied_snapshot_id, 40);
        assert_eq!(binding.accepted_server_tick, 1_003);
        assert_eq!(
            binding.maximum_rewind_ticks,
            ObserverNetworkProfile::CleanLan
                .delay_envelope()
                .ceiling_ticks
        );
        assert_eq!(
            timeline.bind_projectile(observer, 11, 1_003),
            Err(ProjectileTimelineBindingRejection::HeldOrStalled)
        );
        assert_eq!(
            timeline.bind_projectile(runtime(3, 100, 4), 10, 1_003),
            Err(ProjectileTimelineBindingRejection::WrongRuntime)
        );
        assert_eq!(
            timeline.bind_projectile(observer, 12, 1_003),
            Err(ProjectileTimelineBindingRejection::UnverifiedInput)
        );
        let query = timeline
            .projectile_presentation_query(binding, 1_004, &presentation)
            .expect("live projectile follows the issued observer schedule");
        assert_eq!(query.accepted_timeline_generation, 7);
        assert_eq!(query.effective_timeline_generation, 7);
        assert_eq!(query.render_tick, 997);
        assert_eq!(query.applied_snapshot_id, 40);
        assert!(!query.server_advanced_watermark);

        presentation.observe_emitted(presentation_frame(observer, 77, 41, 1_010, Vec::new()));
        let advanced = timeline
            .projectile_presentation_query(binding, 1_020, &presentation)
            .expect("withheld application cannot freeze an in-flight projectile");
        assert_eq!(advanced.render_tick, 1_013);
        assert_eq!(advanced.applied_snapshot_id, 41);
        assert!(advanced.server_advanced_watermark);
        for sequence in 12..=80 {
            assert_eq!(
                timeline.verify_receipt(
                    observer,
                    1_020 + u32::from(sequence),
                    CombatViewReceiptV1::unavailable(sequence),
                    &presentation,
                ),
                Err(ObserverTimelineReceiptRejection::Unavailable)
            );
        }
        assert_eq!(
            timeline.decision_for_sequence(10),
            None,
            "the bounded per-input audit ring deliberately evicts old decisions"
        );
        assert!(
            timeline
                .projectile_presentation_query(binding, 1_100, &presentation)
                .is_ok(),
            "one latest verified watermark, not audit-ring retention, owns live projectile projection"
        );
        assert_eq!(
            timeline.projectile_presentation_query(
                ProjectileCombatTimelineV1 {
                    owner: runtime(3, 100, 4),
                    ..binding
                },
                1_020,
                &presentation,
            ),
            Err(ProjectilePresentationQueryRejection::WrongRuntime)
        );
    }

    #[test]
    fn production_observer_receipt_rejections_freeze_and_stay_bounded() {
        let observer = runtime(4, 100, 5);
        let mut timeline = ObserverCombatTimeline::default();
        timeline
            .begin_scope(observer, 78, ObserverNetworkProfile::CleanLan, 8, 1_000, 20)
            .expect("valid scope");
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 78);
        presentation.observe_emitted(presentation_frame(observer, 78, 50, 994, Vec::new()));

        let unavailable = CombatViewReceiptV1::unavailable(20);
        assert_eq!(
            timeline.verify_receipt(observer, 1_000, unavailable, &presentation),
            Err(ObserverTimelineReceiptRejection::Unavailable)
        );
        assert_eq!(
            timeline.verify_receipt(
                observer,
                1_000,
                CombatViewReceiptV1 {
                    observer_life_seq: 5,
                    input_sequence: 20,
                    timeline_generation: 8,
                    latest_game_applied_snapshot_id: 50,
                    flags: 0,
                },
                &presentation,
            ),
            Err(ObserverTimelineReceiptRejection::DuplicateMismatch),
            "a rejected first view cannot be upgraded after seeing later state"
        );
        assert_eq!(
            timeline.receipt_rejection_for_sequence(20),
            Some(ObserverTimelineReceiptRejection::Unavailable),
            "diagnostics must retain the original rejection, not the later rewrite attempt"
        );

        for offset in 1..=80_u16 {
            let receipt = CombatViewReceiptV1 {
                observer_life_seq: 5,
                input_sequence: 20 + offset,
                timeline_generation: 8,
                latest_game_applied_snapshot_id: 50,
                flags: 0,
            };
            assert!(
                timeline
                    .verify_receipt(observer, 1_000, receipt, &presentation)
                    .is_ok()
            );
        }
        assert_eq!(
            timeline.observed_receipts.len(),
            OBSERVER_INPUT_DECISION_HISTORY_CAPACITY
        );

        let next_life = runtime(4, 100, 6);
        timeline.advance_life(next_life).expect("same session");
        presentation.begin_scope(next_life, 78);
        assert!(timeline.observed_receipts.is_empty());
        assert_eq!(timeline.decision_for_sequence(100), None);
        assert_eq!(
            timeline.verify_receipt(
                next_life,
                1_001,
                CombatViewReceiptV1 {
                    observer_life_seq: 5,
                    input_sequence: 101,
                    timeline_generation: 8,
                    latest_game_applied_snapshot_id: 50,
                    flags: 0,
                },
                &presentation,
            ),
            Err(ObserverTimelineReceiptRejection::WrongLife)
        );
    }

    #[test]
    fn client_observer_timeline_activates_at_exact_wrapped_input_boundary() {
        let mut client = ClientObserverCombatTimeline::default();
        let mut value = directive(ObserverNetworkProfile::CleanLan);
        value.generation = 7;
        value.effective_from_input_sequence = 1;

        assert_eq!(
            client.observe_directive(value, u16::MAX),
            ClientObserverTimelineDisposition::Pending
        );
        assert_eq!(client.active_directive(), None);
        assert_eq!(
            client.advance_input(0),
            ClientObserverTimelineDisposition::Pending
        );
        assert_eq!(
            client.advance_input(1),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(client.active_directive(), Some(value));
    }

    #[test]
    fn client_observer_timeline_slews_only_on_server_directed_ticks() {
        let mut client = ClientObserverCombatTimeline::default();
        let mut value = directive(ObserverNetworkProfile::CleanLan);
        value.generation = 8;
        value.effective_server_tick = 100;
        value.effective_from_input_sequence = 4;
        value.from_delay_ticks = 7;
        value.target_delay_ticks = 9;

        assert_eq!(
            client.observe_directive(value, 4),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(client.delay_ticks_at(99), Some(7));
        assert_eq!(client.delay_ticks_at(109), Some(7));
        assert_eq!(client.delay_ticks_at(110), Some(8));
        assert_eq!(client.delay_ticks_at(119), Some(8));
        assert_eq!(client.delay_ticks_at(120), Some(9));
        assert_eq!(client.delay_ticks_at(1_000), Some(9));
        assert_eq!(client.render_tick_at(120), Some(111));
    }

    #[test]
    fn client_observer_timeline_rejects_conflicts_and_accepts_generation_wrap() {
        let mut client = ClientObserverCombatTimeline::default();
        let mut first = directive(ObserverNetworkProfile::CleanLan);
        first.generation = u32::MAX;
        first.effective_from_input_sequence = 10;
        assert_eq!(
            client.observe_directive(first, 10),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(
            client.observe_directive(first, 10),
            ClientObserverTimelineDisposition::Duplicate
        );

        let mut conflict = first;
        conflict.target_delay_ticks += 1;
        assert_eq!(
            client.observe_directive(conflict, 10),
            ClientObserverTimelineDisposition::RejectedConflict
        );

        let mut wrapped = first;
        wrapped.generation = 1;
        wrapped.effective_from_input_sequence = 11;
        assert_eq!(
            client.observe_directive(wrapped, 10),
            ClientObserverTimelineDisposition::Pending
        );

        let mut stale = first;
        stale.generation = u32::MAX - 1;
        assert_eq!(
            client.observe_directive(stale, 10),
            ClientObserverTimelineDisposition::RejectedStale
        );
        assert_eq!(
            client.advance_input(11),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(client.active_directive(), Some(wrapped));
    }

    #[test]
    fn client_observer_timeline_receipts_are_game_applied_and_fail_closed() {
        let mut client = ClientObserverCombatTimeline::default();
        assert_eq!(
            client.receipt(Some(3), 40, Some(200)),
            CombatViewReceiptV1::unavailable(40)
        );

        let mut value = directive(ObserverNetworkProfile::CleanLan);
        value.generation = 12;
        value.effective_server_tick = 190;
        value.effective_from_input_sequence = 40;
        assert_eq!(
            client.observe_directive(value, 40),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(
            client.receipt(Some(3), 40, Some(200)),
            CombatViewReceiptV1::unavailable(40),
            "a decoded directive is not a Game-applied snapshot receipt"
        );
        assert_eq!(
            client.observe_game_applied_snapshot(u16::MAX, 195),
            ClientAppliedSnapshotDisposition::Applied
        );
        assert_eq!(
            client.observe_game_applied_snapshot(u16::MAX, 195),
            ClientAppliedSnapshotDisposition::Duplicate
        );
        assert_eq!(
            client.observe_game_applied_snapshot(u16::MAX - 1, 196),
            ClientAppliedSnapshotDisposition::RejectedStale
        );
        assert_eq!(
            client.observe_game_applied_snapshot(1, 196),
            ClientAppliedSnapshotDisposition::Applied,
            "snapshot receipt ordering is wrap-safe"
        );
        assert_eq!(
            client.advance_presentation_render_tick(200, 1.0 / 60.0),
            Some(193)
        );

        let receipt = client.receipt(Some(3), 41, Some(200));
        assert_eq!(receipt.observer_life_seq, 3);
        assert_eq!(receipt.input_sequence, 41);
        assert_eq!(receipt.timeline_generation, 12);
        assert_eq!(receipt.latest_game_applied_snapshot_id, 1);
        assert!(!receipt.hold_or_stall());

        let same_frame_phase = client.receipt(Some(3), 42, Some(203));
        assert!(
            !same_frame_phase.hold_or_stall(),
            "the 100 Hz clock crossing inside one bounded browser frame is not a scheduler stall"
        );

        let stalled = client.receipt(Some(3), 43, Some(204));
        assert!(stalled.hold_or_stall());
        assert_eq!(
            client.receipt(None, 44, Some(205)),
            CombatViewReceiptV1::unavailable(44)
        );
    }

    #[test]
    fn client_presentation_clock_long_suspensions_keep_the_existing_recovery_bound() {
        for elapsed in [0.150_f32, 0.350, 1.0, 10.0] {
            let mut client = ClientObserverCombatTimeline::default();
            let mut value = directive(ObserverNetworkProfile::CleanLan);
            value.effective_server_tick = 190;
            value.effective_from_input_sequence = 50;
            client.observe_directive(value, 50);
            client.observe_game_applied_snapshot(1, 200);
            assert_eq!(client.advance_presentation_render_tick(200, 0.0), Some(193));
            client.observe_game_applied_snapshot(2, 1_200);
            assert_eq!(
                client.advance_presentation_render_tick(1_200, elapsed),
                Some(198)
            );
            assert!(client.hold_or_stall_at(1_200));
            assert!(client.receipt(Some(3), 51, Some(1_200)).hold_or_stall());
        }
    }

    #[test]
    fn client_presentation_clock_sustained_cadence_does_not_accumulate_lag() {
        // A regular 100 ms frame is within the client's eight-step gameplay
        // catch-up budget. Fresh applied snapshots and a continuously advancing
        // server clock must not leave presentation progressively farther behind.
        // This isolates the retained low-FPS failure from network loss or a
        // one-off scheduler hitch. Existing hitch-recovery tests remain separate.
        for frame_ticks in [2_u32, 3, 5, 10] {
            let mut client = ClientObserverCombatTimeline::default();
            let mut value = directive(ObserverNetworkProfile::CleanLan);
            value.generation = 13;
            value.effective_server_tick = 190;
            value.effective_from_input_sequence = 50;
            assert_eq!(
                client.observe_directive(value, 50),
                ClientObserverTimelineDisposition::Applied
            );
            client.observe_game_applied_snapshot(1, 200);
            client.advance_presentation_render_tick(200, 0.0);
            let mut maximum_lag = 0;
            for frame in 1_u16..=100 {
                let server_tick = 200 + u32::from(frame) * frame_ticks;
                client.observe_game_applied_snapshot(frame + 1, server_tick);
                let rendered = client
                    .advance_presentation_render_tick(server_tick, frame_ticks as f32 / 100.0)
                    .expect("active presentation cursor");
                let scheduled = client.render_tick_at(server_tick).expect("active schedule");
                let lag = crate::continuum::continuum_tick_diff(scheduled, rendered);
                assert!(
                    lag >= 0,
                    "presentation must not run ahead of server authority"
                );
                maximum_lag = maximum_lag.max(lag);
            }
            assert_eq!(
                maximum_lag, 0,
                "steady {frame_ticks}-tick frames accumulate presentation lag despite fresh snapshots"
            );
        }
    }

    #[test]
    fn client_presentation_clock_recovers_while_server_clock_keeps_advancing() {
        for frame_ticks in [2_u32, 3, 5] {
            let mut client = ClientObserverCombatTimeline::default();
            let mut value = directive(ObserverNetworkProfile::CleanLan);
            value.generation = 13;
            value.effective_server_tick = 190;
            value.effective_from_input_sequence = 50;
            client.observe_directive(value, 50);
            client.observe_game_applied_snapshot(1, 200);
            assert_eq!(client.advance_presentation_render_tick(200, 0.0), Some(193));

            // Preserve the existing source-clock-hitch bound. The change from
            // the older recovery test is that fresh server time and snapshots
            // continue arriving during every subsequent render frame.
            client.observe_game_applied_snapshot(2, 235);
            assert_eq!(
                client.advance_presentation_render_tick(235, 0.018),
                Some(196)
            );
            assert!(client.hold_or_stall_at(235));
            for frame in 1_u16..=200 {
                let tick = 235 + u32::from(frame) * frame_ticks;
                client.observe_game_applied_snapshot(frame + 2, tick);
                let rendered = client
                    .advance_presentation_render_tick(tick, frame_ticks as f32 / 100.0)
                    .expect("active cursor");
                assert!(
                    crate::continuum::continuum_tick_diff(
                        client.render_tick_at(tick).expect("active schedule"),
                        rendered
                    ) >= 0
                );
            }
            let final_tick = 235 + 200 * frame_ticks;
            assert!(
                !client.hold_or_stall_at(final_tick),
                "steady {frame_ticks}-tick frames never recover while the server keeps advancing"
            );
        }
    }

    #[test]
    fn client_presentation_clock_smooths_scheduler_hitches_and_holds_fire_until_caught_up() {
        let mut client = ClientObserverCombatTimeline::default();
        let mut value = directive(ObserverNetworkProfile::CleanLan);
        value.generation = 13;
        value.effective_server_tick = 190;
        value.effective_from_input_sequence = 50;
        assert_eq!(
            client.observe_directive(value, 50),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(
            client.observe_game_applied_snapshot(20, 235),
            ClientAppliedSnapshotDisposition::Applied
        );
        assert_eq!(
            client.advance_presentation_render_tick(200, 1.0 / 60.0),
            Some(193)
        );

        let scheduled_after_hitch = client.render_tick_at(235).expect("schedule");
        assert_eq!(scheduled_after_hitch, 228);
        let first_recovery = client
            .advance_presentation_render_tick(235, 0.018)
            .expect("recovery cursor");
        assert_eq!(
            crate::continuum::continuum_tick_diff(first_recovery, 193),
            3,
            "a 350 ms source-clock jump cannot collapse into one render frame"
        );
        assert!(client.hold_or_stall_at(235));
        assert!(client.receipt(Some(3), 51, Some(235)).hold_or_stall());

        let mut previous = first_recovery;
        for _ in 0..32 {
            let current = client
                .advance_presentation_render_tick(235, 1.0 / 60.0)
                .expect("bounded recovery cursor");
            let step = crate::continuum::continuum_tick_diff(current, previous);
            assert!((0..=4).contains(&step), "bounded 200 Hz step: {step}");
            previous = current;
        }
        assert_eq!(
            client.presentation_render_tick(),
            Some(scheduled_after_hitch)
        );
        assert!(!client.hold_or_stall_at(235));
        assert!(!client.receipt(Some(3), 52, Some(235)).hold_or_stall());
        assert!(!client.receipt(Some(3), 53, Some(238)).hold_or_stall());
        assert!(client.receipt(Some(3), 54, Some(239)).hold_or_stall());
    }

    #[test]
    fn client_presentation_clock_retains_fractional_budget_while_caught_up() {
        let mut client = ClientObserverCombatTimeline::default();
        let mut value = directive(ObserverNetworkProfile::CleanLan);
        value.generation = 14;
        value.effective_server_tick = 190;
        value.effective_from_input_sequence = 60;
        assert_eq!(
            client.observe_directive(value, 60),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(
            client.observe_game_applied_snapshot(30, 201),
            ClientAppliedSnapshotDisposition::Applied
        );
        assert_eq!(
            client.advance_presentation_render_tick(200, 0.004),
            Some(193)
        );
        assert!(!client.receipt(Some(3), 60, Some(200)).hold_or_stall());
        assert_eq!(
            client.advance_presentation_render_tick(200, 0.004),
            Some(193)
        );
        // Xvfb/Firefox can present at roughly 4 ms intervals. A caught-up
        // frame retains its 0.8 fractional recovery tick. If the source clock
        // then jumps two ticks, recovery can consume that accumulated budget
        // without collapsing the full backlog into one visible step.
        assert_eq!(
            client.advance_presentation_render_tick(202, 0.004),
            Some(194)
        );
        assert!(client.receipt(Some(3), 61, Some(202)).hold_or_stall());
    }

    #[test]
    fn client_presentation_clock_follows_an_ordinary_tick_at_high_refresh() {
        let mut client = ClientObserverCombatTimeline::default();
        let mut value = directive(ObserverNetworkProfile::CleanLan);
        value.generation = 15;
        value.effective_server_tick = 190;
        value.effective_from_input_sequence = 70;
        assert_eq!(
            client.observe_directive(value, 70),
            ClientObserverTimelineDisposition::Applied
        );
        assert_eq!(
            client.observe_game_applied_snapshot(31, 201),
            ClientAppliedSnapshotDisposition::Applied
        );
        assert_eq!(
            client.advance_presentation_render_tick(200, 0.004),
            Some(193)
        );

        // No prior fractional budget is available after initialization. The
        // next normal target tick must still be drawn immediately because the
        // cursor was caught up, while a multi-tick hitch remains covered by
        // the separate bounded-recovery regression above.
        assert_eq!(
            client.advance_presentation_render_tick(201, 0.004),
            Some(194)
        );
        assert!(!client.receipt(Some(3), 71, Some(201)).hold_or_stall());
    }

    #[test]
    fn history_and_schedule_bounds_cover_the_frozen_v1_window() {
        assert!(
            ACTOR_MOTION_HISTORY_RETENTION_TICKS
                > u32::from(OBSERVER_DELAY_MAX_TICKS)
                    + u32::from(OBSERVER_PRESENTATION_HOLD_GRACE_TICKS)
                    + 2
        );
        assert!(OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS >= 32);
        assert!(
            OBSERVER_SCHEDULE_HISTORY_CAPACITY as u32 * OBSERVER_TIMELINE_MIN_GENERATION_TICKS
                > OBSERVER_SCHEDULE_RETENTION_TICKS
        );
    }

    #[test]
    fn impulse_segments_preserve_last_relocation_and_retire_exact_history() {
        let actor = runtime(7, 41, 3);
        let mut history = ActorMotionHistory::new();
        assert_eq!(
            append_sample(&mut history, actor, 9, 100, 100.0),
            HistoryAppendDisposition::Applied
        );
        let first = history.current_segment().unwrap();
        assert_eq!(history.relocation_generation(), 1);
        let impulse = history.begin_impulse_segment(actor, 9);
        assert_eq!(impulse.discontinuity_generation, 2);
        assert_eq!(history.relocation_generation(), 1);
        assert_eq!(
            append_sample(&mut history, actor, 9, 101, 105.0),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(
            history.at_or_before(impulse, 100),
            Err(HistoryUnavailable::BeforeRetainedWindow)
        );
        assert_eq!(
            history.at_or_before(first, 100).unwrap().position(),
            (100.0, 200.0)
        );
        let relocation = history.begin_segment(actor, 9);
        assert_eq!(relocation.discontinuity_generation, 3);
        assert_eq!(history.relocation_generation(), 3);
        history.begin_impulse_segment(actor, 9);
        history.begin_impulse_segment(actor, 9);
        assert_eq!(
            history.current_segment().unwrap().discontinuity_generation,
            5
        );
        assert_eq!(
            history.relocation_generation(),
            3,
            "missing the relocation and later impulses must not erase it"
        );
        history.begin_impulse_segment(runtime(7, 41, 4), 9);
        assert_eq!(
            history.current_segment().unwrap().discontinuity_generation,
            1
        );
        assert_eq!(
            history.relocation_generation(),
            1,
            "new life owns a new generation scope"
        );
        history.begin_impulse_segment(runtime(7, 41, 4), 10);
        assert_eq!(
            history.relocation_generation(),
            1,
            "new arena owns a new generation scope"
        );
    }

    #[test]
    fn production_motion_history_is_wrap_safe_bounded_and_exact_segment_only() {
        let actor = runtime(7, 41, 3);
        let mut history = ActorMotionHistory::new();
        assert_eq!(
            append_sample(&mut history, actor, 9, u32::MAX - 1, 100.0),
            HistoryAppendDisposition::Applied
        );
        let first_segment = history.current_segment().expect("first segment");
        assert_eq!(first_segment.discontinuity_generation, 1);
        assert_eq!(
            append_sample(&mut history, actor, 9, 1, 104.0),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(
            history
                .at_or_before(first_segment, 0)
                .map(|sample| sample.position()),
            Ok((100.0, 200.0))
        );
        assert_eq!(
            history
                .at_or_before(first_segment, 1)
                .map(|sample| sample.position()),
            Ok((104.0, 200.0))
        );

        let second_segment = history.begin_segment(actor, 9);
        assert_eq!(second_segment.discontinuity_generation, 2);
        assert_eq!(
            append_sample(&mut history, actor, 9, 2, 900.0),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(
            history.at_or_before(second_segment, 1),
            Err(HistoryUnavailable::BeforeRetainedWindow)
        );
        let retired = ActorMotionSampleV1 {
            segment: first_segment,
            server_tick: 3,
            sample_time: PresentationSampleTime::from_tick(3),
            x_q8: 1,
            y_q8: 2,
            velocity_x_q8: 0,
            velocity_y_q8: 0,
            orientation_u16: 0,
            hull_radius_q8: 1,
            protocol_direction: 0,
            safe_or_attached: 0,
            source_kind: ActorMotionSourceKind::AcceptedHumanWitness,
        };
        assert_eq!(
            history.append(retired),
            HistoryAppendDisposition::RejectedRetiredSegment
        );
        assert_eq!(history.current_segment(), Some(second_segment));
    }

    #[test]
    fn production_motion_history_rejects_stale_samples_and_evicts_to_budget() {
        let actor = runtime(8, 42, 1);
        let mut history = ActorMotionHistory::new();
        assert_eq!(
            append_sample(&mut history, actor, 11, 100, 1.0),
            HistoryAppendDisposition::Applied
        );
        assert_eq!(
            append_sample(&mut history, actor, 11, 99, 2.0),
            HistoryAppendDisposition::RejectedStaleTick
        );
        for tick in 101..=180 {
            assert_eq!(
                append_sample(&mut history, actor, 11, tick, tick as f32),
                HistoryAppendDisposition::Applied
            );
        }
        assert!(history.len() <= ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY);
        assert!(history.evictions() > 0);
        let segment = history.current_segment().expect("active segment");
        assert_eq!(
            history.at_or_before(segment, 110),
            Err(HistoryUnavailable::BeforeRetainedWindow)
        );
        assert_eq!(
            history.current_samples().count(),
            ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY
        );
    }

    #[test]
    fn production_motion_history_new_runtime_and_arena_start_new_identity() {
        let actor = runtime(9, 43, 1);
        let next_life = runtime(9, 43, 2);
        let mut history = ActorMotionHistory::new();
        append_sample(&mut history, actor, 12, 100, 10.0);
        let first = history.current_segment().expect("first segment");
        assert_eq!(
            append_sample(&mut history, next_life, 12, 2, 20.0),
            HistoryAppendDisposition::Applied,
            "a new exact segment owns its own wrapped/rebased tick interval"
        );
        let second = history.current_segment().expect("second segment");
        append_sample(&mut history, next_life, 13, 3, 30.0);
        let third = history.current_segment().expect("third segment");

        assert_ne!(first.actor, second.actor);
        assert_eq!(second.discontinuity_generation, 1);
        assert_ne!(second.arena_stream_id, third.arena_stream_id);
        assert_eq!(third.discontinuity_generation, 1);
        assert_eq!(
            history.at_or_before(first, 100).map(|s| s.position().0),
            Ok(10.0)
        );
        assert_eq!(
            history.at_or_before(second, 3).map(|s| s.position().0),
            Ok(20.0)
        );
        assert_eq!(
            history.at_or_before(third, 3).map(|s| s.position().0),
            Ok(30.0)
        );
    }

    #[test]
    fn retired_projectile_history_keeps_soft_life_frames_bounded_and_isolates_hard_scopes() {
        let first = runtime(1, 100, 1);
        let second = runtime(1, 100, 2);
        let mut history = ObserverPresentationHistory::new();
        history.begin_scope(first, 41);
        assert_eq!(
            history.observe_emitted(presentation_frame(first, 41, 1, 100, vec![])),
            ObserverPresentationAppendDisposition::Applied
        );
        history.advance_life(second, 41);
        assert_eq!(history.len(), 1);
        assert_eq!(history.frames().next().unwrap().observer, first);
        assert_eq!(
            history.observe_emitted(presentation_frame(first, 41, 2, 101, vec![])),
            ObserverPresentationAppendDisposition::RejectedScope
        );
        for id in 2..100 {
            assert_eq!(
                history.observe_emitted(presentation_frame(
                    second,
                    41,
                    id,
                    100 + u32::from(id),
                    vec![]
                )),
                ObserverPresentationAppendDisposition::Applied
            );
        }
        assert_eq!(history.len(), OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS);
        history.advance_life(runtime(1, 101, 1), 41);
        assert_eq!(history.len(), 0, "a new session must clear retained frames");
        let third = runtime(1, 101, 1);
        assert_eq!(
            history.observe_emitted(presentation_frame(third, 41, 1, 201, vec![])),
            ObserverPresentationAppendDisposition::Applied
        );
        history.advance_life(third, 42);
        assert_eq!(history.len(), 0, "a new arena must clear retained frames");
    }

    #[test]
    fn presentation_history_is_exact_scope_wrap_safe_sorted_and_bounded() {
        let observer = runtime(1, 100, 4);
        let next_observer = runtime(1, 101, 1);
        let actor = runtime(8, 200, 3);
        let other_actor = runtime(7, 201, 9);
        let arena_stream_id = 41;
        let segment = MotionSegmentIdentity {
            actor,
            arena_stream_id,
            discontinuity_generation: 2,
        };
        let other_segment = MotionSegmentIdentity {
            actor: other_actor,
            // Actor motion belongs to the actor's arena/bot stream, not the
            // observing recipient's independently allocated stream.
            arena_stream_id: 99,
            discontinuity_generation: 1,
        };
        let mut history = ObserverPresentationHistory::new();
        let first = presentation_frame(
            observer,
            arena_stream_id,
            u16::MAX - 1,
            10,
            vec![
                presentation_sample(actor, segment, 10, 0, false),
                presentation_sample(other_actor, other_segment, 20, 0, false),
            ],
        );
        assert_eq!(
            history.observe_emitted(first.clone()),
            ObserverPresentationAppendDisposition::RejectedScope
        );
        history.begin_scope(observer, arena_stream_id);
        assert_eq!(
            history.observe_emitted(first.clone()),
            ObserverPresentationAppendDisposition::Applied
        );
        let actors = &history.frames().next().expect("first frame").actors;
        assert_eq!(
            actors[0].actor(),
            other_actor,
            "actors sort by exact runtime"
        );
        assert_eq!(actors[1].actor(), actor);

        let duplicate = first.clone();
        assert_eq!(
            history.observe_emitted(duplicate),
            ObserverPresentationAppendDisposition::Duplicate
        );
        for (snapshot_id, tick) in [(u16::MAX, 11), (1, 12)] {
            assert_eq!(
                history.observe_emitted(presentation_frame(
                    observer,
                    arena_stream_id,
                    snapshot_id,
                    tick,
                    vec![presentation_sample(actor, segment, tick as i32, 0, false)],
                )),
                ObserverPresentationAppendDisposition::Applied
            );
        }
        assert_eq!(
            history.observe_emitted(presentation_frame(
                observer,
                arena_stream_id,
                u16::MAX,
                13,
                vec![presentation_sample(actor, segment, 13, 0, false)],
            )),
            ObserverPresentationAppendDisposition::RejectedStaleSnapshot
        );
        for snapshot_id in 2..=40 {
            assert_eq!(
                history.observe_emitted(presentation_frame(
                    observer,
                    arena_stream_id,
                    snapshot_id,
                    u32::from(snapshot_id) + 12,
                    vec![presentation_sample(
                        actor,
                        segment,
                        i32::from(snapshot_id),
                        0,
                        false,
                    )],
                )),
                ObserverPresentationAppendDisposition::Applied
            );
        }
        assert_eq!(history.len(), OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS);
        assert_eq!(history.evictions(), 10);

        history.begin_scope(next_observer, arena_stream_id);
        assert_eq!(history.len(), 0, "a reconnect cannot inherit old frames");
        assert_eq!(
            history.observe_emitted(presentation_frame(
                observer,
                arena_stream_id,
                41,
                53,
                vec![],
            )),
            ObserverPresentationAppendDisposition::RejectedScope
        );
    }

    #[test]
    fn presentation_history_rejects_malformed_identity_and_duplicate_actor_frames() {
        let observer = runtime(2, 102, 1);
        let actor = runtime(9, 202, 2);
        let arena_stream_id = 42;
        let segment = MotionSegmentIdentity {
            actor,
            arena_stream_id,
            discontinuity_generation: 1,
        };
        let mut history = ObserverPresentationHistory::new();
        history.begin_scope(observer, arena_stream_id);

        for frame in [
            presentation_frame(observer, arena_stream_id, 0, 1, vec![]),
            presentation_frame(
                observer,
                arena_stream_id,
                1,
                1,
                vec![
                    presentation_sample(actor, segment, 0, 0, false),
                    presentation_sample(actor, segment, 1, 0, false),
                ],
            ),
            presentation_frame(observer, arena_stream_id, 1, 1, {
                let mismatched_actor = runtime(10, 203, 1);
                vec![presentation_sample(
                    actor,
                    MotionSegmentIdentity {
                        actor: mismatched_actor,
                        arena_stream_id,
                        discontinuity_generation: 1,
                    },
                    0,
                    0,
                    false,
                )]
            }),
        ] {
            assert_eq!(
                history.observe_emitted(frame),
                ObserverPresentationAppendDisposition::RejectedMalformedFrame
            );
        }
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn authority_selection_matches_last_eligible_authored_sample() {
        let segment = MotionSegmentIdentity {
            actor: runtime(10, 203, 5),
            arena_stream_id: 43,
            discontinuity_generation: 1,
        };
        for seed in 0..12_i32 {
            let mut samples = Vec::new();
            let mut authored = Vec::new();
            for ordinal in 0..32_i32 {
                let offset = (ordinal * 7 + seed * 3) % 25 - 12;
                let third = ordinal % 3;
                let scoped = (ordinal + seed) % 5 != 0;
                let mut sample = ActorMotionSampleV1::from_state(
                    segment,
                    (offset as u32) & 0x7fff_ffff,
                    ordinal as f32,
                    200.0,
                    0.0,
                    0.0,
                    0.0,
                    14.0,
                    0,
                    ActorMotionSourceKind::AcceptedHumanWitness,
                );
                sample.sample_time.third = third as u8;
                if !scoped {
                    sample.segment.discontinuity_generation += 1;
                }
                samples.push(sample);
                authored.push((offset * 3 + third, scoped));
            }
            for retained in 0..=samples.len() {
                for render in -15_i32..=15 {
                    let mut expected = None;
                    for index in 0..retained {
                        if authored[index].1 && authored[index].0 <= render * 3 {
                            expected = Some(samples[index]);
                        }
                    }
                    let expected = expected.ok_or(if retained == 0 {
                        HistoryUnavailable::Empty
                    } else if !authored[..retained].iter().any(|entry| entry.1) {
                        HistoryUnavailable::WrongSegment
                    } else {
                        HistoryUnavailable::BeforeRetainedWindow
                    });
                    assert_eq!(
                        actor_motion_sample_at_or_before(
                            samples[..retained].iter().copied(),
                            segment,
                            (render as u32) & 0x7fff_ffff,
                        ),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn packed_projection_selection_matches_independent_unpacked_schedule() {
        // Select expected bounds from authored signed source-time offsets and
        // emission ordinals, not the packed lookup, wrap helpers, or resolver.
        // Include repeated/nonmonotonic source times: emission order alone is
        // not sufficient to choose either interpolation bound.
        let observer = runtime(3, 103, 1);
        let actor = runtime(10, 203, 5);
        let segment = MotionSegmentIdentity {
            actor,
            arena_stream_id: 43,
            discontinuity_generation: 1,
        };
        for seed in 0..8_i32 {
            let mut history = ObserverPresentationHistory::new();
            history.begin_scope(observer, 43);
            let mut schedule = Vec::new();
            for ordinal in 0..32_i32 {
                let id = if ordinal < 16 {
                    (65520 + ordinal) as u16
                } else {
                    (ordinal - 15) as u16
                };
                let source_quanta = ((ordinal * 7 + seed * 11) % 25 - 12) * 3 + ordinal % 3;
                let mut sample = presentation_sample(actor, segment, 0, 0, false);
                sample.sample_time = PresentationSampleTime {
                    tick: (source_quanta.div_euclid(3) as u32) & 0x7fff_ffff,
                    third: source_quanta.rem_euclid(3) as u8,
                };
                let matches_segment = (ordinal + seed) % 5 != 0;
                if !matches_segment {
                    sample.motion_segment.discontinuity_generation += 1;
                }
                let frame = ObserverPresentationFrameV1 {
                    observer,
                    arena_stream_id: 43,
                    snapshot_id: id,
                    server_tick: ordinal as u32,
                    actors: vec![sample],
                };
                assert_eq!(
                    history.observe_emitted(frame),
                    ObserverPresentationAppendDisposition::Applied
                );
                schedule.push((id, source_quanta, matches_segment));
            }
            for applied in 0..schedule.len() {
                for render_offset in -15_i32..=15 {
                    let eligible: Vec<_> = schedule[..=applied]
                        .iter()
                        .filter(|entry| entry.2)
                        .collect();
                    let lower = eligible
                        .iter()
                        .filter(|entry| entry.1 <= render_offset * 3)
                        .min_by_key(|entry| render_offset * 3 - entry.1);
                    let upper = eligible
                        .iter()
                        .filter(|entry| entry.1 >= render_offset * 3)
                        .min_by_key(|entry| entry.1 - render_offset * 3);
                    let tick = (render_offset as u32) & 0x7fff_ffff;
                    let authority = ActorMotionSampleV1::from_state(
                        segment,
                        tick,
                        0.0,
                        200.0,
                        0.0,
                        0.0,
                        0.0,
                        14.0,
                        0,
                        ActorMotionSourceKind::AcceptedHumanWitness,
                    );
                    let actual = PresentationGeometryResolver::resolve_from_samples(
                        &history,
                        segment,
                        tick,
                        schedule[applied].0,
                        &[authority],
                    );
                    match (lower, upper) {
                        (None, None) => assert_eq!(
                            actual,
                            Err(PresentationProjectionUnavailable::NoAppliedSample)
                        ),
                        (None, Some(_)) => assert_eq!(
                            actual,
                            Err(PresentationProjectionUnavailable::BeforeFirstSample)
                        ),
                        (Some(lower), upper) => {
                            let actual =
                                actual.expect("constant geometry is within the authority bound");
                            assert_eq!(actual.lower_snapshot_id, lower.0);
                            assert_eq!(
                                actual.upper_snapshot_id,
                                upper.map_or(lower.0, |entry| entry.0)
                            );
                            assert_eq!(
                                (actual.x_q8, actual.y_q8, actual.authority_error_q8),
                                (0, 200 * 256, 0)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn presentation_history_transaction_clones_keep_independent_retention_and_scope() {
        let observer = runtime(3, 103, 1);
        let actor = runtime(10, 203, 5);
        let mut motion = ActorMotionHistory::new();
        let segment = append_motion_at(&mut motion, actor, 43, 100, 0, 100);
        let frame = |id| {
            presentation_frame(
                observer,
                43,
                id,
                100 + u32::from(id),
                vec![presentation_sample(
                    actor,
                    segment,
                    i32::from(id),
                    100,
                    false,
                )],
            )
        };
        let mut original = ObserverPresentationHistory::new();
        original.begin_scope(observer, 43);
        assert_eq!(
            original.observe_emitted(frame(1)),
            ObserverPresentationAppendDisposition::Applied
        );
        let mut staged = original.clone();
        for id in 2..=(OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS as u16 + 2) {
            assert_eq!(
                staged.observe_emitted(frame(id)),
                ObserverPresentationAppendDisposition::Applied
            );
        }
        assert_eq!(original.len(), 1);
        assert_eq!(original.evictions(), 0);
        assert!(original.frames().next().unwrap().matches(&frame(1)));
        assert_eq!(staged.len(), OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS);
        assert_eq!(staged.evictions(), 2);
        let committed = staged.clone();
        staged.begin_scope(runtime(3, 104, 1), 44);
        assert_eq!(staged.len(), 0);
        assert_eq!(committed.len(), OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS);
        assert_eq!(
            original.observe_emitted(frame(1)),
            ObserverPresentationAppendDisposition::Duplicate
        );
        assert!(
            committed
                .frames()
                .last()
                .unwrap()
                .matches(&frame(OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS as u16 + 2))
        );
    }

    #[test]
    fn presentation_geometry_resolves_exact_interpolated_and_bounded_hold() {
        let observer = runtime(3, 103, 1);
        let actor = runtime(10, 203, 5);
        let arena_stream_id = 43;
        let mut motion = ActorMotionHistory::new();
        let segment = append_motion_at(&mut motion, actor, arena_stream_id, 100, 0, 100);
        append_motion_at(&mut motion, actor, arena_stream_id, 105, 5, 100);
        append_motion_at(&mut motion, actor, arena_stream_id, 110, 10, 100);
        let mut history = ObserverPresentationHistory::new();
        history.begin_scope(observer, arena_stream_id);
        for (snapshot_id, tick, x) in [(u16::MAX, 100, 0), (1, 110, 10)] {
            assert_eq!(
                history.observe_emitted(presentation_frame(
                    observer,
                    arena_stream_id,
                    snapshot_id,
                    tick,
                    vec![presentation_sample(actor, segment, x, 100, false)],
                )),
                ObserverPresentationAppendDisposition::Applied
            );
        }

        let exact =
            PresentationGeometryResolver::resolve(&history, segment, 100, u16::MAX, &motion)
                .expect("exact geometry");
        assert_eq!(exact.disposition, PresentationProjectionDisposition::Exact);
        assert_eq!(exact.x_q8, 0);

        let interpolated =
            PresentationGeometryResolver::resolve(&history, segment, 105, 1, &motion)
                .expect("interpolated geometry");
        assert_eq!(
            interpolated.disposition,
            PresentationProjectionDisposition::Interpolated
        );
        assert_eq!(interpolated.x_q8, 5 * 256);
        assert_eq!(interpolated.authority_error_q8, 0);
        assert_eq!(interpolated.lower_snapshot_id, u16::MAX);
        assert_eq!(interpolated.upper_snapshot_id, 1);
        let motion_samples = motion.current_samples().collect::<Vec<_>>();
        assert_eq!(
            PresentationGeometryResolver::resolve_from_samples(
                &history,
                segment,
                105,
                1,
                &motion_samples,
            ),
            Ok(interpolated),
            "the allocation-free production view matches the owned reference history"
        );

        let held = PresentationGeometryResolver::resolve(&history, segment, 105, u16::MAX, &motion)
            .expect("bounded hold uses the last applied sample");
        assert_eq!(
            held.disposition,
            PresentationProjectionDisposition::HeldNoUpperSample
        );
        assert_eq!(held.x_q8, 0, "hold never extrapolates velocity");
        assert_eq!(held.authority_error_q8, 5 * 256);
    }

    #[test]
    fn presentation_geometry_fails_closed_on_watermark_segment_and_authority_errors() {
        let observer = runtime(4, 104, 1);
        let actor = runtime(11, 204, 5);
        let arena_stream_id = 44;
        let mut motion = ActorMotionHistory::new();
        let segment = append_motion_at(&mut motion, actor, arena_stream_id, 100, 0, 0);
        let mut history = ObserverPresentationHistory::new();
        history.begin_scope(observer, arena_stream_id);
        history.observe_emitted(presentation_frame(
            observer,
            arena_stream_id,
            9,
            100,
            vec![presentation_sample(actor, segment, 13, 0, false)],
        ));

        assert_eq!(
            PresentationGeometryResolver::resolve(&history, segment, 100, 0, &motion),
            Err(PresentationProjectionUnavailable::NoAppliedSample)
        );
        assert_eq!(
            PresentationGeometryResolver::resolve(&history, segment, 99, 9, &motion),
            Err(PresentationProjectionUnavailable::BeforeFirstSample)
        );
        assert_eq!(
            PresentationGeometryResolver::resolve(
                &history,
                MotionSegmentIdentity {
                    actor,
                    arena_stream_id,
                    discontinuity_generation: 2,
                },
                100,
                9,
                &motion,
            ),
            Err(PresentationProjectionUnavailable::WrongSegment)
        );
        assert_eq!(
            PresentationGeometryResolver::resolve(&history, segment, 100, 9, &motion),
            Err(PresentationProjectionUnavailable::AuthorityErrorExceeded)
        );
    }

    #[test]
    fn presentation_geometry_reproduces_immediate_browser_discontinuity_snap() {
        let observer = runtime(5, 105, 1);
        let actor = runtime(12, 205, 5);
        let arena_stream_id = 45;
        let mut motion = ActorMotionHistory::new();
        let segment = append_motion_at(&mut motion, actor, arena_stream_id, 100, 0, 100);
        append_motion_at(&mut motion, actor, arena_stream_id, 105, 10, 100);
        let mut history = ObserverPresentationHistory::new();
        history.begin_scope(observer, arena_stream_id);
        for (snapshot_id, tick, x, snap) in [(1, 100, 0, false), (2, 110, 10, true)] {
            history.observe_emitted(presentation_frame(
                observer,
                arena_stream_id,
                snapshot_id,
                tick,
                vec![presentation_sample(actor, segment, x, 100, snap)],
            ));
        }
        let anchor = PresentationGeometryResolver::resolve(&history, segment, 105, 2, &motion)
            .expect("snap geometry");
        assert_eq!(
            anchor.disposition,
            PresentationProjectionDisposition::SnappedDiscontinuity
        );
        assert_eq!(anchor.x_q8, 10 * 256);
        assert_eq!(anchor.authority_error_q8, 0);
    }
}
