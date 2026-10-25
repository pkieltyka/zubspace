use super::*;

// The server's dense per-recipient projection ring stays at 32 emitted
// snapshots. The browser's per-actor ring is cheaper and must cover the full
// 64-tick bounded cursor-recovery window: otherwise a scheduler pause can put
// the cursor before its oldest actor sample, visibly freezing the sprite until
// recovery reaches retained history. Sixty-five samples are the conservative
// one-sample-per-tick bound already used by ActorMotionHistory.
const DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS: usize =
    crate::combat_timeline::ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirectedPresentationSample {
    snapshot_id: u16,
    server_tick: u32,
    sample_time: crate::combat_timeline::PresentationSampleTime,
    kinematics: crate::combat_timeline::PresentationKinematicSampleV1,
    orientation_u16: u16,
    snap_discontinuity: bool,
    motion_segment: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetworkPlayer {
    pub(crate) player_id: u16,
    /// Authoritative application lifecycle generation for this roster slot.
    /// `None` only until the first zubspace lifecycle-bearing ship update or
    /// death is observed; browser evidence must not infer a generation from a
    /// reusable numeric player id alone.
    pub(crate) life_seq: Option<u32>,
    /// Authenticated connection identity learned from a current-life 0x7d
    /// event. Cleared whenever lifecycle generation changes.
    pub(crate) source_session_id: Option<u64>,
    /// Presentation discontinuities are counted on the exact current life.
    /// These counters reset whenever authoritative lifecycle identity changes,
    /// so another player or an older generation cannot alias browser evidence.
    pub(crate) presentation_snap_count: u32,
    pub(crate) presentation_initial_count: u32,
    pub(crate) last_presentation_snap_reason: u8,
    pub(crate) name: String,
    pub(crate) squad: String,
    pub(crate) ship: u8,
    pub(crate) frequency: u16,
    pub(crate) position: Option<Vec2>,
    pub(crate) previous_position: Option<Vec2>,
    pub(crate) render_position: Option<Vec2>,
    /// True after this actor has received exact snapshot provenance under the
    /// server-owned observer schedule. While active, display must never fall
    /// back to the latest authoritative position ahead of the directed clock.
    pub(crate) directed_presentation: bool,
    pub(crate) directed_render_orientation: Option<f32>,
    /// Time of the position actually displayed, which can precede the shared
    /// cursor when this actor lacks a bracketing snapshot or holds a segment.
    pub(crate) directed_render_time: Option<crate::combat_timeline::PresentationSampleTime>,
    /// Render-only authoritative firing cue. Position, aim and weapon physics
    /// remain on their existing clocks. Kept until history moves past the cue
    /// so the next frame cannot chase an older pre-shot heading backwards.
    pub(crate) directed_firing_heading:
        Option<(crate::combat_timeline::PresentationSampleTime, f32)>,
    pub(crate) directed_presentation_history:
        std::collections::VecDeque<DirectedPresentationSample>,
    pub(crate) interpolation_start_position: Option<Vec2>,
    pub(crate) interpolation_target_position: Option<Vec2>,
    pub(crate) interpolation_elapsed: f32,
    pub(crate) interpolation_duration: f32,
    /// Velocity (px/s) sampled at the start of the active interpolation
    /// segment. Used as the incoming Hermite tangent so a remote ship
    /// curves smoothly through turns instead of changing direction
    /// abruptly at each snapshot boundary.
    pub(crate) interpolation_start_velocity: Vec2,
    /// Real seconds between the two server samples bounding the active
    /// interpolation segment. Hermite tangents are scaled by this so that
    /// straight constant-velocity motion reproduces the linear chord
    /// exactly (no overshoot); only velocity *changes* add curvature.
    pub(crate) interpolation_sample_interval: f32,
    /// EWMA of the inter-sample gap (seconds) and its deviation, used to
    /// size the interpolation/playback buffer adaptively: tight + low
    /// latency on steady links, wider under jitter.
    pub(crate) interp_mean_gap_seconds: f32,
    pub(crate) interp_jitter_seconds: f32,
    pub(crate) velocity: Vec2,
    pub(crate) orientation: f32,
    pub(crate) previous_orientation: Option<f32>,
    /// The wire heading this actor held before the current one. Both render paths - the legacy
    /// blend and the server-directed bounded rotation - only ever move the drawn heading between
    /// that heading and `orientation`, so a browser gate can tell a legitimately trailing sprite
    /// apart from a heading the server never sent.
    pub(crate) previous_authoritative_orientation: Option<f32>,
    pub(crate) orientation_interpolation_elapsed: f32,
    pub(crate) orientation_interpolation_duration: f32,
    pub(crate) bounty: u16,
    pub(crate) kill_points: u32,
    pub(crate) assists: u32,
    pub(crate) flag_points: u32,
    pub(crate) wins: u16,
    pub(crate) losses: u16,
    pub(crate) attach_parent: u16,
    pub(crate) flags: u16,
    pub(crate) flag_timer: Option<u16>,
    pub(crate) banner: Option<[u8; PLAYER_BANNER_BYTES]>,
    pub(crate) ping: u8,
    pub(crate) energy: Option<f32>,
    pub(crate) togglables: u8,
    pub(crate) weapon: u16,
    pub(crate) last_server_position_tick: Option<u16>,
    pub(crate) snapshot_position_owned: bool,
    pub(crate) snapshot_motion_segment: Option<u32>,
    pub(crate) snapshot_relocation_generation: Option<u32>,
    /// Phase 3e-3: snapshot id at which this player last appeared in
    /// the local recipient's near or mid AOI. `None` if never seen
    /// (or after a sweep cleared them). Used by the snapshot diff to
    /// despawn render proxies for entities that have left the AOI.
    pub(crate) last_seen_snapshot_id: Option<u16>,
}

impl NetworkPlayer {
    pub(crate) fn from_roster(entry: &PlayerRosterEntry) -> Self {
        Self {
            player_id: entry.player_id,
            life_seq: None,
            source_session_id: None,
            presentation_snap_count: 0,
            presentation_initial_count: 0,
            last_presentation_snap_reason: 0,
            name: entry.name.clone(),
            squad: entry.squad.clone(),
            ship: entry.ship,
            frequency: entry.frequency,
            position: None,
            previous_position: None,
            render_position: None,
            directed_presentation: false,
            directed_render_orientation: None,
            directed_render_time: None,
            directed_firing_heading: None,
            directed_presentation_history: std::collections::VecDeque::with_capacity(
                DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS,
            ),
            interpolation_start_position: None,
            interpolation_target_position: None,
            interpolation_elapsed: 0.0,
            interpolation_duration: 0.0,
            interpolation_start_velocity: Vec2::ZERO,
            interpolation_sample_interval: 0.0,
            interp_mean_gap_seconds: 0.0,
            interp_jitter_seconds: 0.0,
            velocity: Vec2::ZERO,
            orientation: 0.0,
            previous_authoritative_orientation: None,
            previous_orientation: None,
            orientation_interpolation_elapsed: 0.0,
            orientation_interpolation_duration: 0.0,
            bounty: 0,
            kill_points: entry.kill_points,
            assists: 0,
            flag_points: entry.flag_points,
            wins: entry.wins,
            losses: entry.losses,
            attach_parent: entry.attach_parent,
            flags: entry.flags,
            flag_timer: None,
            banner: None,
            ping: 0,
            energy: None,
            togglables: 0,
            weapon: 0,
            last_server_position_tick: None,
            snapshot_position_owned: false,
            snapshot_motion_segment: None,
            snapshot_relocation_generation: None,
            last_seen_snapshot_id: None,
        }
    }

    pub(crate) fn set_life_seq(&mut self, life_seq: Option<u32>) {
        if self.life_seq == life_seq {
            return;
        }
        self.life_seq = life_seq;
        self.source_session_id = None;
        self.snapshot_motion_segment = None;
        self.snapshot_relocation_generation = None;
        self.presentation_snap_count = 0;
        self.presentation_initial_count = 0;
        self.last_presentation_snap_reason = 0;
        self.directed_presentation_history.clear();
        self.directed_render_orientation = None;
        self.directed_render_time = None;
        self.directed_firing_heading = None;
        if self.directed_presentation {
            self.render_position = None;
        }
    }

    pub(crate) fn bind_source_session(&mut self, session_id: u64, life_seq: u32) -> bool {
        if session_id == 0 || life_seq == 0 || self.life_seq != Some(life_seq) {
            return false;
        }
        if self.source_session_id != Some(session_id) {
            self.directed_firing_heading = None;
        }
        self.source_session_id = Some(session_id);
        true
    }

    pub(crate) fn record_presentation_snap(&mut self, reason: RemoteSnapReason) {
        match reason {
            RemoteSnapReason::None => return,
            RemoteSnapReason::Initial => {
                self.presentation_initial_count = self.presentation_initial_count.wrapping_add(1);
            }
            RemoteSnapReason::Distance | RemoteSnapReason::Flash => {
                self.presentation_snap_count = self.presentation_snap_count.wrapping_add(1);
            }
        }
        self.last_presentation_snap_reason = reason.code();
    }

    /// An acknowledged-base delta repeats a relocation generation until it is
    /// received, including when the first post-warp snapshot is lost. Consume
    /// the edge only once; ordinary samples in the same segment stay smooth.
    pub(crate) fn accept_snapshot_motion_segment(
        &mut self,
        segment: Option<u32>,
        relocation_generation: Option<u32>,
    ) -> bool {
        let (Some(segment), Some(relocation)) = (
            segment.filter(|segment| *segment != 0),
            relocation_generation.filter(|generation| *generation != 0),
        ) else {
            return false;
        };
        self.snapshot_motion_segment = Some(segment);
        let prior_relocation = self.snapshot_relocation_generation.replace(relocation);
        let relocated = prior_relocation.is_some_and(|previous| previous != relocation);
        if relocated {
            // A relocation invalidates predecessor geometry. An impulse does
            // not erase already observed positions that the delayed cursor
            // has yet to draw. Retain them within the existing bounded ring;
            // each sample's segment prevents interpolation across the edge.
            // This grants no server collision or weapon-admission authority.
            self.directed_presentation_history.clear();
            self.directed_render_orientation = None;
            self.directed_render_time = None;
            self.directed_firing_heading = None;
        }
        relocated
    }

    pub(crate) fn apply_position(&mut self, update: &PlayerPositionUpdate) -> NetworkPositionApply {
        if !self.accepts_position_timestamp(update.timestamp, POSITION_TIMESTAMP_FULL_MASK) {
            return NetworkPositionApply::Stale;
        }
        let snap_reason = self.apply_position_core(update);
        self.togglables = update.togglables;
        NetworkPositionApply::Applied {
            flash_warp: self.take_flash_warp_effect(),
            snap_reason,
        }
    }

    pub(crate) fn apply_position_preserving_high_togglables(
        &mut self,
        update: &PlayerPositionUpdate,
    ) -> NetworkPositionApply {
        if !self.accepts_position_timestamp(update.timestamp, POSITION_TIMESTAMP_BATCHED_MASK) {
            return NetworkPositionApply::Stale;
        }
        let snap_reason = self.apply_position_core(update);
        self.togglables = (update.togglables & 0x3f) | (self.togglables & 0xc0);
        NetworkPositionApply::Applied {
            flash_warp: self.take_flash_warp_effect(),
            snap_reason,
        }
    }

    pub(crate) fn apply_position_without_togglables(
        &mut self,
        update: &PlayerPositionUpdate,
    ) -> NetworkPositionApply {
        if !self.accepts_position_timestamp(update.timestamp, POSITION_TIMESTAMP_BATCHED_MASK) {
            return NetworkPositionApply::Stale;
        }
        let snap_reason = self.apply_position_core(update);
        NetworkPositionApply::Applied {
            flash_warp: self.take_flash_warp_effect(),
            snap_reason,
        }
    }

    pub(crate) fn apply_position_core(
        &mut self,
        update: &PlayerPositionUpdate,
    ) -> RemoteSnapReason {
        let previous_position = self.position;
        let position = vec2(update.x as f32, update.y as f32);
        let next_orientation =
            crate::continuum_position::protocol_direction_to_orientation(update.direction);
        let (snap_reason, legacy_orientation_interpolation) = if self.directed_presentation {
            let reason = if update.togglables & STATUS_FLASH != 0 {
                RemoteSnapReason::Flash
            } else if previous_position.is_none() {
                RemoteSnapReason::Initial
            } else if previous_position.is_some_and(|previous| {
                previous.distance(position) > REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE
            }) {
                RemoteSnapReason::Distance
            } else {
                RemoteSnapReason::None
            };
            (reason, None)
        } else {
            let current_display_orientation = self.display_orientation();
            // Both reads happen before `last_server_position_tick` is advanced
            // below, so they see the previous sample's tick.
            let sample_interval = self.sample_interval_seconds(update.timestamp);
            let duration = self.adaptive_interpolation_duration(sample_interval);
            let reason = self.begin_render_interpolation(
                previous_position,
                position,
                update,
                duration,
                sample_interval,
            );
            (reason, Some((current_display_orientation, duration)))
        };
        self.position = Some(position);
        self.velocity = vec2(
            crate::continuum_position::protocol_velocity_pixels_per_second(update.velocity_x),
            crate::continuum_position::protocol_velocity_pixels_per_second(update.velocity_y),
        );
        if let Some((current_display_orientation, duration)) = legacy_orientation_interpolation {
            self.begin_orientation_interpolation(
                current_display_orientation,
                next_orientation,
                snap_reason,
                duration,
            );
        }
        self.previous_authoritative_orientation = Some(self.orientation);
        self.orientation = next_orientation;
        self.bounty = update.bounty;
        self.ping = update.ping;
        self.weapon = update.weapon;
        self.flag_timer = update.flag_timer;
        self.last_server_position_tick = Some(update.timestamp);
        if let Some(energy) = update.energy {
            self.energy = Some(energy as f32);
        }
        self.previous_position = previous_position;
        snap_reason
    }

    pub(crate) fn update_render_position(&mut self, dt: f32) -> bool {
        // This is presentation-only Hermite interpolation with an optional
        // bounded extrapolation tail (currently configured to zero). It is not
        // the reference game's 10 ms remote packet catch-up simulation and must never
        // call the authoritative live-player collision integrator.
        self.update_render_orientation(dt);
        let Some(target) = self.interpolation_target_position else {
            if self.render_position.is_none() {
                self.render_position = self.position;
            }
            return false;
        };
        let Some(start) = self.interpolation_start_position else {
            self.render_position = Some(target);
            return false;
        };

        self.interpolation_elapsed = (self.interpolation_elapsed + dt.max(0.0))
            .min(self.interpolation_duration + REMOTE_PLAYER_EXTRAPOLATION_LIMIT_SECONDS);
        if self.interpolation_duration <= f32::EPSILON {
            self.render_position = Some(target);
            return false;
        }

        let t = (self.interpolation_elapsed / self.interpolation_duration).clamp(0.0, 1.0);
        // Velocity-aware cubic Hermite. Tangents are scaled by the real
        // sample interval, so straight constant-velocity motion reproduces
        // the linear chord while velocity changes (turns) round off smoothly.
        // Each tangent is clamped to the chord length, which (a) collapses a
        // stationary segment to a hold instead of a wobble, (b) bounds
        // overshoot when the reported velocity disagrees with the position
        // delta, and (c) keeps straight coasting visually linear. Falls back
        // to a plain lerp when no sample interval is known (post-snap frame).
        let interpolated = if self.interpolation_sample_interval > f32::EPSILON {
            let max_tangent =
                REMOTE_PLAYER_INTERPOLATION_TANGENT_CHORD_FACTOR * start.distance(target);
            let start_tangent = clamp_length(
                self.interpolation_start_velocity * self.interpolation_sample_interval,
                max_tangent,
            );
            let target_tangent = clamp_length(
                self.velocity * self.interpolation_sample_interval,
                max_tangent,
            );
            hermite(start, target, start_tangent, target_tangent, t)
        } else {
            start.lerp(target, t)
        };
        if self.interpolation_elapsed <= self.interpolation_duration {
            self.render_position = Some(bounded_render_position(
                self.render_position.unwrap_or(start),
                interpolated,
                target,
                dt,
                self.velocity.length(),
            ));
            return false;
        }

        let extrapolated_seconds = (self.interpolation_elapsed - self.interpolation_duration)
            .clamp(0.0, REMOTE_PLAYER_EXTRAPOLATION_LIMIT_SECONDS);
        self.render_position = Some(bounded_render_position(
            self.render_position.unwrap_or(start),
            target + self.velocity * extrapolated_seconds,
            target,
            dt,
            self.velocity.length(),
        ));
        extrapolated_seconds > 0.0
    }

    pub(crate) fn display_position(&self) -> Option<Vec2> {
        if self.directed_presentation {
            self.render_position
        } else {
            self.render_position.or(self.position)
        }
    }

    pub(crate) fn display_energy(&self) -> Option<f32> {
        // Fully live: show the latest server snapshot energy directly. Damage
        // events remain presentation/diagnostic evidence and never mutate a
        // player selected only by reusable numeric id.
        self.energy
    }

    pub(crate) fn display_orientation(&self) -> f32 {
        if self.directed_presentation {
            return self.directed_render_orientation.unwrap_or(self.orientation);
        }
        let Some(start) = self.previous_orientation else {
            return self.orientation;
        };
        if self.orientation_interpolation_duration <= f32::EPSILON {
            return self.orientation;
        }

        let t = (self.orientation_interpolation_elapsed / self.orientation_interpolation_duration)
            .clamp(0.0, 1.0);
        interpolate_orientation(start, self.orientation, t)
    }

    pub(crate) fn clear_position(&mut self) {
        self.position = None;
        self.previous_position = None;
        self.render_position = None;
        self.interpolation_start_position = None;
        self.interpolation_target_position = None;
        self.interpolation_elapsed = 0.0;
        self.interpolation_duration = 0.0;
        self.interpolation_start_velocity = Vec2::ZERO;
        self.interpolation_sample_interval = 0.0;
        self.previous_orientation = None;
        self.orientation_interpolation_elapsed = 0.0;
        self.orientation_interpolation_duration = 0.0;
        self.directed_presentation = false;
        self.directed_render_orientation = None;
        self.directed_render_time = None;
        self.directed_firing_heading = None;
        self.directed_presentation_history.clear();
    }

    #[cfg(test)]
    pub(crate) fn record_directed_snapshot(
        &mut self,
        snapshot_id: u16,
        server_tick: u32,
        snap_reason: RemoteSnapReason,
    ) {
        self.record_directed_source_snapshot(
            snapshot_id,
            server_tick,
            crate::combat_timeline::PresentationSampleTime::from_tick(server_tick),
            snap_reason,
        );
    }

    pub(crate) fn record_directed_source_snapshot(
        &mut self,
        snapshot_id: u16,
        server_tick: u32,
        sample_time: crate::combat_timeline::PresentationSampleTime,
        snap_reason: RemoteSnapReason,
    ) {
        let (Some(position), Some(life_seq)) = (self.position, self.life_seq) else {
            return;
        };
        if snapshot_id == 0 || life_seq == 0 {
            return;
        }
        if let Some(previous) = self.directed_presentation_history.back() {
            let snapshot_distance = snapshot_id.wrapping_sub(previous.snapshot_id);
            if snapshot_distance == 0
                || snapshot_distance >= (1_u16 << 15)
                || crate::continuum::continuum_tick_diff(server_tick, previous.server_tick) < 0
            {
                return;
            }
        }
        if !self.directed_presentation {
            self.directed_presentation = true;
            self.render_position = None;
            self.directed_render_orientation = None;
            self.directed_render_time = None;
            self.directed_firing_heading = None;
        }
        self.directed_presentation_history
            .push_back(DirectedPresentationSample {
                snapshot_id,
                server_tick,
                sample_time,
                kinematics: crate::combat_timeline::PresentationKinematicSampleV1 {
                    x_q8: presentation_q8(position.x),
                    y_q8: presentation_q8(position.y),
                    velocity_x_q8: presentation_q8(self.velocity.x),
                    velocity_y_q8: presentation_q8(self.velocity.y),
                },
                orientation_u16: presentation_orientation_u16(self.orientation),
                snap_discontinuity: matches!(
                    snap_reason,
                    RemoteSnapReason::Distance | RemoteSnapReason::Flash
                ),
                motion_segment: self.snapshot_motion_segment,
            });
        while self.directed_presentation_history.len() > DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS {
            self.directed_presentation_history.pop_front();
        }
    }

    /// Called only for an exact fresh birth at the source actor's draw tick.
    /// Late/current-flight fallbacks must never rewind the present ship.
    pub(crate) fn present_firing_heading(
        &mut self,
        session_id: u64,
        life_seq: u32,
        tick: u32,
        orientation: f32,
    ) -> bool {
        if !self.directed_presentation
            || self.source_session_id != Some(session_id)
            || self.life_seq != Some(life_seq)
            || self.display_position().is_none()
            || !orientation.is_finite()
            || !(0.0..1.0).contains(&orientation)
        {
            return false;
        }
        let time = crate::combat_timeline::PresentationSampleTime::from_tick(tick);
        if self.directed_firing_heading.is_some_and(|(previous, _)| {
            time.quanta_since(previous)
                .is_none_or(|distance| distance <= 0)
        }) {
            return false;
        }
        self.directed_firing_heading = Some((time, orientation));
        self.directed_render_orientation = Some(orientation);
        true
    }

    pub(crate) fn update_directed_render_position(&mut self, render_tick: u32, dt: f32) {
        self.directed_presentation = true;
        let render_time = crate::combat_timeline::PresentationSampleTime::from_tick(render_tick);
        let mut lower = None;
        let mut upper = None;
        for sample in self.directed_presentation_history.iter().copied() {
            let Some(age) = render_time.quanta_since(sample.sample_time) else {
                continue;
            };
            if age >= 0
                && lower.is_none_or(|current: DirectedPresentationSample| {
                    render_time
                        .quanta_since(current.sample_time)
                        .is_some_and(|current_age| current_age > age)
                })
            {
                lower = Some(sample);
            }
            let lead = -age;
            if lead >= 0
                && upper.is_none_or(|current: DirectedPresentationSample| {
                    current
                        .sample_time
                        .quanta_since(render_time)
                        .is_some_and(|current_lead| current_lead > lead)
                })
            {
                upper = Some(sample);
            }
        }
        let Some(lower) = lower else {
            // A bounded scheduler recovery may temporarily leave the shared
            // cursor just before the oldest retained sample. Hold the last
            // proven presentation instead of making a live actor disappear;
            // the combat-view receipt remains held and therefore cannot
            // authorize a new weapon edge until the cursor catches up.
            return;
        };
        let (x_q8, y_q8, mut orientation, snap_orientation, displayed_time) = match upper {
            Some(upper) if upper.sample_time == lower.sample_time => (
                lower.kinematics.x_q8,
                lower.kinematics.y_q8,
                presentation_orientation(lower.orientation_u16),
                lower.snap_discontinuity,
                lower.sample_time,
            ),
            Some(upper)
                if upper.snap_discontinuity
                    || presentation_sample_distance_q8(lower, upper)
                        > crate::combat_timeline::OBSERVER_PRESENTATION_SNAP_DISTANCE_Q8 =>
            {
                (
                    upper.kinematics.x_q8,
                    upper.kinematics.y_q8,
                    presentation_orientation(upper.orientation_u16),
                    true,
                    upper.sample_time,
                )
            }
            Some(upper) if upper.motion_segment != lower.motion_segment => (
                // No source proves the path between two physical segments.
                // Draw the last lower sample until the cursor reaches the
                // first observed sample of the new segment, without blending
                // incompatible tangents or pulling future authority forward.
                lower.kinematics.x_q8,
                lower.kinematics.y_q8,
                presentation_orientation(lower.orientation_u16),
                false,
                lower.sample_time,
            ),
            Some(upper) => {
                let (x_q8, y_q8) = crate::combat_timeline::presentation_hermite_sample_time_q8(
                    lower.sample_time,
                    lower.kinematics,
                    upper.sample_time,
                    upper.kinematics,
                    render_time,
                )
                .expect("ordered distinct source samples");
                let span = upper
                    .sample_time
                    .quanta_since(lower.sample_time)
                    .expect("ordered samples") as f32;
                let elapsed = render_time
                    .quanta_since(lower.sample_time)
                    .expect("bracketed render time") as f32;
                (
                    x_q8,
                    y_q8,
                    interpolate_orientation(
                        presentation_orientation(lower.orientation_u16),
                        presentation_orientation(upper.orientation_u16),
                        (elapsed / span).clamp(0.0, 1.0),
                    ),
                    false,
                    render_time,
                )
            }
            None => (
                lower.kinematics.x_q8,
                lower.kinematics.y_q8,
                presentation_orientation(lower.orientation_u16),
                false,
                lower.sample_time,
            ),
        };
        if snap_orientation {
            self.directed_firing_heading = None;
        } else if let Some((time, firing_orientation)) = self.directed_firing_heading
            && render_time.quanta_since(time).is_some_and(|age| age >= 0)
        {
            if lower
                .sample_time
                .quanta_since(time)
                .is_some_and(|age| age > 0)
            {
                self.directed_firing_heading = None;
            } else {
                // The cue replaces only the older angular endpoint. Hermite
                // position/tangents and the authoritative heading are untouched.
                orientation = if let Some(upper) = upper
                    && upper.motion_segment == lower.motion_segment
                    && let Some(span) = upper
                        .sample_time
                        .quanta_since(time)
                        .filter(|span| *span > 0)
                {
                    let elapsed = render_time.quanta_since(time).unwrap_or(0) as f32;
                    interpolate_orientation(
                        firing_orientation,
                        presentation_orientation(upper.orientation_u16),
                        (elapsed / span as f32).clamp(0.0, 1.0),
                    )
                } else {
                    firing_orientation
                };
            }
        }
        self.render_position = Some(vec2(x_q8 as f32 / 256.0, y_q8 as f32 / 256.0));
        self.directed_render_time = Some(displayed_time);
        // Human headings arrive in 40 protocol bins and several bins can be
        // coalesced between render-grade snapshots. Positions must stay on
        // the exact server-directed combat-view cursor, but presenting that
        // quantization as one frame-sized heading jump makes an otherwise
        // smooth authoritative turn visibly stutter. Retain the established
        // gameplay-rate bound for ordinary orientation projection; authored
        // presentation discontinuities still snap position and heading as one
        // terminal boundary.
        self.directed_render_orientation = Some(if snap_orientation {
            orientation
        } else if let Some(current) = self.directed_render_orientation {
            advance_orientation_bounded(
                current,
                orientation,
                REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND
                    * dt.clamp(0.0, REMOTE_PLAYER_MAX_RENDER_FRAME_SECONDS),
            )
        } else {
            orientation
        });
    }

    pub(crate) fn take_flash_warp_effect(&mut self) -> Option<Vec2> {
        if self.togglables & STATUS_FLASH == 0 {
            return None;
        }

        self.togglables &= !STATUS_FLASH;
        self.previous_position
    }

    fn begin_render_interpolation(
        &mut self,
        previous_position: Option<Vec2>,
        position: Vec2,
        update: &PlayerPositionUpdate,
        duration: f32,
        sample_interval: f32,
    ) -> RemoteSnapReason {
        let current_render_position = self.render_position.or(previous_position);
        let snap_reason = if update.togglables & STATUS_FLASH != 0 {
            RemoteSnapReason::Flash
        } else if current_render_position.is_none() {
            RemoteSnapReason::Initial
        } else if current_render_position.is_some_and(|current| {
            current.distance(position) > REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE
        }) {
            RemoteSnapReason::Distance
        } else {
            RemoteSnapReason::None
        };

        if snap_reason != RemoteSnapReason::None {
            self.render_position = Some(position);
            self.interpolation_start_position = None;
            self.interpolation_target_position = None;
            self.interpolation_elapsed = 0.0;
            self.interpolation_duration = 0.0;
            self.interpolation_start_velocity = Vec2::ZERO;
            self.interpolation_sample_interval = 0.0;
            return snap_reason;
        }

        let target_unchanged = previous_position == Some(position);
        let active_same_target_interpolation = self.interpolation_target_position == Some(position)
            && self.interpolation_elapsed < self.interpolation_duration;
        if target_unchanged && active_same_target_interpolation {
            return RemoteSnapReason::None;
        }

        // `self.velocity` still holds the previous sample's velocity here
        // (apply_position_core updates it after this call), so it is the
        // correct incoming tangent for the segment we are starting.
        let bounded_start = current_render_position
            .map(|current| clamp_render_to_authoritative_hull(current, position));
        self.render_position = bounded_start;
        self.interpolation_start_position = bounded_start;
        self.interpolation_target_position = Some(position);
        self.interpolation_elapsed = 0.0;
        self.interpolation_duration = duration;
        self.interpolation_start_velocity = self.velocity;
        self.interpolation_sample_interval = sample_interval;
        RemoteSnapReason::None
    }

    fn begin_orientation_interpolation(
        &mut self,
        current_display_orientation: f32,
        next_orientation: f32,
        snap_reason: RemoteSnapReason,
        duration: f32,
    ) {
        if snap_reason != RemoteSnapReason::None || self.last_server_position_tick.is_none() {
            self.previous_orientation = None;
            self.orientation_interpolation_elapsed = 0.0;
            self.orientation_interpolation_duration = 0.0;
            return;
        }

        if orientation_distance(current_display_orientation, next_orientation) <= f32::EPSILON {
            self.previous_orientation = None;
            self.orientation_interpolation_elapsed = 0.0;
            self.orientation_interpolation_duration = 0.0;
            return;
        }

        self.previous_orientation = Some(current_display_orientation);
        self.orientation_interpolation_elapsed = 0.0;
        self.orientation_interpolation_duration = duration;
    }

    fn update_render_orientation(&mut self, dt: f32) {
        let Some(start) = self.previous_orientation else {
            return;
        };
        if self.orientation_interpolation_duration <= f32::EPSILON {
            self.previous_orientation = None;
            self.orientation_interpolation_elapsed = 0.0;
            return;
        }
        self.orientation_interpolation_elapsed = bounded_orientation_interpolation_elapsed(
            self.orientation_interpolation_elapsed,
            self.orientation_interpolation_duration,
            orientation_distance(start, self.orientation),
            dt,
        );
        if self.orientation_interpolation_elapsed >= self.orientation_interpolation_duration {
            self.previous_orientation = None;
        }
    }

    fn accepts_position_timestamp(&self, timestamp: u16, mask: u16) -> bool {
        let Some(last_timestamp) = self.last_server_position_tick else {
            return true;
        };
        let timestamp = timestamp & mask;
        let last_timestamp = last_timestamp & mask;
        if timestamp == last_timestamp {
            return false;
        }

        let modulus = mask as u32 + 1;
        let half_range = modulus / 2;
        let delta = (timestamp.wrapping_sub(last_timestamp) & mask) as u32;
        delta > 0 && delta < half_range
    }

    /// Real seconds between the previous accepted sample and this one,
    /// derived from the 100 Hz Continuum tick delta. Used both to scale
    /// Hermite tangents and to size the adaptive playback buffer. Must be
    /// called before `last_server_position_tick` is advanced for this frame.
    fn sample_interval_seconds(&self, timestamp: u16) -> f32 {
        let Some(previous_timestamp) = self.last_server_position_tick else {
            return REMOTE_PLAYER_INTERPOLATION_DEFAULT_SAMPLE_SECONDS;
        };
        let ticks = timestamp.wrapping_sub(previous_timestamp);
        if ticks == 0 || ticks > 1_000 {
            return REMOTE_PLAYER_INTERPOLATION_DEFAULT_SAMPLE_SECONDS;
        }
        (ticks as f32 / 100.0).clamp(
            REMOTE_PLAYER_INTERPOLATION_MIN_SAMPLE_SECONDS,
            REMOTE_PLAYER_INTERPOLATION_MAX_SAMPLE_SECONDS,
        )
    }

    /// Adaptive playback buffer: render each remote ship slightly behind
    /// live so the next snapshot has usually arrived before this segment
    /// finishes. The buffer margin tracks an EWMA of inter-sample jitter,
    /// so steady links stay tight (low latency) while lossy/jittery links
    /// widen automatically. Mutates the EWMA state; call once per sample.
    fn adaptive_interpolation_duration(&mut self, sample_interval: f32) -> f32 {
        if self.interp_mean_gap_seconds <= 0.0 {
            self.interp_mean_gap_seconds = sample_interval;
            self.interp_jitter_seconds = 0.0;
        } else {
            let deviation = (sample_interval - self.interp_mean_gap_seconds).abs();
            self.interp_mean_gap_seconds += REMOTE_PLAYER_INTERPOLATION_EWMA_ALPHA
                * (sample_interval - self.interp_mean_gap_seconds);
            self.interp_jitter_seconds +=
                REMOTE_PLAYER_INTERPOLATION_EWMA_ALPHA * (deviation - self.interp_jitter_seconds);
        }
        let margin = (REMOTE_PLAYER_INTERPOLATION_BASE_MARGIN_SECONDS
            + REMOTE_PLAYER_INTERPOLATION_JITTER_MARGIN_K * self.interp_jitter_seconds)
            .clamp(
                REMOTE_PLAYER_INTERPOLATION_MIN_MARGIN_SECONDS,
                REMOTE_PLAYER_INTERPOLATION_MAX_MARGIN_SECONDS,
            );
        (sample_interval + margin).clamp(
            REMOTE_PLAYER_INTERPOLATION_MIN_SECONDS,
            REMOTE_PLAYER_INTERPOLATION_MAX_SECONDS,
        )
    }
}

fn presentation_q8(value: f32) -> i32 {
    (value * 256.0)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

fn presentation_orientation_u16(orientation: f32) -> u16 {
    (orientation.rem_euclid(1.0) * 65_536.0).round() as u32 as u16
}

fn presentation_orientation(orientation: u16) -> f32 {
    // Recover exact wire headings before truncating to a sprite/gun bin.
    // A rounded u16 approximation can land just below its original boundary
    // (direction 13 becomes 0.32499695), incorrectly selecting the prior frame.
    // Other sub-bin codes retain their original precision and interpolation.
    let nearest_direction = ((u32::from(orientation) * 40 + 32_768) / 65_536) % 40;
    let canonical =
        crate::continuum_position::protocol_direction_to_orientation(nearest_direction as u8);
    if presentation_orientation_u16(canonical) == orientation {
        return canonical;
    }
    f32::from(orientation) / 65_536.0
}

fn presentation_sample_distance_q8(
    lower: DirectedPresentationSample,
    upper: DirectedPresentationSample,
) -> i32 {
    let dx = i64::from(upper.kinematics.x_q8) - i64::from(lower.kinematics.x_q8);
    let dy = i64::from(upper.kinematics.y_q8) - i64::from(lower.kinematics.y_q8);
    ((dx * dx + dy * dy) as f64).sqrt().round() as i32
}

/// Cubic Hermite spline point at parameter `t` in `[0, 1]` for endpoints
/// `p0`/`p1` with tangents `m0`/`m1` (already scaled to the segment's
/// parameter space). With zero tangents this is smoothstep; with tangents
/// equal to the chord it is exactly linear.
fn hermite(p0: Vec2, p1: Vec2, m0: Vec2, m1: Vec2, t: f32) -> Vec2 {
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11
}

/// Clamp a vector's magnitude to `max_len`. A non-positive `max_len`
/// collapses the vector to zero (used to kill tangents on a stationary
/// segment so a held ship does not wobble).
fn clamp_length(v: Vec2, max_len: f32) -> Vec2 {
    if max_len <= 0.0 {
        return Vec2::ZERO;
    }
    let len = v.length();
    if len > max_len {
        v * (max_len / len)
    } else {
        v
    }
}

fn bounded_render_position(
    current: Vec2,
    desired: Vec2,
    authoritative_target: Vec2,
    dt: f32,
    target_speed_px_per_second: f32,
) -> Vec2 {
    let delta = desired - current;
    let distance = delta.length();
    // A fixed per-frame ceiling turns into a lower world-space speed as the
    // browser frame rate falls. The former 5.5 px ceiling was only 165 px/s
    // at 30 FPS, so an authored 350 px/s Extreme ship could permanently outrun
    // its own visible proxy. That made a projectile visibly cross the proxy
    // while authority correctly tested a hull tens of pixels ahead. Permit
    // the target's measured gameplay speed plus a bounded, frame-rate-neutral
    // catch-up budget. Explicit Initial/Distance/Flash paths still own large
    // discontinuities.
    let frame_seconds = dt.clamp(0.0, REMOTE_PLAYER_MAX_RENDER_FRAME_SECONDS);
    let target_speed = if target_speed_px_per_second.is_finite() {
        target_speed_px_per_second.max(0.0)
    } else {
        0.0
    };
    let max_step = (target_speed + REMOTE_PLAYER_RENDER_CATCHUP_PX_PER_SECOND) * frame_seconds;
    let bounded = if max_step <= 0.0 {
        current
    } else if distance > max_step {
        current + delta * (max_step / distance)
    } else {
        desired
    };
    // Interpolation is presentation-only: it must never move the visible
    // combat hull so far behind the latest accepted snapshot that a shot can
    // visibly cross the ship while server authority correctly sees a miss.
    // This can happen when an actor returns to AOI less than the large-warp
    // snap threshold away and immediately resumes Extreme-speed motion. Keep
    // the smooth path inside one small combat-scale leash; explicit
    // Initial/Distance/Flash discontinuities still own real teleports.
    if frame_seconds <= 0.0 {
        return bounded;
    }
    clamp_render_to_authoritative_hull(bounded, authoritative_target)
}

fn clamp_render_to_authoritative_hull(render: Vec2, authoritative: Vec2) -> Vec2 {
    let authority_delta = render - authoritative;
    let authority_distance = authority_delta.length();
    if authority_distance > REMOTE_PLAYER_PRESENTATION_AUTHORITY_MAX_ERROR_PX {
        authoritative
            + authority_delta
                * (REMOTE_PLAYER_PRESENTATION_AUTHORITY_MAX_ERROR_PX / authority_distance)
    } else {
        render
    }
}

fn interpolate_orientation(start: f32, target: f32, t: f32) -> f32 {
    let delta = (target - start + 0.5).rem_euclid(1.0) - 0.5;
    (start + delta * t).rem_euclid(1.0)
}

fn advance_orientation_bounded(current: f32, desired: f32, maximum_delta: f32) -> f32 {
    let delta = (desired - current + 0.5).rem_euclid(1.0) - 0.5;
    let maximum_delta = maximum_delta.max(0.0);
    if maximum_delta == 0.0 {
        return current.rem_euclid(1.0);
    }
    // Once within the existing step budget, retain the exact proven target.
    // Adding a rounded shortest-angle delta can settle one f32 ULP below it
    // forever, leaving a truncating sprite in the preceding protocol bin.
    if delta.abs() <= maximum_delta {
        return desired.rem_euclid(1.0);
    }
    (current + delta.clamp(-maximum_delta, maximum_delta)).rem_euclid(1.0)
}

fn orientation_distance(a: f32, b: f32) -> f32 {
    ((b - a + 0.5).rem_euclid(1.0) - 0.5).abs()
}

fn bounded_orientation_interpolation_elapsed(
    elapsed: f32,
    duration: f32,
    orientation_distance: f32,
    dt: f32,
) -> f32 {
    if duration <= f32::EPSILON || orientation_distance <= f32::EPSILON {
        return duration.max(0.0);
    }
    let current_t = (elapsed / duration).clamp(0.0, 1.0);
    let time_t = dt.max(0.0) / duration;
    let rotation_t =
        REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND * dt.max(0.0) / orientation_distance;
    (current_t + time_t.min(rotation_t)).clamp(0.0, 1.0) * duration
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NetworkPositionApply {
    Applied {
        flash_warp: Option<Vec2>,
        snap_reason: RemoteSnapReason,
    },
    Stale,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RemoteSnapReason {
    #[default]
    None,
    Initial,
    Distance,
    Flash,
}

impl RemoteSnapReason {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Initial => 1,
            Self::Distance => 2,
            Self::Flash => 3,
        }
    }
}

// Lower/upper bounds on the per-segment interpolation duration (the
// playback buffer). The floor is lower than the historical fixed 120 ms so
// steady links render remote ships closer to live; the ceiling caps how far
// behind a very jittery link falls. Phase A (AOI low-latency) lowers the
// floor to 60 ms: near-tier ships now arrive at ~60 Hz, so a smaller buffer
// still has several updates of headroom. The buffer is adaptive, so mid-tier
// ships (still ~30 Hz, larger sample interval) automatically settle on a
// larger buffer than near-tier ships without a separate constant.
pub(crate) const REMOTE_PLAYER_INTERPOLATION_MIN_SECONDS: f32 = 0.06;
pub(crate) const REMOTE_PLAYER_INTERPOLATION_MAX_SECONDS: f32 = 0.28;
// Extrapolation stays disabled: remote ships hold their last server target
// rather than overshoot. The combat E2E asserts zero extrapolated frames.
pub(crate) const REMOTE_PLAYER_EXTRAPOLATION_LIMIT_SECONDS: f32 = 0.0;
// Adaptive playback-buffer tuning. `margin = base + k * jitter`, clamped,
// added on top of the measured inter-sample interval.
const REMOTE_PLAYER_INTERPOLATION_DEFAULT_SAMPLE_SECONDS: f32 = 0.033;
const REMOTE_PLAYER_INTERPOLATION_MIN_SAMPLE_SECONDS: f32 = 0.01;
const REMOTE_PLAYER_INTERPOLATION_MAX_SAMPLE_SECONDS: f32 = 0.5;
const REMOTE_PLAYER_INTERPOLATION_BASE_MARGIN_SECONDS: f32 = 0.04;
const REMOTE_PLAYER_INTERPOLATION_MIN_MARGIN_SECONDS: f32 = 0.04;
const REMOTE_PLAYER_INTERPOLATION_MAX_MARGIN_SECONDS: f32 = 0.14;
const REMOTE_PLAYER_INTERPOLATION_JITTER_MARGIN_K: f32 = 2.0;
const REMOTE_PLAYER_INTERPOLATION_EWMA_ALPHA: f32 = 0.2;
// Hermite tangents are clamped to this multiple of the chord length to bound
// overshoot/wobble when reported velocity disagrees with the position delta.
// [REVIEW M12] Keep this at <= 1.0: with tangents bounded by the chord, the
// worst-case excursion past a segment endpoint (velocity/chord reversal,
// e.g. a wall bounce) stays small and is pinned by the
// `hermite_reversal_excursion_is_bounded` test below; 1.5 permitted ~8%
// overshoot past the endpoint that the zero-extrapolation E2E assertion
// does not cover.
const REMOTE_PLAYER_INTERPOLATION_TANGENT_CHORD_FACTOR: f32 = 1.0;
// Presentation may catch up at most this much faster than the currently
// authored remote velocity. Expressing the bound per second keeps the same
// contract at 30, 60, and 144 Hz; large warps still use the explicit
// Initial/Distance/Flash snap paths above.
const REMOTE_PLAYER_RENDER_CATCHUP_PX_PER_SECOND: f32 = 180.0;
/// Maximum visible distance from the latest accepted authoritative remote
/// hull during ordinary interpolation. This stays below one 16px tile and is
/// deliberately much tighter than the 512px discontinuity threshold.
const REMOTE_PLAYER_PRESENTATION_AUTHORITY_MAX_ERROR_PX: f32 = 12.0;
// A scheduler hitch must not be presented as one giant correction. Thirty FPS
// is the slowest acceptance cadence whose authored motion we promise to track
// in one frame; larger elapsed values converge over later render calls.
const REMOTE_PLAYER_MAX_RENDER_FRAME_SECONDS: f32 = 1.0 / 30.0;
// The fastest Chaos ship turns at 0.725 rotations/s. Bound interpolation to
// that gameplay envelope (plus a small rounding margin) so coalesced direction
// samples cannot create a faster visible one-frame rotation correction.
const REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND: f32 = 0.75;
pub(crate) const REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE: f32 = 512.0;
const POSITION_TIMESTAMP_FULL_MASK: u16 = u16::MAX;
const POSITION_TIMESTAMP_BATCHED_MASK: u16 = 0x03ff;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkChatEntry {
    pub(crate) chat_type: u8,
    pub(crate) sound: u8,
    pub(crate) sender_id: u16,
    pub(crate) sender_name: Option<String>,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetworkPowerball {
    pub(crate) ball_id: u8,
    pub(crate) position: Vec2,
    pub(crate) velocity: Vec2,
    pub(crate) owner_id: Option<u16>,
    pub(crate) timestamp: u32,
    pub(crate) carried: bool,
    pub(crate) pickup_unlock_tick: Option<u32>,
    pub(crate) killer_id: Option<u16>,
    pub(crate) killer_pickup_unlock_tick: Option<u32>,
    /// True when the objective extension explicitly supplied lock metadata,
    /// including an explicit unlocked `None`. False is legacy provenance.
    pub(crate) authoritative_unlock_provenance: bool,
    pub(crate) age_since_update: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkPowerballRadarKind {
    Loose,
    TeamCarrier,
    EnemyCarrier,
}

impl NetworkPowerball {
    pub(crate) fn from_update(update: PowerballPositionUpdate) -> Option<Self> {
        let owner_id = update.owner();
        if update.timestamp == 0 && owner_id.is_none() {
            return None;
        }

        Some(Self {
            ball_id: update.ball_id,
            position: vec2(update.x as f32, update.y as f32),
            velocity: vec2(
                crate::continuum_position::protocol_velocity_pixels_per_second(update.velocity_x),
                crate::continuum_position::protocol_velocity_pixels_per_second(update.velocity_y),
            ),
            owner_id,
            timestamp: update.timestamp,
            carried: update.timestamp == 0 && owner_id.is_some(),
            pickup_unlock_tick: None,
            killer_id: None,
            killer_pickup_unlock_tick: None,
            authoritative_unlock_provenance: false,
            age_since_update: 0.0,
        })
    }

    pub(crate) fn accepts_update(&self, update: &PowerballPositionUpdate) -> bool {
        update.timestamp == 0
            || self.carried
            || continuum_tick_diff(update.timestamp, self.timestamp) > 0
    }

    pub(crate) fn update(&mut self, dt: f32) {
        self.age_since_update += dt.max(0.0);
        if !self.carried {
            self.position += self.velocity * dt;
        }
    }

    pub(crate) fn phased_passing(&self, pass_delay_seconds: f32) -> bool {
        !self.carried
            && self.timestamp != 0
            && pass_delay_seconds > 0.0
            && self.age_since_update < pass_delay_seconds
    }

    pub(crate) fn objective_pickup_locked(&self, current_server_tick: Option<u32>) -> bool {
        self.pickup_unlock_tick.is_some_and(|unlock_tick| {
            current_server_tick
                .is_none_or(|tick| !crate::continuum::continuum_tick_reached(tick, unlock_tick))
        })
    }

    pub(crate) fn objective_pickup_locked_for(
        &self,
        player_id: u16,
        current_server_tick: Option<u32>,
    ) -> bool {
        let unlock_tick = if self.killer_id == Some(player_id) {
            self.killer_pickup_unlock_tick
        } else {
            self.pickup_unlock_tick
        };
        unlock_tick.is_some_and(|unlock_tick| {
            current_server_tick
                .is_none_or(|tick| !crate::continuum::continuum_tick_reached(tick, unlock_tick))
        })
    }

    pub(crate) fn objective_phased_passing(
        &self,
        pass_delay_seconds: f32,
        current_server_tick: Option<u32>,
    ) -> bool {
        if self.authoritative_unlock_provenance {
            !self.carried && self.objective_pickup_locked(current_server_tick)
        } else {
            self.phased_passing(pass_delay_seconds)
        }
    }

    pub(crate) fn objective_pickup_blocked_for_last_thrower(&self, player_id: u16) -> bool {
        !self.carried
            && self.owner_id == Some(player_id)
            && self.velocity.length_squared() > f32::EPSILON
    }

    pub(crate) fn radar_kind(
        &self,
        world: &NetworkWorld,
        viewer_frequency: u16,
    ) -> NetworkPowerballRadarKind {
        if self.carried {
            if let Some(owner_id) = self.owner_id {
                if let Some(carrier) = world.players.get(&owner_id) {
                    return if carrier.frequency == viewer_frequency {
                        NetworkPowerballRadarKind::TeamCarrier
                    } else {
                        NetworkPowerballRadarKind::EnemyCarrier
                    };
                }
            }
        }

        NetworkPowerballRadarKind::Loose
    }

    pub(crate) fn display_position(&self, world: &NetworkWorld, tuning: &ArenaTuning) -> Vec2 {
        if self.carried {
            if let Some(owner_id) = self.owner_id {
                if let Some(player) = world.players.get(&owner_id) {
                    if player.ship < 8 {
                        if let Some(position) = player.display_position() {
                            let (x, y) =
                                crate::continuum_position::canonical_carried_powerball_position(
                                    position.x,
                                    position.y,
                                    player.display_orientation(),
                                    tuning.ship(player.ship as usize).radius_pixels,
                                );
                            return vec2(x, y);
                        }
                    }
                }
            }
        }

        self.position
    }
}

#[cfg(test)]
mod interpolation_tests {
    use super::*;

    fn runtime_player() -> NetworkPlayer {
        NetworkPlayer::from_roster(&PlayerRosterEntry {
            ship: 0,
            audio: 0,
            name: "source".to_string(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id: 9,
            frequency: 7,
            wins: 0,
            losses: 0,
            attach_parent: INVALID_PLAYER_ID,
            flags: 0,
            koth: 0,
        })
    }

    fn position_update(timestamp: u16, x: u16, velocity_x: i16) -> PlayerPositionUpdate {
        PlayerPositionUpdate {
            player_id: 9,
            direction: 10,
            timestamp,
            x,
            y: 1_000,
            velocity_x,
            velocity_y: 0,
            togglables: 0,
            ping: 0,
            bounty: 0,
            weapon: 0,
            checksum: None,
            energy: Some(1_000),
            s2c_latency: None,
            flag_timer: None,
            items: None,
        }
    }

    #[test]
    fn source_session_binding_requires_exact_life_and_clears_across_wrap() {
        let mut player = runtime_player();
        player.set_life_seq(Some(u32::MAX));
        assert!(!player.bind_source_session(0, u32::MAX));
        assert!(!player.bind_source_session(u64::MAX, 1));
        assert!(player.bind_source_session(u64::MAX, u32::MAX));
        assert_eq!(player.source_session_id, Some(u64::MAX));

        player.set_life_seq(Some(1));
        assert_eq!(player.source_session_id, None);
        assert!(!player.bind_source_session(u64::MAX, u32::MAX));
        assert!(player.bind_source_session(1, 1));
        assert_eq!(player.source_session_id, Some(1));
    }

    #[test]
    fn impulse_keeps_observed_past_motion_without_interpolating_across_segments() {
        let mut player = runtime_player();
        player.set_life_seq(Some(1));
        let apply = |player: &mut NetworkPlayer, id, tick, x, velocity, segment| {
            assert!(!player.accept_snapshot_motion_segment(Some(segment), Some(1)));
            let reason = match player.apply_position(&position_update(tick, x, velocity)) {
                NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
                NetworkPositionApply::Stale => panic!("ordered source must apply"),
            };
            player.record_directed_snapshot(id, u32::from(tick), reason);
        };
        // The observer cursor trails authority by seven ticks. An impulse at
        // 110 must not discard the already observed 100..108 source interval.
        apply(&mut player, 1, 100, 100, 1_000, 1);
        apply(&mut player, 2, 108, 108, 1_000, 1);
        player.update_directed_render_position(102, 0.01);
        assert_eq!(player.display_position(), Some(vec2(102.0, 1_000.0)));
        apply(&mut player, 3, 110, 110, -5_000, 2);
        player.update_directed_render_position(103, 0.01);
        assert_eq!(
            player.display_position(),
            Some(vec2(103.0, 1_000.0)),
            "receiving an impulse must not freeze the delayed pre-impulse cursor"
        );
        // There is no source sample for tick 109. Never invent a Hermite
        // chord/tangent across the velocity discontinuity or extrapolate it.
        player.update_directed_render_position(109, 0.01);
        assert_eq!(player.display_position(), Some(vec2(108.0, 1_000.0)));
        player.update_directed_render_position(110, 0.01);
        assert_eq!(player.display_position(), Some(vec2(110.0, 1_000.0)));
        apply(&mut player, 4, 112, 100, -5_000, 2);
        player.update_directed_render_position(111, 0.01);
        assert_eq!(player.display_position(), Some(vec2(105.0, 1_000.0)));
        assert_eq!(player.presentation_snap_count, 0);
        assert!(!player.accept_snapshot_motion_segment(Some(4), Some(1)));
        assert!(player.directed_presentation_history.len() >= 4);
        assert!(player.accept_snapshot_motion_segment(Some(5), Some(3)));
        assert!(
            player.directed_presentation_history.is_empty(),
            "lost relocation still clears predecessor geometry"
        );
        player.set_life_seq(Some(2));
        assert!(player.directed_presentation_history.is_empty());
    }

    #[test]
    fn directed_presentation_uses_one_tick_no_extrapolation_and_exact_life() {
        let mut player = runtime_player();
        player.set_life_seq(Some(1));

        let first = position_update(100, 100, 0);
        let first_reason = match player.apply_position(&first) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("first sample must apply"),
        };
        player.record_directed_snapshot(1, 100, first_reason);
        player.update_directed_render_position(99, 1.0 / 60.0);
        assert_eq!(
            player.display_position(),
            None,
            "the latest authority cannot leak ahead of the directed render tick"
        );

        let second = position_update(110, 200, 0);
        let second_reason = match player.apply_position(&second) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("second sample must apply"),
        };
        player.record_directed_snapshot(2, 110, second_reason);
        player.update_directed_render_position(105, 1.0 / 60.0);
        assert_eq!(player.display_position(), Some(vec2(150.0, 1_000.0)));

        player.update_directed_render_position(150, 1.0 / 60.0);
        assert_eq!(
            player.display_position(),
            Some(vec2(200.0, 1_000.0)),
            "a missing upper sample must hold rather than extrapolate"
        );

        player.set_life_seq(Some(2));
        assert!(player.directed_presentation_history.is_empty());
        assert_eq!(
            player.display_position(),
            None,
            "a successor life cannot inherit predecessor presentation"
        );
    }

    #[test]
    fn directed_presentation_history_is_bounded_and_snaps_discontinuity() {
        let mut player = runtime_player();
        player.set_life_seq(Some(1));
        for offset in 0..DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS as u16 + 8 {
            let tick = 100 + u32::from(offset);
            let update = position_update(tick as u16, 100 + offset, 0);
            let reason = match player.apply_position(&update) {
                NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
                NetworkPositionApply::Stale => panic!("ordered sample must apply"),
            };
            player.record_directed_snapshot(offset + 1, tick, reason);
        }
        assert_eq!(
            player.directed_presentation_history.len(),
            DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS
        );

        let flash_tick = 100 + DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS as u16 + 9;
        let flash = PlayerPositionUpdate {
            togglables: STATUS_FLASH,
            ..position_update(flash_tick, 900, 0)
        };
        let reason = match player.apply_position(&flash) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("flash sample must apply"),
        };
        player.record_directed_snapshot(flash_tick, u32::from(flash_tick), reason);
        player.update_directed_render_position(u32::from(flash_tick - 1), 1.0 / 60.0);
        assert_eq!(
            player.display_position(),
            Some(vec2(900.0, 1_000.0)),
            "the upper discontinuity owns the entire crossing interval"
        );
        player.update_directed_render_position(100, 1.0 / 60.0);
        assert_eq!(
            player.display_position(),
            Some(vec2(900.0, 1_000.0)),
            "bounded cursor recovery holds the last proven sprite before retained history"
        );
    }

    #[test]
    fn resumed_frame_projects_newly_applied_samples_instead_of_the_predecessor_hold() {
        let mut player = runtime_player();
        player.set_life_seq(Some(4));
        let first = position_update(36_944, 7_499, 3_100);
        let reason = match player.apply_position(&first) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("first sample must apply"),
        };
        player.record_directed_snapshot(21_567, 36_944, reason);
        player.update_directed_render_position(36_950, 1.0 / 60.0);
        assert_eq!(player.display_position().unwrap().x, 7_499.0);

        // The retained Firefox recovery decoded this successor before the
        // frame, but Game installed it after the old projection point. A
        // snapshot does not itself redraw a ship: the frame must project
        // after public authority is applied, on the same server-owned cursor.
        let successor = position_update(36_956, 7_536, 3_100);
        let reason = match player.apply_position(&successor) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("successor must apply"),
        };
        player.record_directed_snapshot(21_574, 36_956, reason);
        assert_eq!(player.display_position().unwrap().x, 7_499.0);
        player.update_directed_render_position(36_950, 1.0 / 60.0);
        let resumed = player.display_position().unwrap();
        assert!((resumed.x - 7_517.5).abs() < 0.01);
        player.update_directed_render_position(36_952, 1.0 / 60.0);
        assert!((player.display_position().unwrap().x - resumed.x).abs() < 7.0);
        assert_eq!(player.life_seq, Some(4));
    }

    #[test]
    fn directed_history_covers_the_complete_bounded_cursor_recovery_window() {
        let mut player = runtime_player();
        player.set_life_seq(Some(1));
        for offset in 0..DIRECTED_PRESENTATION_HISTORY_SNAPSHOTS as u16 {
            let tick = 136 + u32::from(offset);
            let update = position_update(tick as u16, 1_000 + offset, 100);
            let reason = match player.apply_position(&update) {
                NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
                NetworkPositionApply::Stale => panic!("ordered sample must apply"),
            };
            player.record_directed_snapshot(offset + 1, tick, reason);
        }

        player.update_directed_render_position(136, 1.0 / 60.0);
        assert_eq!(
            player.display_position(),
            Some(vec2(1_000.0, 1_000.0)),
            "a cursor exactly 64 ticks behind the latest sample must still have a drawable lower bound"
        );
        player.update_directed_render_position(139, 1.0 / 60.0);
        assert!(
            player
                .display_position()
                .is_some_and(|position| position.x > 1_000.0),
            "bounded recovery must advance the sprite instead of holding until newer retained history"
        );
    }

    #[test]
    fn directed_orientation_codec_retains_all_protocol_headings_and_wraps() {
        for direction in 0..40 {
            let turns = direction as f32 / 40.0;
            let packed = presentation_orientation_u16(turns);
            let decoded = presentation_orientation(packed);
            assert!(orientation_distance(turns, decoded) <= 0.5 / 65_536.0);
            assert_eq!((decoded * 40.0).round() as u8 % 40, direction);
            assert_eq!(
                crate::continuum_position::protocol_direction(decoded),
                direction,
                "the actual truncating sprite/gun bin must survive the codec"
            );
        }
        assert_eq!(presentation_orientation_u16(1.0), 0);
        assert_eq!(presentation_orientation_u16(-0.25), 49_152);
        assert_eq!(presentation_orientation(16_384), 0.25);
    }

    #[test]
    fn directed_orientation_codec_preserves_every_other_sub_bin_code() {
        let canonical_codes = (0..40)
            .map(|direction| {
                presentation_orientation_u16(
                    crate::continuum_position::protocol_direction_to_orientation(direction),
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut previous = 0.0;
        for code in 0..=u16::MAX {
            let decoded = presentation_orientation(code);
            let raw = f32::from(code) / 65_536.0;
            assert!(decoded >= previous, "decoding cannot reverse angular order");
            assert!((decoded - raw).abs() <= 0.5 / 65_536.0);
            if !canonical_codes.contains(&code) {
                assert_eq!(decoded.to_bits(), raw.to_bits());
            }
            previous = decoded;
        }
    }

    #[test]
    fn firing_heading_anchor_does_not_chase_pre_shot_history_or_change_position() {
        let mut player = runtime_player();
        player.set_life_seq(Some(1));
        player.position = Some(vec2(100.0, 100.0));
        player.orientation = 0.1;
        player.record_directed_snapshot(1, 100, RemoteSnapReason::None);
        player.position = Some(vec2(110.0, 100.0));
        player.orientation = 0.3;
        player.record_directed_snapshot(2, 110, RemoteSnapReason::None);
        player.update_directed_render_position(105, 1.0 / 60.0);
        assert!(player.bind_source_session(99, 1));
        let mut control = player.clone();
        assert!(player.present_firing_heading(99, 1, 105, 0.2));
        assert_eq!(player.display_orientation(), 0.2);
        player.update_directed_render_position(106, 1.0 / 60.0);
        control.update_directed_render_position(106, 1.0 / 60.0);
        assert_eq!(player.display_position(), control.display_position());
        assert!(
            player.display_orientation() > 0.2,
            "a firing cue must replace the older angular endpoint, not chase it backwards"
        );
        assert!(
            orientation_distance(player.display_orientation(), 0.2)
                <= REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND / 60.0 + 1e-5
        );
        let advanced = player.display_orientation();
        assert!(!player.present_firing_heading(99, 1, 105, 0.9));
        assert!(!player.present_firing_heading(99, 1, 104, 0.9));
        assert_eq!(player.display_orientation(), advanced);
        assert_eq!(player.orientation, 0.3);
        assert!(!player.present_firing_heading(98, 1, 106, 0.4));
        assert!(!player.present_firing_heading(99, 2, 106, 0.4));
        assert!(!player.present_firing_heading(99, 1, 106, f32::NAN));
        player.clear_position();
        assert!(player.directed_firing_heading.is_none());
        assert!(!player.present_firing_heading(99, 1, 106, 0.4));
    }

    #[test]
    fn firing_heading_anchor_clears_at_source_and_motion_boundaries() {
        for boundary in ["life", "session", "flash", "distance"] {
            let mut player = runtime_player();
            player.set_life_seq(Some(1));
            player.position = Some(vec2(100.0, 100.0));
            player.orientation = 0.1;
            player.record_directed_snapshot(1, 100, RemoteSnapReason::None);
            player.update_directed_render_position(100, 1.0 / 60.0);
            assert!(player.bind_source_session(99, 1));
            assert!(player.present_firing_heading(99, 1, 100, 0.2));
            match boundary {
                "life" => player.set_life_seq(Some(2)),
                "session" => {
                    assert!(player.bind_source_session(100, 1));
                }
                _ => {
                    player.orientation = 0.75;
                    player.record_directed_snapshot(
                        2,
                        110,
                        if boundary == "flash" {
                            RemoteSnapReason::Flash
                        } else {
                            RemoteSnapReason::Distance
                        },
                    );
                    player.update_directed_render_position(110, 1.0 / 60.0);
                    assert_eq!(player.display_orientation(), 0.75);
                }
            }
            assert!(player.directed_firing_heading.is_none(), "{boundary}");
        }
    }

    #[test]
    fn directed_rotation_settles_in_the_actual_protocol_bin_after_bounded_recovery() {
        for direction in 0..40 {
            let mut player = runtime_player();
            player.set_life_seq(Some(1));
            let mut update = position_update(100, 100, 0);
            update.direction = direction;
            let reason = match player.apply_position(&update) {
                NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
                NetworkPositionApply::Stale => panic!("first heading must apply"),
            };
            player.record_directed_snapshot(1, 100, reason);
            let expected = crate::continuum_position::protocol_direction_to_orientation(direction);
            let paused = f32::from_bits(expected.to_bits().saturating_sub(1));
            player.directed_render_orientation = Some(paused);
            player.update_directed_render_position(100, 0.0);
            assert_eq!(
                player.display_orientation().to_bits(),
                paused.to_bits(),
                "zero elapsed time must not advance even a sub-ULP correction"
            );
            // Same sub-bin phase as the retained direction13 birth, including
            // recovery from a prior quantized heading. Exercise actual history
            // selection, the decoder and the bounded rotation step together.
            player.directed_render_orientation = Some((expected - 0.024_743_93).rem_euclid(1.0));
            for _ in 0..4 {
                player.update_directed_render_position(100, 1.0 / 60.0);
            }
            assert_eq!(
                crate::continuum_position::protocol_direction(player.display_orientation()),
                direction,
                "the submitted sprite must settle in the firing bin"
            );
        }
    }

    #[test]
    fn directed_orientation_smooths_coalesced_headings_and_snaps_discontinuities() {
        let mut player = runtime_player();
        player.set_life_seq(Some(1));

        let mut first = position_update(100, 100, 0);
        first.direction = 12;
        let first_reason = match player.apply_position(&first) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("first heading must apply"),
        };
        player.record_directed_snapshot(1, 100, first_reason);
        player.update_directed_render_position(100, 1.0 / 60.0);
        let initial = player.display_orientation();
        assert!((initial - 0.3).abs() < 1e-4);

        let mut coalesced = position_update(101, 101, 0);
        coalesced.direction = 10;
        let coalesced_reason = match player.apply_position(&coalesced) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("coalesced heading must apply"),
        };
        player.record_directed_snapshot(2, 101, coalesced_reason);
        player.update_directed_render_position(101, 1.0 / 60.0);
        let one_frame = player.display_orientation();
        assert!(
            orientation_distance(initial, one_frame)
                <= REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND / 60.0 + 1e-5,
            "an ordinary directed sample must retain the frame-rate rotation bound"
        );

        player.update_directed_render_position(101, 1.0);
        let after_hitch = player.display_orientation();
        assert!(
            orientation_distance(one_frame, after_hitch)
                <= REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND
                    * REMOTE_PLAYER_MAX_RENDER_FRAME_SECONDS
                    + 1e-5,
            "a scheduler hitch must not become one giant visible rotation correction"
        );

        let mut flash = position_update(102, 900, 0);
        flash.direction = 20;
        flash.togglables = STATUS_FLASH;
        let flash_reason = match player.apply_position(&flash) {
            NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
            NetworkPositionApply::Stale => panic!("flash heading must apply"),
        };
        player.record_directed_snapshot(3, 102, flash_reason);
        player.update_directed_render_position(102, 1.0 / 60.0);
        assert!(
            orientation_distance(player.display_orientation(), 0.5) < 1e-4,
            "an authored discontinuity must snap position and orientation together"
        );
    }

    fn max_overshoot_past_endpoints(p0: Vec2, p1: Vec2, m0: Vec2, m1: Vec2) -> f32 {
        let lo = p0.x.min(p1.x);
        let hi = p0.x.max(p1.x);
        let mut worst: f32 = 0.0;
        for step in 0..=1_000 {
            let t = step as f32 / 1_000.0;
            let x = hermite(p0, p1, m0, m1, t).x;
            worst = worst.max(lo - x).max(x - hi);
        }
        worst
    }

    #[test]
    fn hermite_reversal_excursion_is_bounded() {
        // Wall-bounce shape: the ship reported moving +x, but the chord goes
        // -x and the new velocity is -x. With tangents clamped to <= chord
        // (REMOTE_PLAYER_INTERPOLATION_TANGENT_CHORD_FACTOR = 1.0) the curve
        // may bulge slightly past the start point but must stay tightly
        // bounded — this is the regression guard for raising the factor.
        let p0 = vec2(100.0, 0.0);
        let p1 = vec2(80.0, 0.0);
        let chord = p0.distance(p1);
        let max_tangent = REMOTE_PLAYER_INTERPOLATION_TANGENT_CHORD_FACTOR * chord;
        let m0 = clamp_length(vec2(200.0, 0.0) * 0.1, max_tangent); // away from chord
        let m1 = clamp_length(vec2(-200.0, 0.0) * 0.1, max_tangent); // along chord
        let overshoot = max_overshoot_past_endpoints(p0, p1, m0, m1);
        assert!(
            overshoot <= 0.10 * chord,
            "reversal overshoot must stay within 10% of the chord, got {overshoot} px on a {chord} px chord"
        );
    }

    #[test]
    fn hermite_consistent_straight_motion_is_exactly_linear() {
        // Tangents equal to the chord reduce cubic Hermite to the linear
        // chord — straight constant-velocity coasting must not curve.
        let p0 = vec2(0.0, 0.0);
        let p1 = vec2(20.0, 0.0);
        let m = vec2(20.0, 0.0);
        for step in 0..=100 {
            let t = step as f32 / 100.0;
            let p = hermite(p0, p1, m, m, t);
            assert!(
                (p.x - 20.0 * t).abs() < 1e-3 && p.y.abs() < 1e-6,
                "expected linear chord at t={t}, got {p:?}"
            );
        }
    }

    #[test]
    fn source_timed_browser_and_server_projection_ignore_batched_emission_cadence() {
        use crate::authority_identity::{
            LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
        };
        use crate::combat_timeline::{
            ActorMotionHistory, ActorMotionSampleV1, ActorMotionSourceKind,
            ObserverPresentationFrameV1, ObserverPresentationHistory, ObserverPresentationSampleV1,
            PresentationGeometryResolver, PresentationSampleTime,
        };
        let runtime = |id| {
            PlayerRuntimeIdentity::new(
                PlayerSessionIdentity::new(id, PlayerSessionId::new(u64::from(id))),
                LifeIdentity::new(id, 3),
            )
        };
        let actor = runtime(9);
        let observer = runtime(1);
        let mut motion = ActorMotionHistory::new();
        let segment = motion.begin_segment(actor, 7);
        let baseline = ActorMotionSampleV1::from_state(
            segment,
            100,
            100.0,
            1_000.0,
            120.0,
            0.0,
            0.25,
            14.0,
            0,
            ActorMotionSourceKind::AcceptedHumanWitness,
        );
        motion.begin_accepted_samples(100, baseline).unwrap();
        motion
            .append_accepted_samples(&[
                (
                    101,
                    ActorMotionSampleV1 {
                        server_tick: 105,
                        x_q8: 102 * 256,
                        ..baseline
                    },
                ),
                (
                    102,
                    ActorMotionSampleV1 {
                        server_tick: 105,
                        x_q8: 104 * 256,
                        ..baseline
                    },
                ),
            ])
            .unwrap();
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, 7);
        let mut player = runtime_player();
        player.set_life_seq(Some(3));
        for (id, emitted, x, source_tick, third) in [
            (1, 100, 100, 100, 0),
            (2, 105, 104, 103, 1),
            (3, 108, 104, 103, 1),
        ] {
            let source_time = PresentationSampleTime::from_parts(source_tick, third).unwrap();
            let reason = match player.apply_position(&position_update(emitted as u16, x, 1_200)) {
                NetworkPositionApply::Applied { snap_reason, .. } => snap_reason,
                NetworkPositionApply::Stale => panic!("ordered frame must apply"),
            };
            player.record_directed_source_snapshot(id, emitted, source_time, reason);
            presentation.observe_emitted(ObserverPresentationFrameV1 {
                observer,
                arena_stream_id: 7,
                snapshot_id: id,
                server_tick: emitted,
                actors: vec![ObserverPresentationSampleV1 {
                    actor,
                    motion_segment: segment,
                    sample_time: source_time,
                    x_q8: i32::from(x) * 256,
                    y_q8: 1_000 * 256,
                    velocity_x_q8: 120 * 256,
                    velocity_y_q8: 0,
                    orientation_u16: 16_384,
                    snap_discontinuity: false,
                }],
            });
        }
        for tick in 100..=106 {
            player.update_directed_render_position(tick, 1.0 / 60.0);
            let actual = player.display_position().unwrap();
            let expected = (100.0 + (tick - 100) as f32 * 1.2).min(104.0);
            assert!(
                (actual.x - expected).abs() <= 1.0 / 256.0,
                "source motion must stay at 120 px/s then hold: {tick} {actual:?}"
            );
            let server =
                PresentationGeometryResolver::resolve(&presentation, segment, tick, 3, &motion)
                    .unwrap();
            assert_eq!(server.x_q8, (actual.x * 256.0).round() as i32);
            assert_eq!(server.y_q8, (actual.y * 256.0).round() as i32);
        }
    }

    #[test]
    fn server_presentation_resolver_matches_browser_hermite_policy_fixture() {
        use crate::authority_identity::{
            LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
        };
        use crate::combat_timeline::{
            ActorMotionHistory, ActorMotionSourceKind, ObserverPresentationFrameV1,
            ObserverPresentationHistory, ObserverPresentationSampleV1,
            PresentationGeometryResolver,
        };

        fn runtime(player_id: u16, session_id: u64, life_seq: u32) -> PlayerRuntimeIdentity {
            PlayerRuntimeIdentity::new(
                PlayerSessionIdentity::new(player_id, PlayerSessionId::new(session_id)),
                LifeIdentity::new(player_id, life_seq),
            )
        }

        assert_eq!(
            crate::combat_timeline::OBSERVER_PRESENTATION_SNAP_DISTANCE_Q8 as f32 / 256.0,
            REMOTE_PLAYER_INTERPOLATION_SNAP_DISTANCE
        );
        assert_eq!(
            crate::combat_timeline::PRESENTATION_AUTHORITY_MAX_ERROR_Q8 as f32 / 256.0,
            REMOTE_PLAYER_PRESENTATION_AUTHORITY_MAX_ERROR_PX
        );
        assert_eq!(REMOTE_PLAYER_INTERPOLATION_TANGENT_CHORD_FACTOR, 1.0);
        assert_eq!(REMOTE_PLAYER_EXTRAPOLATION_LIMIT_SECONDS, 0.0);

        let p0 = vec2(100.0, 1_000.0);
        let p1 = vec2(120.0, 1_010.0);
        let v0 = vec2(300.0, 0.0);
        let v1 = vec2(0.0, 200.0);
        let sample_interval = 0.1;
        let chord = p0.distance(p1);
        let browser = hermite(
            p0,
            p1,
            clamp_length(v0 * sample_interval, chord),
            clamp_length(v1 * sample_interval, chord),
            0.5,
        );

        let observer = runtime(1, 10, 1);
        let actor = runtime(9, 20, 3);
        let arena_stream_id = 7;
        let mut motion = ActorMotionHistory::new();
        motion.append_state(
            actor,
            arena_stream_id,
            100,
            p0.x,
            p0.y,
            v0.x,
            v0.y,
            0.0,
            14.0,
            0,
            ActorMotionSourceKind::AcceptedHumanWitness,
        );
        let segment = motion.current_segment().expect("motion segment");
        motion.append_state(
            actor,
            arena_stream_id,
            105,
            browser.x,
            browser.y,
            0.0,
            0.0,
            0.0,
            14.0,
            0,
            ActorMotionSourceKind::AcceptedHumanWitness,
        );
        motion.append_state(
            actor,
            arena_stream_id,
            110,
            p1.x,
            p1.y,
            v1.x,
            v1.y,
            0.0,
            14.0,
            0,
            ActorMotionSourceKind::AcceptedHumanWitness,
        );

        let sample = |position: Vec2, velocity: Vec2, tick| ObserverPresentationSampleV1 {
            sample_time: crate::combat_timeline::PresentationSampleTime::from_tick(tick),
            actor,
            motion_segment: segment,
            x_q8: (position.x * 256.0).round() as i32,
            y_q8: (position.y * 256.0).round() as i32,
            velocity_x_q8: (velocity.x * 256.0).round() as i32,
            velocity_y_q8: (velocity.y * 256.0).round() as i32,
            orientation_u16: 0,
            snap_discontinuity: false,
        };
        let mut presentation = ObserverPresentationHistory::new();
        presentation.begin_scope(observer, arena_stream_id);
        for (snapshot_id, server_tick, position, velocity) in [(10, 100, p0, v0), (11, 110, p1, v1)]
        {
            presentation.observe_emitted(ObserverPresentationFrameV1 {
                observer,
                arena_stream_id,
                snapshot_id,
                server_tick,
                actors: vec![sample(position, velocity, server_tick)],
            });
        }

        let server =
            PresentationGeometryResolver::resolve(&presentation, segment, 105, 11, &motion)
                .expect("shared server projection");
        let browser_q8 = (
            (browser.x * 256.0).round() as i32,
            (browser.y * 256.0).round() as i32,
        );
        assert!(
            (server.x_q8 - browser_q8.0).abs() <= 1 && (server.y_q8 - browser_q8.1).abs() <= 1,
            "shared projection ({}, {}) diverged from browser fixture {browser_q8:?}",
            server.x_q8,
            server.y_q8
        );
        assert!(server.authority_error_q8 <= 1);
    }

    #[test]
    fn clamp_length_bounds_magnitude_and_zeroes_on_empty_chord() {
        assert_eq!(clamp_length(vec2(10.0, 0.0), 0.0), Vec2::ZERO);
        assert_eq!(clamp_length(vec2(3.0, 4.0), 10.0), vec2(3.0, 4.0));
        let clamped = clamp_length(vec2(30.0, 40.0), 5.0);
        assert!((clamped.length() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn interpolation_correction_stays_inside_one_normal_render_step() {
        let current = vec2(100.0, 100.0);
        let dt = 1.0 / 60.0;
        let bounded =
            bounded_render_position(current, vec2(100.0, 110.0), vec2(100.0, 110.0), dt, 0.0);
        let expected = REMOTE_PLAYER_RENDER_CATCHUP_PX_PER_SECOND * dt;
        assert!((bounded.distance(current) - expected).abs() < 1e-4);
        assert_eq!(
            bounded_render_position(current, vec2(101.0, 102.0), vec2(101.0, 102.0), dt, 0.0,),
            vec2(101.0, 102.0)
        );
        assert_eq!(
            bounded_render_position(current, vec2(140.0, 140.0), vec2(140.0, 140.0), 0.0, 350.0,),
            current,
            "a zero-duration render frame must not become an unbounded presentation snap"
        );
    }

    #[test]
    fn ordinary_interpolation_cannot_leave_visible_hull_far_from_authority() {
        let current = vec2(100.0, 100.0);
        let authoritative = vec2(250.0, 100.0);
        let displayed = bounded_render_position(
            current,
            vec2(110.0, 100.0),
            authoritative,
            1.0 / 60.0,
            350.0,
        );
        assert!(
            displayed.distance(authoritative)
                <= REMOTE_PLAYER_PRESENTATION_AUTHORITY_MAX_ERROR_PX + 1e-4,
            "displayed {displayed:?} escaped authoritative hull {authoritative:?}"
        );
        assert!(
            displayed.x < authoritative.x,
            "ordinary smoothing should remain bounded, not overshoot"
        );
    }

    #[test]
    fn accepted_snapshot_immediately_bounds_visible_hull_before_next_render() {
        let mut player = runtime_player();
        assert!(matches!(
            player.apply_position(&position_update(1, 1_000, 3_500)),
            NetworkPositionApply::Applied { .. }
        ));
        player.update_render_position(1.0 / 60.0);
        assert!(matches!(
            player.apply_position(&position_update(2, 1_150, 3_500)),
            NetworkPositionApply::Applied {
                snap_reason: RemoteSnapReason::None,
                ..
            }
        ));
        assert!(
            player
                .display_position()
                .expect("accepted snapshot keeps a presentation position")
                .distance(vec2(1_150.0, 1_000.0))
                <= REMOTE_PLAYER_PRESENTATION_AUTHORITY_MAX_ERROR_PX + 1e-4,
            "snapshot admission must not wait for a later browser frame to restore the combat hull"
        );
    }

    #[test]
    fn extreme_speed_proxy_cannot_fall_permanently_behind_at_thirty_fps() {
        let mut player = runtime_player();
        assert!(matches!(
            player.apply_position(&position_update(1, 1_000, 3_500)),
            NetworkPositionApply::Applied { .. }
        ));
        player.update_render_position(1.0 / 30.0);

        for sample in 1..=120_u16 {
            let timestamp = 1 + sample * 3;
            let x = (1_000.0 + sample as f32 * 10.5).round() as u16;
            assert!(matches!(
                player.apply_position(&position_update(timestamp, x, 3_500)),
                NetworkPositionApply::Applied {
                    snap_reason: RemoteSnapReason::None,
                    ..
                }
            ));
            player.update_render_position(1.0 / 30.0);
        }

        let authoritative = player.position.expect("authoritative position");
        let rendered = player.display_position().expect("render position");
        assert!(
            authoritative.distance(rendered) <= 24.0,
            "350 px/s authored motion must stay visually hittable at 30 FPS: authoritative={authoritative:?} rendered={rendered:?}"
        );
        assert_eq!(player.presentation_snap_count, 0);
    }

    #[test]
    fn orientation_correction_stays_inside_the_fastest_ship_turn_rate() {
        let duration = 0.06;
        let distance = 0.10;
        let dt = 0.04;
        let elapsed = bounded_orientation_interpolation_elapsed(0.0, duration, distance, dt);
        let displayed_delta = distance * elapsed / duration;
        assert!((displayed_delta - REMOTE_PLAYER_MAX_RENDER_ROTATION_PER_SECOND * dt).abs() < 1e-5);
        assert_eq!(
            bounded_orientation_interpolation_elapsed(elapsed, duration, distance, 0.4),
            duration
        );
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetworkFlag {
    pub(crate) flag_id: u16,
    pub(crate) owner: u16,
    pub(crate) position: Vec2,
    pub(crate) dropped: bool,
    pub(crate) turf: bool,
    pub(crate) carried_by: Option<u16>,
}

impl NetworkFlag {
    pub(crate) fn from_position(position: FlagPosition) -> Self {
        Self {
            flag_id: position.flag_id,
            owner: position.owner,
            position: vec2(position.x as f32, position.y as f32),
            dropped: true,
            turf: false,
            carried_by: None,
        }
    }
}
