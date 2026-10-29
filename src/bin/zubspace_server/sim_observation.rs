//! Phase 1c movement authority: server-side simulation of the next
//! player position from the latest accepted input state.
//!
//! For each accepted position relay, the server runs one deterministic
//! movement step from the previously seeded state using the most
//! recently observed `PlayerInput` (Phase 1b). It compares the simulated
//! next position against the position the client reported and logs
//! deviation when requested. The server keeps this running sim as a
//! validation/correction model, but snapshots publish the server-accepted
//! client report. That avoids mixing two nearby motion sources on remote
//! clients while still giving the server a deterministic authority model
//! to compare against.
//!
//! Movement and collision geometry call the same engine-independent core as
//! the client. This module only adapts server arena settings, doors, bricks,
//! input records, and observation bookkeeping around that core.

use crate::arena_settings::{
    arena_flagger_speed_adjustment, arena_flagger_thrust_adjustment,
    arena_repel_speed_pixels_per_second, arena_rocket_speed_pixels_per_second,
    arena_rocket_thrust_pixels_per_second_squared, arena_ship_afterburner_energy_per_second,
    arena_ship_initial_recharge_per_second, arena_ship_initial_rotation_per_second,
    arena_ship_initial_thrust_pixels_per_second_squared,
    arena_ship_initial_top_speed_pixels_per_second, arena_ship_max_speed_pixels_per_second,
    arena_ship_max_thrust_pixels_per_second_squared, arena_ship_radius_pixels,
    arena_ship_turret_speed_penalty, arena_ship_turret_thrust_penalty, arena_ship_wormhole_gravity,
    arena_ship_wormhole_top_speed,
};
use crate::shared_player_movement::{
    CollisionAxis, CollisionIntegration, MovementVector, PlayerMovementEnvironment,
    PlayerMovementInput, PlayerMovementState, PlayerMovementTuning, WormholeTileGrid,
    integrate_player_collision, movement_axis_overlaps_blocked_tile,
    sample_wormhole_gravity_in_grid, step_player_movement,
};
use crate::state::{
    CanonicalPositionWitness, ClientHandle, ClientInputObservation, ServerSimMovement,
};
use crate::zone::{MAP_TILES, TILE_SIZE_PX, ZoneMap};
use tracing::debug;

/// Pixel deviation above which we log. ~16 px = 1 tile. After Phase
/// 1c-3a (wall collision) a 1016-sample headed-Chrome soak with QWASD
/// movement showed: 99.7 % of comparisons under 5 px, max 5.9 px
/// (a wall bounce), zero outliers above 6 px. The remaining floor
/// comes from u16 position quantization plus float rounding in axis
/// sweep. Rocket, exact reference-game wormhole gravity, and hostile-repel
/// special-state paths now feed this shared model. Set this to 0 to see every observation; 16
/// hides the float-jitter floor without missing real physics
/// divergence.
const DEVIATION_LOG_THRESHOLD_PX: f32 = 16.0;

/// Skip unsequenced fixture comparison when the gap between observed
/// positions exceeds this. ~200 ms = 20 ticks; anything beyond is
/// either a network stall or a player rejoining and the
/// extrapolation isn't a fair comparison either way. The first soak
/// at 0.5 s produced a 540 px "outlier" that was really a 0.5 s gap
/// in the position relay during page load.
const SKIP_OBSERVATION_BEYOND_DT_SECONDS: f32 = 0.2;

/// Browser movement reports are sampled at this cadence. The authoritative
/// accept path continues to use server ticks for every gameplay decision, but
/// the private parity observer must replay the interval represented by the
/// accepted position-sequence gap rather than quantizing a 60 Hz browser frame
/// to the server clock's 10 ms protocol ticks.
const OBSERVED_POSITION_INTERVAL_SECONDS: f32 = 1.0 / 60.0;
/// One client render batch remains bounded to eight logical steps. Transport
/// loss can omit several position tails across multiple batches, however, so
/// the server replays as far as its wider, exactly reconstructed input history
/// can prove. It never extrapolates beyond that finite history.
const MAX_OBSERVED_POSITION_SEQUENCE_STEPS: u16 =
    crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16;

#[cfg(test)]
fn position_input_history_complete(sender: &ClientHandle, current_sequence: u16) -> bool {
    let (first_sequence, steps) = match sender.last_position_sequence {
        Some(previous) => {
            let steps = current_sequence.wrapping_sub(previous);
            if steps == 0 || steps > MAX_OBSERVED_POSITION_SEQUENCE_STEPS {
                return false;
            }
            (previous.wrapping_add(1), steps)
        }
        None => (current_sequence, 1),
    };
    (0..steps).all(|offset| {
        let sequence = first_sequence.wrapping_add(offset);
        sender
            .recent_input_records
            .iter()
            .any(|record| record.sequence == sequence)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PositionInputWitnessError {
    Missing,
    DirectionOutOfRange,
    DirectionMismatch,
    InvalidDirectionHistory,
}

impl PositionInputWitnessError {
    pub(crate) const fn security_reason(self) -> &'static str {
        match self {
            Self::Missing => "missing_input_record",
            Self::DirectionOutOfRange => "direction_out_of_range",
            Self::DirectionMismatch => "input_direction_mismatch",
            Self::InvalidDirectionHistory => "invalid_direction_history",
        }
    }
}

/// Debug-only evidence for a terminal missing-witness rejection. Do not call
/// this for ordinary input-recovery deferral. The retained sequence list is
/// bounded by the existing input ring; it contains no packet or credential data
/// and cannot mutate admission state.
pub(crate) fn trace_missing_position_input_witness(
    sender: &ClientHandle,
    player_id: u16,
    sequence: u16,
    direction: u8,
    weapon: u16,
    server_tick: u32,
    stage: &'static str,
) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    let retained_sequences = sender
        .recent_input_records
        .iter()
        .take(crate::state::MAX_RECENT_INPUT_OBSERVATIONS)
        .map(|record| record.sequence)
        .collect::<Vec<_>>();
    debug!(
        player_id,
        life_seq = sender.life_seq,
        server_tick,
        stage,
        position_sequence = sequence,
        snapshot_direction = direction,
        reported_weapon = weapon,
        last_position_sequence = ?sender.last_position_sequence,
        latest_input_sequence = ?sender.last_input_seq,
        last_position_server_tick = ?sender.last_position_server_tick,
        canonical_anchor = ?sender.canonical_movement_witness.map(|witness| {
            (witness.life_seq, witness.sequence)
        }),
        retained_sequences = ?retained_sequences,
        "terminal position input witness missing"
    );
}

/// True when the input record for `sequence` is a hole strictly inside the
/// retained input window rather than aged out of its front.
///
/// A captured replay is always older than everything the server still retains,
/// so a sequence that lies between the oldest and newest retained input cannot
/// be a replay. The only way that record is absent while strictly newer ones
/// are present is that the transport dropped it, which leaves the matching
/// stale position frame an inert movement mirror rather than attack evidence.
/// Sequences at or behind the oldest retained input stay indistinguishable from
/// a replay and keep their explicit security rejection.
pub(crate) fn input_record_lost_inside_retained_window(
    sender: &ClientHandle,
    sequence: u16,
) -> bool {
    let Some(newest) = sender.last_input_seq else {
        return false;
    };
    let mut oldest_distance = None::<u16>;
    for record in sender.recent_input_records.iter() {
        // Anything strictly ahead of the watermark cannot bound the window's
        // back edge in wrap-safe order.
        if newest.wrapping_sub(record.sequence) >= (1 << 15) {
            continue;
        }
        let distance = newest.wrapping_sub(record.sequence);
        if oldest_distance.is_none_or(|best| distance > best) {
            oldest_distance = Some(distance);
        }
    }
    // A single retained record cannot bound a window: there is no interior to
    // place a hole in, so the omission cannot be proven to be transport loss.
    oldest_distance.is_some_and(|span| span > 0 && newest.wrapping_sub(sequence) < span)
}

/// Find the earliest absent input in a bounded forward replay interval. Held
/// prefix records must not make a later, retransmittable hole look exhausted.
/// This diagnostic/recovery query never admits movement or invents inputs.
pub(crate) fn first_missing_position_input_sequence(
    sender: &ClientHandle,
    current_sequence: u16,
) -> Option<u16> {
    let previous = sender.last_position_sequence?;
    let steps = current_sequence.wrapping_sub(previous);
    if steps == 0 || steps >= (1 << 15) {
        return None;
    }
    // Retention is deliberately finite. Locate the first hole in O(records),
    // rather than repeatedly searching the deque for each replayed input.
    const {
        assert!(
            MAX_OBSERVED_POSITION_SEQUENCE_STEPS > 0
                && MAX_OBSERVED_POSITION_SEQUENCE_STEPS <= u64::BITS as u16
        )
    };
    let checked = steps.min(MAX_OBSERVED_POSITION_SEQUENCE_STEPS);
    let mut present = 0_u64;
    for record in &sender.recent_input_records {
        let offset = record.sequence.wrapping_sub(previous);
        if offset > 0 && offset <= checked {
            present |= 1_u64 << (offset - 1);
        }
    }
    let required = u64::MAX >> (u64::BITS - u32::from(checked));
    let missing = !present & required;
    if missing != 0 {
        Some(previous.wrapping_add(missing.trailing_zeros() as u16 + 1))
    } else if steps > checked {
        // All retained slots are present. A longer interval still requires
        // another input; no claim of reconstructing beyond the existing cap.
        Some(previous.wrapping_add(checked + 1))
    } else {
        None
    }
}

/// Bind one position (and any weapon mirrored by it) to the exact sequenced
/// input record that produced it. Forward gaps are accepted only when every
/// record is present and all exact40-bin headings have a non-empty common
/// orientation interval under the shared rotation rules. Recovery queries
/// above never change these proof requirements.
pub(crate) fn validate_position_input_witness(
    sender: &ClientHandle,
    current_sequence: u16,
    snapshot_direction: u8,
    accepted_server_tick: u32,
) -> Result<ClientInputObservation, PositionInputWitnessError> {
    validate_position_input_witness_with_additions(
        sender,
        current_sequence,
        snapshot_direction,
        accepted_server_tick,
        &[],
    )
}

/// Prospective form used by an embedded input bundle before that bundle is
/// recorded. This keeps a biased/impossible embedded history atomic: the
/// position and its witness are both rejected without altering client state.
pub(crate) fn validate_position_input_witness_with_additions(
    sender: &ClientHandle,
    current_sequence: u16,
    snapshot_direction: u8,
    accepted_server_tick: u32,
    additions: &[(u16, ClientInputObservation)],
) -> Result<ClientInputObservation, PositionInputWitnessError> {
    if snapshot_direction >= 40 {
        return Err(PositionInputWitnessError::DirectionOutOfRange);
    }
    let find_input = |sequence| {
        additions
            .iter()
            .rev()
            .find(|(candidate, _)| *candidate == sequence)
            .map(|(_, input)| *input)
            .or_else(|| {
                sender
                    .recent_input_records
                    .iter()
                    .rev()
                    .find(|record| record.sequence == sequence)
                    .map(|record| record.input)
            })
    };
    let exact = find_input(current_sequence).ok_or(PositionInputWitnessError::Missing)?;
    if exact.direction >= 40 {
        return Err(PositionInputWitnessError::DirectionOutOfRange);
    }
    if exact.direction != snapshot_direction {
        return Err(PositionInputWitnessError::DirectionMismatch);
    }

    let Some(previous_sequence) = sender.last_position_sequence else {
        return Ok(exact);
    };
    let steps = current_sequence.wrapping_sub(previous_sequence);
    // Reordered positions do not define a forward replay interval, but a stale
    // weapon mirror must still carry its own exact, matching input witness.
    if steps == 0 || steps >= (1 << 15) {
        return Ok(exact);
    }
    if steps > MAX_OBSERVED_POSITION_SEQUENCE_STEPS {
        return Err(PositionInputWitnessError::Missing);
    }
    let inputs = (1..=steps)
        .map(|offset| find_input(previous_sequence.wrapping_add(offset)))
        .collect::<Option<Vec<_>>>()
        .ok_or(PositionInputWitnessError::Missing)?;
    if inputs.iter().any(|input| input.direction >= 40) {
        return Err(PositionInputWitnessError::DirectionOutOfRange);
    }

    let canonical_anchor = sender.canonical_movement_witness.filter(|witness| {
        witness.sequence == previous_sequence && witness.life_seq == sender.life_seq
    });
    let Some(witness) = canonical_anchor else {
        return Err(PositionInputWitnessError::Missing);
    };
    let Some(anchor_tick) = sender.last_position_server_tick else {
        return Err(PositionInputWitnessError::Missing);
    };
    let positive_server_ticks =
        crate::continuum::continuum_tick_diff(accepted_server_tick, anchor_tick).max(0) as u32;
    let step_ticks = (0..inputs.len())
        .map(|index| {
            let offset =
                positive_server_ticks.saturating_mul(index as u32 + 1) / inputs.len().max(1) as u32;
            crate::continuum::continuum_tick_add(anchor_tick, offset)
        })
        .collect::<Vec<_>>();
    let history_matches = |upgrades: PlayerMovementUpgrades,
                           earlier: Option<EarlierMovementTuning>| {
        let current_tuning = movement_tuning_from_ship(sender.player.ship, upgrades);
        let earlier_tuning = earlier.map(|earlier| {
            (
                earlier,
                movement_tuning_from_ship(sender.player.ship, earlier.upgrades),
            )
        });
        let rotation_steps = step_ticks
            .iter()
            .enumerate()
            .map(|(index, &step_tick)| {
                let sequence = previous_sequence.wrapping_add(index as u16 + 1);
                let tuning = earlier_tuning
                    .filter(|(earlier, _)| earlier.covers(sequence))
                    .map_or(current_tuning, |(_, earlier_tuning)| earlier_tuning);
                let engine_shutdown =
                    sender.server_engine_shutdown_active_for_sequence(step_tick, sequence);
                input_rotation_step(
                    if engine_shutdown {
                        tuning.shutdown_rotation_per_second
                    } else {
                        tuning.rotation_per_second
                    },
                    OBSERVED_POSITION_INTERVAL_SECONDS,
                )
            })
            .collect::<Vec<_>>();
        exact_orientation_history_matches(
            witness.state.orientation,
            witness.direction,
            &inputs,
            &rotation_steps,
        )
    };
    // Input generated before the browser learned of a damage prize change
    // turns at the replaced rate (`DamageMovementGrace`), up to the frame the
    // change reached it.
    let current = movement_upgrades_for_client(sender);
    let matches = history_matches(current, None)
        || damage_movement_grace_switch_points(sender, previous_sequence, current_sequence)
            .into_iter()
            .any(|earlier| history_matches(current, Some(earlier)));
    if !matches {
        return Err(PositionInputWitnessError::InvalidDirectionHistory);
    }
    Ok(exact)
}

/// Every way the gap after `previous_sequence` may have run at the tuning a
/// damage prize change replaced: all of it, then each later switch to the
/// current tuning, last switch first. Empty outside the grace window. A gap of
/// at most `MAX_OBSERVED_POSITION_SEQUENCE_STEPS` bounds the candidates.
pub(crate) fn damage_movement_grace_switch_points(
    sender: &ClientHandle,
    previous_sequence: u16,
    current_sequence: u16,
) -> Vec<EarlierMovementTuning> {
    let steps = current_sequence.wrapping_sub(previous_sequence);
    if steps == 0 || steps > MAX_OBSERVED_POSITION_SEQUENCE_STEPS {
        return Vec::new();
    }
    (1..=steps)
        .rev()
        .map(|offset| previous_sequence.wrapping_add(offset))
        .filter_map(|last_sequence| {
            sender
                .damage_movement_grace_tuning(last_sequence)
                .map(|tuning| EarlierMovementTuning {
                    upgrades: movement_upgrades_from_tuning(tuning),
                    last_sequence,
                })
        })
        .collect()
}

fn exact_orientation_history_matches(
    initial_orientation: f32,
    initial_direction: u8,
    inputs: &[ClientInputObservation],
    rotation_steps: &[f32],
) -> bool {
    if initial_direction >= 40
        || inputs.len() != rotation_steps.len()
        || crate::continuum_position::protocol_direction(initial_orientation) != initial_direction
    {
        return false;
    }
    let mut orientation = initial_orientation;
    for (input, rotation_step) in inputs.iter().zip(rotation_steps) {
        orientation = crate::shared_player_movement::rotate_orientation_step(
            orientation,
            input.bits & (1 << 2) != 0,
            input.bits & (1 << 3) != 0,
            *rotation_step,
        );
        if crate::continuum_position::protocol_direction(orientation) != input.direction {
            return false;
        }
    }
    true
}

/// Result of one observation step. `deviation_px`/`resynced` are
/// diagnostic-only. `snap_back`, when set, carries the complete exact
/// collision-validated movement state. The accept path publishes that same
/// raw-f32 state to authority and to the corrected owner, rather than mixing
/// an integer-only position override with the old velocity/orientation tail.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SimObservationResult {
    pub(crate) deviation_px: Option<u16>,
    pub(crate) resynced: bool,
    pub(crate) snap_back: Option<ServerSimMovement>,
}

/// Pure result of replaying every authenticated 60 Hz input between two
/// accepted position tails. Callers may inspect it during admission, but must
/// append `witnesses` only after every other position check succeeds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CanonicalPositionReplay {
    pub(crate) witnesses: Vec<CanonicalPositionWitness>,
    pub(crate) bounced: bool,
    /// An authenticated dynamic-door close relocated this branch to spawn.
    pub(crate) relocated: bool,
    /// An authenticated shared-core movement step entered a wormhole and
    /// applied its exact terminal spawn/vector transformation.
    pub(crate) wormhole_relocated: bool,
    safe_zone_weapon_stopped: bool,
    /// A browser advances doors from authenticated wall-clock authority before
    /// its fixed-step batch. Under bounded input loss, the rolling records can
    /// still resolve to an earlier occurrence of the same one-byte door seed
    /// even though the receipt-time challenge stream has crossed one or more
    /// complete phases. Retain every exact, receipt-bounded relocation timing
    /// as a separate replay branch; admission still requires one branch to
    /// reproduce every raw-f32 and legacy wire field.
    receipt_door_relocation_branches: Vec<CanonicalPositionReplayBranch>,
}

#[derive(Clone, Debug, PartialEq)]
struct CanonicalPositionReplayBranch {
    witnesses: Vec<CanonicalPositionWitness>,
    bounced: bool,
    relocated: bool,
    wormhole_relocated: bool,
    safe_zone_weapon_stopped: bool,
}

impl CanonicalPositionReplay {
    fn branch_matches_report(
        witnesses: &[CanonicalPositionWitness],
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> bool {
        witnesses.last().is_some_and(|witness| {
            witness.wire_x == x
                && witness.wire_y == y
                && witness.direction == direction
                && witness.wire_velocity_x == velocity_x
                && witness.wire_velocity_y == velocity_y
        })
    }

    fn branch_matches_exact_report(
        witnesses: &[CanonicalPositionWitness],
        canonical_state: crate::continuum_position::ClientPositionCanonicalState,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> bool {
        witnesses.last().is_some_and(|witness| {
            let (exact_x, exact_y, exact_velocity_x, exact_velocity_y, exact_orientation) =
                canonical_state.values();
            Self::branch_matches_report(witnesses, direction, x, y, velocity_x, velocity_y)
                && witness.state.position_x.to_bits() == exact_x.to_bits()
                && witness.state.position_y.to_bits() == exact_y.to_bits()
                && witness.state.velocity_x.to_bits() == exact_velocity_x.to_bits()
                && witness.state.velocity_y.to_bits() == exact_velocity_y.to_bits()
                && witness.state.orientation.to_bits() == exact_orientation.to_bits()
        })
    }

    #[cfg(test)]
    pub(crate) fn matching_witnesses(
        &self,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> Option<&[CanonicalPositionWitness]> {
        Self::branch_matches_report(&self.witnesses, direction, x, y, velocity_x, velocity_y)
            .then_some(self.witnesses.as_slice())
    }

    pub(crate) fn matching_exact_branch_witnesses(
        &self,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> Vec<&[CanonicalPositionWitness]> {
        let mut matching = Self::branch_matches_exact_report(
            &self.witnesses,
            canonical_state,
            direction,
            x,
            y,
            velocity_x,
            velocity_y,
        )
        .then_some(self.witnesses.as_slice())
        .into_iter()
        .collect::<Vec<_>>();
        matching.extend(
            self.receipt_door_relocation_branches
                .iter()
                .filter(|branch| {
                    Self::branch_matches_exact_report(
                        &branch.witnesses,
                        canonical_state,
                        direction,
                        x,
                        y,
                        velocity_x,
                        velocity_y,
                    )
                })
                .map(|branch| branch.witnesses.as_slice()),
        );
        matching
    }

    pub(crate) fn matching_exact_relocated_branch_witnesses(
        &self,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> Option<&[CanonicalPositionWitness]> {
        if self.relocated
            && Self::branch_matches_exact_report(
                &self.witnesses,
                canonical_state,
                direction,
                x,
                y,
                velocity_x,
                velocity_y,
            )
        {
            return Some(self.witnesses.as_slice());
        }
        self.receipt_door_relocation_branches
            .iter()
            .find(|branch| {
                branch.relocated
                    && Self::branch_matches_exact_report(
                        &branch.witnesses,
                        canonical_state,
                        direction,
                        x,
                        y,
                        velocity_x,
                        velocity_y,
                    )
            })
            .map(|branch| branch.witnesses.as_slice())
    }

    pub(crate) fn matching_exact_wormhole_branch_witnesses(
        &self,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> Option<&[CanonicalPositionWitness]> {
        if self.wormhole_relocated
            && Self::branch_matches_exact_report(
                &self.witnesses,
                canonical_state,
                direction,
                x,
                y,
                velocity_x,
                velocity_y,
            )
        {
            return Some(self.witnesses.as_slice());
        }
        self.receipt_door_relocation_branches
            .iter()
            .find(|branch| {
                branch.wormhole_relocated
                    && Self::branch_matches_exact_report(
                        &branch.witnesses,
                        canonical_state,
                        direction,
                        x,
                        y,
                        velocity_x,
                        velocity_y,
                    )
            })
            .map(|branch| branch.witnesses.as_slice())
    }

    pub(crate) fn any_bounced(&self) -> bool {
        self.bounced
            || self
                .receipt_door_relocation_branches
                .iter()
                .any(|branch| branch.bounced)
    }

    pub(crate) fn any_relocated(&self) -> bool {
        self.relocated
            || self.wormhole_relocated
            || self
                .receipt_door_relocation_branches
                .iter()
                .any(|branch| branch.relocated || branch.wormhole_relocated)
    }

    /// A large velocity drop is legal only when the exact matching replay
    /// consumed the explicit safe-zone stop bit at server-owned safe geometry.
    pub(crate) fn proves_exact_f32_safe_zone_weapon_stop_report(
        &self,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> bool {
        (self.safe_zone_weapon_stopped
            && Self::branch_matches_exact_report(
                &self.witnesses,
                canonical_state,
                direction,
                x,
                y,
                velocity_x,
                velocity_y,
            ))
            || self.receipt_door_relocation_branches.iter().any(|branch| {
                branch.safe_zone_weapon_stopped
                    && Self::branch_matches_exact_report(
                        &branch.witnesses,
                        canonical_state,
                        direction,
                        x,
                        y,
                        velocity_x,
                        velocity_y,
                    )
            })
    }

    /// Apply the exact post-movement bomb recoil used by the browser before it
    /// publishes a weapon-bearing position. Only the tail record carries the
    /// one-shot action; earlier gap witnesses remain pre-action movement
    /// states and continue to be valid delayed-weapon provenance.
    #[cfg(test)]
    pub(crate) fn apply_bomb_recoil_to_tail(&mut self, recoil_speed: f32) -> bool {
        fn apply(witnesses: &mut [CanonicalPositionWitness], recoil_speed: f32) -> bool {
            let Some(witness) = witnesses.last_mut() else {
                return false;
            };
            let (heading_x, heading_y) =
                crate::continuum_position::canonical_discrete_heading_components(
                    witness.state.orientation,
                );
            witness.state.velocity_x -= heading_x * recoil_speed.max(0.0);
            witness.state.velocity_y -= heading_y * recoil_speed.max(0.0);
            witness.wire_velocity_x =
                crate::continuum_position::protocol_velocity_component(witness.state.velocity_x);
            witness.wire_velocity_y =
                crate::continuum_position::protocol_velocity_component(witness.state.velocity_y);
            true
        }

        apply(&mut self.witnesses, recoil_speed)
    }

    /// Exact Continuum wire quantization compatibility. This is deliberately
    /// not a distance grace: the reconstructed float state must encode to the
    /// candidate position, direction, and velocity vector.
    #[cfg(test)]
    pub(crate) fn final_matches_report(
        &self,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> bool {
        self.matching_witnesses(direction, x, y, velocity_x, velocity_y)
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn bounced_tail_witnesses(&self) -> Vec<CanonicalPositionWitness> {
        self.bounced
            .then(|| self.witnesses.last().copied())
            .flatten()
            .into_iter()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn branch_count(&self) -> usize {
        1 + self.receipt_door_relocation_branches.len()
    }

    /// A velocity sign flip can bypass the ordinary thrust envelope only when
    /// the exact retained per-input replay both collided and reproduces every
    /// raw f32 and legacy wire tail field.
    pub(crate) fn proves_exact_f32_bounce_report(
        &self,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> bool {
        (self.bounced
            && Self::branch_matches_exact_report(
                &self.witnesses,
                canonical_state,
                direction,
                x,
                y,
                velocity_x,
                velocity_y,
            ))
            || self.receipt_door_relocation_branches.iter().any(|branch| {
                branch.bounced
                    && Self::branch_matches_exact_report(
                        &branch.witnesses,
                        canonical_state,
                        direction,
                        x,
                        y,
                        velocity_x,
                        velocity_y,
                    )
            })
    }
}

fn position_wire_component(value: f32) -> u16 {
    value.round().clamp(0.0, u16::MAX as f32) as u16
}

/// Reconstruct every missing logical movement record without mutating client
/// authority. Each record uses its own security-resolved door phase. Door
/// transitions are applied before that record's movement step, matching the
/// browser fixed-update order, including exact open-to-closed relocation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_canonical_position_gap(
    sender: &ClientHandle,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
) -> Option<CanonicalPositionReplay> {
    replay_canonical_position_gap_with_repel_application(
        sender,
        map,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        enemy_brick_tiles,
        current_sequence,
        accepted_server_tick,
        spawn_x,
        spawn_y,
        None,
        None,
    )
}

/// Exact logical boundary for a delivered hostile Repel. Browser networking
/// polls after the current fixed movement step, so the velocity replacement
/// occurs after movement for `sequence`; later records then integrate under
/// the exact server-authored logical duration carried by that snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanonicalRepelApplication {
    pub(crate) sequence: u16,
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
    pub(crate) speed: f32,
    pub(crate) radius: f32,
    /// Immutable server-decided replacement vector captured at event time.
    /// Delivery delay or mutable client state cannot re-evaluate eligibility.
    pub(crate) expected_velocity_x: f32,
    pub(crate) expected_velocity_y: f32,
    pub(crate) delivered_snapshot_tick: u32,
    /// Server-authored total suffix after `sequence`, narrowed at marker
    /// receipt so it can never extend beyond the original event deadline.
    pub(crate) active_steps: u16,
}

/// Apply the exact number of subsequent 60 Hz RepelSpeed records encoded by
/// the server in the weapon-bearing snapshot. The application record itself
/// precedes snapshot polling, so the window begins at `sequence + 1`.
pub(crate) fn canonical_repel_sequence_window(
    application: CanonicalRepelApplication,
) -> (u16, u16) {
    crate::gameplay_rules::repel_sequence_window_from_active_steps(
        application.sequence,
        u32::from(application.active_steps),
    )
}

pub(crate) fn repel_sequence_window_contains(window: (u16, u16), sequence: u16) -> bool {
    crate::gameplay_rules::repel_sequence_window_contains(window, sequence)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_canonical_position_gap_with_repel_application(
    sender: &ClientHandle,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    tail_bomb_recoil_speed: Option<f32>,
    repel_application: Option<CanonicalRepelApplication>,
) -> Option<CanonicalPositionReplay> {
    let previous_sequence = sender.last_position_sequence?;
    let previous_witness = sender.canonical_movement_witness.filter(|witness| {
        witness.sequence == previous_sequence && witness.life_seq == sender.life_seq
    })?;
    let anchor_tick = sender.last_position_server_tick?;
    replay_canonical_position_gap_from_witness(
        sender,
        previous_witness,
        anchor_tick,
        map,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        enemy_brick_tiles,
        current_sequence,
        accepted_server_tick,
        spawn_x,
        spawn_y,
        tail_bomb_recoil_speed,
        repel_application,
    )
}

/// Replay an authenticated input gap from an explicit exact witness. This is
/// shared by ordinary admission and the movement-correction shadow lineage;
/// both therefore execute identical nonlinear movement, gravity, doors,
/// walls, bricks, caps, recoil, and Repel semantics.
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_canonical_position_gap_from_witness(
    sender: &ClientHandle,
    previous_witness: CanonicalPositionWitness,
    anchor_tick: u32,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    tail_bomb_recoil_speed: Option<f32>,
    repel_application: Option<CanonicalRepelApplication>,
) -> Option<CanonicalPositionReplay> {
    replay_canonical_position_gap_from_witness_with_repel_window(
        sender,
        previous_witness,
        anchor_tick,
        map,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        enemy_brick_tiles,
        current_sequence,
        accepted_server_tick,
        spawn_x,
        spawn_y,
        tail_bomb_recoil_speed,
        repel_application,
        sender.server_repel_sequence_window,
    )
}

/// Counterfactual replay is proof for a correction only, never position
/// admission. The ordinary wrapper always supplies the server-sealed window.
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_canonical_position_gap_from_witness_with_repel_window(
    sender: &ClientHandle,
    previous_witness: CanonicalPositionWitness,
    anchor_tick: u32,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    tail_bomb_recoil_speed: Option<f32>,
    repel_application: Option<CanonicalRepelApplication>,
    repel_sequence_window: Option<(u16, u16)>,
) -> Option<CanonicalPositionReplay> {
    replay_gap_from_witness(
        sender,
        previous_witness,
        anchor_tick,
        map,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        None,
        enemy_brick_tiles,
        current_sequence,
        accepted_server_tick,
        spawn_x,
        spawn_y,
        tail_bomb_recoil_speed,
        repel_application,
        repel_sequence_window,
    )
}

/// The browser learned of a damage prize change part-way through a replayed
/// gap (`DamageMovementGrace`): its inputs up to and including
/// `last_sequence` were simulated with `upgrades`, the replaced tuning, and
/// the rest with the current tuning. It switches exactly once, at the frame
/// the private prize state reached it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EarlierMovementTuning {
    pub(crate) upgrades: PlayerMovementUpgrades,
    pub(crate) last_sequence: u16,
}

impl EarlierMovementTuning {
    /// Wrap-safe `sequence <= last_sequence`.
    fn covers(self, sequence: u16) -> bool {
        self.last_sequence.wrapping_sub(sequence) < (1 << 15)
    }
}

/// `replay_canonical_position_gap_with_repel_application` where the first
/// inputs of the gap ran at an earlier tuning (see `EarlierMovementTuning`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_canonical_position_gap_with_earlier_tuning(
    sender: &ClientHandle,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    earlier: EarlierMovementTuning,
    enemy_brick_tiles: &[(usize, usize)],
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    tail_bomb_recoil_speed: Option<f32>,
    repel_application: Option<CanonicalRepelApplication>,
) -> Option<CanonicalPositionReplay> {
    let previous_sequence = sender.last_position_sequence?;
    let previous_witness = sender.canonical_movement_witness.filter(|witness| {
        witness.sequence == previous_sequence && witness.life_seq == sender.life_seq
    })?;
    let anchor_tick = sender.last_position_server_tick?;
    replay_gap_from_witness(
        sender,
        previous_witness,
        anchor_tick,
        map,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        Some(earlier),
        enemy_brick_tiles,
        current_sequence,
        accepted_server_tick,
        spawn_x,
        spawn_y,
        tail_bomb_recoil_speed,
        repel_application,
        sender.server_repel_sequence_window,
    )
}

#[allow(clippy::too_many_arguments)]
fn replay_gap_from_witness(
    sender: &ClientHandle,
    previous_witness: CanonicalPositionWitness,
    anchor_tick: u32,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    earlier: Option<EarlierMovementTuning>,
    enemy_brick_tiles: &[(usize, usize)],
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    tail_bomb_recoil_speed: Option<f32>,
    repel_application: Option<CanonicalRepelApplication>,
    repel_sequence_window: Option<(u16, u16)>,
) -> Option<CanonicalPositionReplay> {
    if previous_witness.life_seq != sender.life_seq {
        return None;
    }
    let previous_sequence = previous_witness.sequence;
    let steps = current_sequence.wrapping_sub(previous_sequence);
    if steps == 0 || steps > MAX_OBSERVED_POSITION_SEQUENCE_STEPS {
        return None;
    }
    let records = (1..=steps)
        .map(|offset| {
            let sequence = previous_sequence.wrapping_add(offset);
            sender
                .recent_input_records
                .iter()
                .rev()
                .find(|record| record.sequence == sequence)
                .copied()
        })
        .collect::<Option<Vec<_>>>()?;
    let server_dt_ticks = crate::continuum::continuum_tick_diff(accepted_server_tick, anchor_tick);
    if server_dt_ticks < 0 {
        return None;
    }
    let positive_server_ticks = server_dt_ticks as u32;
    let step_ticks = (0..records.len())
        .map(|index| {
            let offset = positive_server_ticks.saturating_mul(index as u32 + 1)
                / records.len().max(1) as u32;
            crate::continuum::continuum_tick_add(anchor_tick, offset)
        })
        .collect::<Vec<_>>();
    let tuning = movement_tuning_from_ship(sender.player.ship, upgrades);
    let earlier_tuning = earlier.map(|earlier| {
        (
            earlier,
            movement_tuning_from_ship(sender.player.ship, earlier.upgrades),
        )
    });
    let radius = arena_ship_radius_pixels(sender.player.ship);
    let bounce_factor = arena_bounce_factor();
    // Current-protocol reports carry the prior exact raw f32 orientation and
    // movement state. Replay the shared operations themselves, including each
    // f32 addition/wrap, rather than solving an ideal cumulative direction
    // interval or selecting samples from an integer-wire cell.
    let replay_candidate = |candidate: ServerSimMovement,
                            receipt_relocation: Option<(usize, crate::doors::ClientDoorPhase)>|
     -> Option<CanonicalPositionReplayBranch> {
        let mut state = candidate;
        let mut position_x = state.position_x;
        let mut position_y = state.position_y;
        let mut prior_seed = previous_witness.door_seed;
        let mut prior_phase_tick = previous_witness.door_phase_tick;
        let mut bounced = false;
        let mut relocated = false;
        let mut wormhole_relocated = false;
        let mut safe_zone_weapon_stopped = false;
        let mut applied_repel_sequence_window = None;
        let mut witnesses = Vec::with_capacity(records.len());

        for (index, record) in records.iter().copied().enumerate() {
            let mut effective_door_seed = record.door_seed;
            let mut effective_door_phase_tick = record.door_phase_tick;
            if let Some((boundary, receipt_phase)) = receipt_relocation
                && index >= boundary
            {
                if record.door_seed != receipt_phase.seed
                    || crate::continuum::continuum_tick_diff(
                        receipt_phase.started_tick,
                        record.door_phase_tick,
                    ) <= 0
                {
                    return None;
                }
                if index == boundary {
                    let phases = crate::doors::client_door_phase_sequence(
                        sender.door_state,
                        prior_phase_tick,
                        receipt_phase.started_tick,
                    )?;
                    if phases.first().map(|phase| phase.seed) != Some(prior_seed)
                        || phases.last().copied() != Some(receipt_phase)
                    {
                        return None;
                    }
                    let mut boundary_relocated = false;
                    for transition in phases.windows(2) {
                        if crate::gameplay_rules::newly_closed_door_overlap(
                            position_x,
                            position_y,
                            radius,
                            MAP_TILES,
                            TILE_SIZE_PX,
                            transition[0].seed,
                            transition[1].seed,
                            |tile_x, tile_y| {
                                map.tiles
                                    .get(tile_y as usize * MAP_TILES + tile_x as usize)
                                    .copied()
                            },
                            crate::zone::position_blocking_tile_with_door_seed,
                        ) {
                            boundary_relocated = true;
                            relocated = true;
                            position_x = spawn_x as f32;
                            position_y = spawn_y as f32;
                            state.position_x = position_x;
                            state.position_y = position_y;
                            state.velocity_x = 0.0;
                            state.velocity_y = 0.0;
                        }
                    }
                    if !boundary_relocated {
                        return None;
                    }
                }
                effective_door_seed = receipt_phase.seed;
                effective_door_phase_tick = receipt_phase.started_tick;
            } else {
                let phase_delta =
                    crate::continuum::continuum_tick_diff(record.door_phase_tick, prior_phase_tick);
                if phase_delta > 0 {
                    if crate::continuum::continuum_tick_diff(
                        record.door_phase_tick,
                        accepted_server_tick,
                    ) > 0
                    {
                        return None;
                    }
                    let phases = crate::doors::client_door_phase_sequence(
                        sender.door_state,
                        prior_phase_tick,
                        record.door_phase_tick,
                    )?;
                    if phases.first().map(|phase| phase.seed) != Some(prior_seed)
                        || phases.last().map(|phase| phase.seed) != Some(record.door_seed)
                    {
                        return None;
                    }
                    for transition in phases.windows(2) {
                        if crate::gameplay_rules::newly_closed_door_overlap(
                            position_x,
                            position_y,
                            radius,
                            MAP_TILES,
                            TILE_SIZE_PX,
                            transition[0].seed,
                            transition[1].seed,
                            |tile_x, tile_y| {
                                map.tiles
                                    .get(tile_y as usize * MAP_TILES + tile_x as usize)
                                    .copied()
                            },
                            crate::zone::position_blocking_tile_with_door_seed,
                        ) {
                            relocated = true;
                            position_x = spawn_x as f32;
                            position_y = spawn_y as f32;
                            state.position_x = position_x;
                            state.position_y = position_y;
                            state.velocity_x = 0.0;
                            state.velocity_y = 0.0;
                        }
                    }
                } else if phase_delta < 0 || record.door_seed != prior_seed {
                    return None;
                }
            }

            let step_tick = step_ticks[index];
            let engine_shutdown =
                sender.server_engine_shutdown_active_for_sequence(step_tick, record.sequence);
            // Replay keeps EMP recharge suppression separate from prize movement Shutdown.
            let emp_shutdown = sender.server_emp_active_for_sequence(step_tick, record.sequence);
            let repel_active = repel_sequence_window.map_or_else(
                || {
                    sender
                        .server_repel_until_tick
                        .map_or(is_repel_active, |until| {
                            !crate::continuum::continuum_tick_reached(step_tick, until)
                        })
                },
                |window| repel_sequence_window_contains(window, record.sequence),
            ) || applied_repel_sequence_window
                .is_some_and(|window| repel_sequence_window_contains(window, record.sequence));
            let step_seed = [effective_door_seed];
            let step_tuning = earlier_tuning
                .filter(|(earlier, _)| earlier.covers(record.sequence))
                .map_or(tuning, |(_, earlier_tuning)| earlier_tuning);
            let stepped = integrate_with_collision(
                state,
                position_x,
                position_y,
                decode_input_bits(record.input.bits),
                step_tuning,
                PlayerMovementEnvironment {
                    engine_shutdown,
                    emp_shutdown,
                    rocket_active: record.input.bits & (1 << 5) != 0,
                    repel_active,
                    flagger_active: is_flagger,
                    has_attached_riders,
                    ..Default::default()
                },
                OBSERVED_POSITION_INTERVAL_SECONDS,
                radius,
                bounce_factor,
                map,
                &step_seed,
                enemy_brick_tiles,
                sender.player.ship,
            );
            state = stepped.state;
            position_x = stepped.position_x;
            position_y = stepped.position_y;
            bounced |= stepped.bounced;
            // Browser movement applies the wormhole terminal immediately
            // after the shared movement/status step and before any weapon
            // action is emitted. Reproduce the same map-derived boundary in
            // canonical replay: the browser still predicts it instantly for
            // smoothness, while only an exact server replay may authorize the
            // reported teleport.
            let wormhole_tile_x = (position_x / TILE_SIZE_PX).floor() as i32;
            let wormhole_tile_y = (position_y / TILE_SIZE_PX).floor() as i32;
            let entered_wormhole_this_step =
                is_wormhole_tile(map, wormhole_tile_x, wormhole_tile_y);
            if entered_wormhole_this_step {
                wormhole_relocated = true;
                position_x = spawn_x as f32;
                position_y = spawn_y as f32;
                state.position_x = position_x;
                state.position_y = position_y;
                state.velocity_x = 0.0;
                state.velocity_y = 0.0;
                state.energy = crate::gameplay_rules::wormhole_energy_after(state.energy);
            }
            if !entered_wormhole_this_step
                && record.input.bits & crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT != 0
            {
                if !apply_safe_zone_weapon_stop(&mut state, map, record.input.bits) {
                    return None;
                }
                safe_zone_weapon_stopped = true;
            }
            if !entered_wormhole_this_step
                && index + 1 == records.len()
                && let Some(recoil_speed) = tail_bomb_recoil_speed
            {
                let (heading_x, heading_y) =
                    crate::continuum_position::canonical_discrete_heading_components(
                        state.orientation,
                    );
                state.velocity_x -= heading_x * recoil_speed.max(0.0);
                state.velocity_y -= heading_y * recoil_speed.max(0.0);
            }
            if !entered_wormhole_this_step
                && let Some(application) = repel_application
                && record.sequence == application.sequence
            {
                state.velocity_x = application.expected_velocity_x;
                state.velocity_y = application.expected_velocity_y;
                applied_repel_sequence_window = Some(canonical_repel_sequence_window(application));
            }
            let wire_x = position_wire_component(position_x);
            let wire_y = position_wire_component(position_y);
            let wire_velocity_x =
                crate::continuum_position::protocol_velocity_component(state.velocity_x);
            let wire_velocity_y =
                crate::continuum_position::protocol_velocity_component(state.velocity_y);
            witnesses.push(CanonicalPositionWitness {
                sequence: record.sequence,
                life_seq: sender.life_seq,
                state,
                wire_x,
                wire_y,
                wire_velocity_x,
                wire_velocity_y,
                direction: crate::continuum_position::protocol_direction(state.orientation),
                door_seed: effective_door_seed,
                door_phase_tick: effective_door_phase_tick,
            });
            prior_seed = effective_door_seed;
            prior_phase_tick = effective_door_phase_tick;
        }
        Some(CanonicalPositionReplayBranch {
            witnesses,
            bounced,
            relocated,
            wormhole_relocated,
            safe_zone_weapon_stopped,
        })
    };

    let primary = replay_candidate(previous_witness.state, None)?;
    let receipt_phase =
        crate::doors::client_door_phase_candidates_at_tick(sender.door_state, accepted_server_tick)
            .into_iter()
            .filter_map(|phase| {
                let age =
                    crate::continuum::continuum_tick_diff(accepted_server_tick, phase.started_tick);
                (age >= 0).then_some((age as u32, phase))
            })
            .min_by_key(|(age, _)| *age)
            .map(|(_, phase)| phase)
            .filter(|phase| {
                crate::continuum::continuum_tick_diff(
                    phase.started_tick,
                    previous_witness.door_phase_tick,
                ) > 0
                    && records.last().is_some_and(|record| {
                        // The ordinary replay already owns forward phase
                        // changes. This exceptional branch exists only for a
                        // newer authenticated occurrence of the same compact
                        // seed byte, which an unordered rolling record cannot
                        // distinguish from its older occurrence by value.
                        phase.seed == record.door_seed
                            && crate::continuum::continuum_tick_diff(
                                phase.started_tick,
                                record.door_phase_tick,
                            ) > 0
                    })
            });
    let receipt_door_relocation_branches = receipt_phase
        .into_iter()
        .flat_map(|phase| {
            (0..records.len()).filter_map(move |boundary| {
                replay_candidate(previous_witness.state, Some((boundary, phase)))
            })
        })
        .collect();
    Some(CanonicalPositionReplay {
        witnesses: primary.witnesses,
        bounced: primary.bounced,
        relocated: primary.relocated,
        wormhole_relocated: primary.wormhole_relocated,
        safe_zone_weapon_stopped: primary.safe_zone_weapon_stopped,
        receipt_door_relocation_branches,
    })
}

/// Prove the exact wall-clock door-close relocation performed by the browser.
/// A seed change is not authority by itself: the retained canonical phases
/// must be exactly adjacent, the prior float hull must fit before the change,
/// and that same hull must overlap a door which becomes solid afterward. The
/// replay then starts that logical input step at the canonical spawn with zero
/// velocity, exactly as `Game::update_doors` does before fixed-step movement.
///
/// This returns a one-position admission witness only. Sequence advancement
/// consumes it naturally; lifecycle resets clear both the prior position and
/// retained input records, so it cannot authorize a later report or life.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn position_replay_matches_door_close_relocation(
    sender: &ClientHandle,
    map: &ZoneMap,
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    previous_x: u16,
    previous_y: u16,
    current_sequence: u16,
    accepted_server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    reported_direction: u8,
    reported_x: u16,
    reported_y: u16,
    reported_velocity_x: i16,
    reported_velocity_y: i16,
) -> bool {
    if sender.door_state.is_none() {
        return false;
    }
    let Some(previous_sequence) = sender.last_position_sequence else {
        return false;
    };
    let steps = current_sequence.wrapping_sub(previous_sequence);
    if steps == 0 || steps > MAX_OBSERVED_POSITION_SEQUENCE_STEPS {
        return false;
    }
    let Some(previous_record) = sender
        .recent_input_records
        .iter()
        .rev()
        .find(|record| record.sequence == previous_sequence)
        .copied()
    else {
        return false;
    };
    let Some(records) = (1..=steps)
        .map(|offset| {
            let sequence = previous_sequence.wrapping_add(offset);
            sender
                .recent_input_records
                .iter()
                .rev()
                .find(|record| record.sequence == sequence)
                .copied()
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let (Some(previous), Some(anchor_tick)) =
        (sender.server_sim_state, sender.server_sim_anchor_tick)
    else {
        return false;
    };
    let server_dt_ticks = crate::continuum::continuum_tick_diff(accepted_server_tick, anchor_tick);
    if server_dt_ticks <= 0 {
        return false;
    }

    let tuning = movement_tuning_from_ship(sender.player.ship, upgrades);
    let radius = arena_ship_radius_pixels(sender.player.ship);
    let bounce_factor = arena_bounce_factor();
    // Door relocation authority starts from the exact last admitted packet,
    // not the nearby private observer float. The browser applies the phase
    // transition to its accepted/reported position before the next movement
    // step; bounded observer drift must not erase that authenticated AABB or
    // make an otherwise canonical spawn rebase intermittently fail.
    let mut replay_state = previous;
    replay_state.position_x = previous_x as f32;
    replay_state.position_y = previous_y as f32;
    let mut integration = SimIntegration {
        state: replay_state,
        position_x: previous_x as f32,
        position_y: previous_y as f32,
        bounced: false,
    };
    let inputs = records
        .iter()
        .map(|record| record.input)
        .collect::<Vec<_>>();
    let positive_server_ticks = server_dt_ticks as u32;
    let step_ticks = (0..records.len())
        .map(|index| {
            let offset = positive_server_ticks.saturating_mul(index as u32 + 1)
                / records.len().max(1) as u32;
            crate::continuum::continuum_tick_add(anchor_tick, offset)
        })
        .collect::<Vec<_>>();
    let rotation_steps = inputs
        .iter()
        .zip(&step_ticks)
        .enumerate()
        .map(|(index, (_input, &step_tick))| {
            let engine_shutdown = sender
                .server_engine_shutdown_active_for_sequence(step_tick, records[index].sequence);
            input_rotation_step(
                if engine_shutdown {
                    tuning.shutdown_rotation_per_second
                } else {
                    tuning.rotation_per_second
                },
                OBSERVED_POSITION_INTERVAL_SECONDS,
            )
        })
        .collect::<Vec<_>>();
    let Some(orientation) = constrained_initial_orientation(
        integration.state.orientation,
        prior_canonical_direction(sender, integration.state.orientation),
        &inputs,
        &rotation_steps,
    ) else {
        return false;
    };
    integration.state.orientation = orientation;

    let mut prior_phase = previous_record;
    let mut relocated = false;
    let mut traversed_close_candidate = false;
    for (index, record) in records.iter().copied().enumerate() {
        let phase_delta = crate::continuum::continuum_tick_diff(
            record.door_phase_tick,
            prior_phase.door_phase_tick,
        );
        if phase_delta > 0 {
            // Relocation is wall-clock authority, not adjacent-seed grace.
            // Even a resolved one-tick rounding candidate cannot relocate a
            // ship until that canonical transition has actually started at
            // this server receipt.
            if crate::continuum::continuum_tick_diff(record.door_phase_tick, accepted_server_tick)
                > 0
            {
                return false;
            }

            let Some(phases) = crate::doors::client_door_phase_sequence(
                sender.door_state,
                prior_phase.door_phase_tick,
                record.door_phase_tick,
            ) else {
                return false;
            };
            if phases.first().map(|phase| phase.seed) != Some(prior_phase.door_seed)
                || phases.last().map(|phase| phase.seed) != Some(record.door_seed)
            {
                return false;
            }
            for transition in phases.windows(2) {
                traversed_close_candidate |=
                    door_transition_closes_candidate(transition[0].seed, transition[1].seed);
                if !relocated
                    && crate::gameplay_rules::newly_closed_door_overlap(
                        integration.position_x,
                        integration.position_y,
                        radius,
                        MAP_TILES,
                        TILE_SIZE_PX,
                        transition[0].seed,
                        transition[1].seed,
                        |tile_x, tile_y| {
                            map.tiles
                                .get(tile_y as usize * MAP_TILES + tile_x as usize)
                                .copied()
                        },
                        crate::zone::position_blocking_tile_with_door_seed,
                    )
                {
                    relocated = true;
                    integration.position_x = spawn_x as f32;
                    integration.position_y = spawn_y as f32;
                    integration.state.position_x = spawn_x as f32;
                    integration.state.position_y = spawn_y as f32;
                    integration.state.velocity_x = 0.0;
                    integration.state.velocity_y = 0.0;
                }
            }
        } else if phase_delta < 0 || record.door_seed != prior_phase.door_seed {
            return false;
        }

        let step_tick = step_ticks[index];
        let engine_shutdown =
            sender.server_engine_shutdown_active_for_sequence(step_tick, record.sequence);
        let emp_shutdown = sender.server_emp_active_for_sequence(step_tick, record.sequence);
        let repel_active = sender
            .server_repel_until_tick
            .map_or(is_repel_active, |until| {
                !crate::continuum::continuum_tick_reached(step_tick, until)
            });
        let step_seed = [record.door_seed];
        integration = integrate_with_collision(
            integration.state,
            integration.position_x,
            integration.position_y,
            decode_input_bits(record.input.bits),
            tuning,
            PlayerMovementEnvironment {
                engine_shutdown,
                emp_shutdown,
                rocket_active: record.input.bits & (1 << 5) != 0,
                repel_active,
                flagger_active: is_flagger,
                has_attached_riders,
                ..Default::default()
            },
            OBSERVED_POSITION_INTERVAL_SECONDS,
            radius,
            bounce_factor,
            map,
            &step_seed,
            enemy_brick_tiles,
            sender.player.ship,
        );
        prior_phase = record;
    }
    if !relocated {
        // Ordinary position relays intentionally stay quiet. A client report
        // at the canonical spawn is the narrow signal that it believes a
        // door-close relocation happened, so retain enough authenticated
        // context to diagnose a witness disagreement without flooding the
        // server log for every non-relocation packet.
        if traversed_close_candidate && reported_x == spawn_x && reported_y == spawn_y {
            let final_record = records.last().expect("non-empty replay record range");
            debug!(
                previous_sequence,
                current_sequence,
                exact_previous_x = previous_x,
                exact_previous_y = previous_y,
                observer_previous_x = previous.position_x,
                observer_previous_y = previous.position_y,
                previous_phase_tick = previous_record.door_phase_tick,
                previous_door_seed = previous_record.door_seed,
                final_phase_tick = final_record.door_phase_tick,
                final_door_seed = final_record.door_seed,
                accepted_server_tick,
                radius,
                "canonical door-close relocation missed every authenticated intermediate close"
            );
        }
        return false;
    }

    let expected_x = integration.position_x.round().clamp(0.0, u16::MAX as f32) as u16;
    let expected_y = integration.position_y.round().clamp(0.0, u16::MAX as f32) as u16;
    let reported_velocity_x =
        crate::continuum_position::protocol_velocity_pixels_per_second(reported_velocity_x);
    let reported_velocity_y =
        crate::continuum_position::protocol_velocity_pixels_per_second(reported_velocity_y);
    let expected_direction =
        crate::continuum_position::protocol_direction(integration.state.orientation);
    let velocity_error = (integration.state.velocity_x - reported_velocity_x)
        .hypot(integration.state.velocity_y - reported_velocity_y);
    let matches = expected_x == reported_x
        && expected_y == reported_y
        && expected_direction == reported_direction
        && velocity_error <= 0.2;
    if !matches && reported_x == spawn_x && reported_y == spawn_y {
        let final_record = records.last().expect("non-empty replay record range");
        debug!(
            previous_sequence,
            current_sequence,
            exact_previous_x = previous_x,
            exact_previous_y = previous_y,
            observer_previous_x = previous.position_x,
            observer_previous_y = previous.position_y,
            previous_phase_tick = previous_record.door_phase_tick,
            previous_door_seed = previous_record.door_seed,
            final_phase_tick = final_record.door_phase_tick,
            final_door_seed = final_record.door_seed,
            accepted_server_tick,
            expected_x,
            expected_y,
            reported_x,
            reported_y,
            expected_direction,
            reported_direction,
            expected_velocity_x = integration.state.velocity_x,
            expected_velocity_y = integration.state.velocity_y,
            reported_velocity_x,
            reported_velocity_y,
            velocity_error,
            "canonical door-close relocation replayed but final vector did not match"
        );
    }
    matches
}

/// Dynamic-door bit `1` means closed. Diagnostics about a missed relocation
/// are useful only when the authenticated phase range actually crossed at
/// least one open-to-closed bit; same-phase spawn traffic must stay silent.
#[cfg(test)]
fn door_transition_closes_candidate(before_seed: u8, after_seed: u8) -> bool {
    after_seed & !before_seed != 0
}

#[cfg(test)]
fn circle_hull_fits_with_door_seed(
    x: f32,
    y: f32,
    radius: f32,
    map: &ZoneMap,
    door_seed: u8,
) -> bool {
    let min_x = ((x - radius) / TILE_SIZE_PX).floor() as i32;
    let min_y = ((y - radius) / TILE_SIZE_PX).floor() as i32;
    let max_x = ((x + radius) / TILE_SIZE_PX).floor() as i32;
    let max_y = ((y + radius) / TILE_SIZE_PX).floor() as i32;
    for tile_y in min_y..=max_y {
        for tile_x in min_x..=max_x {
            if tile_x < 0 || tile_y < 0 || tile_x >= MAP_TILES as i32 || tile_y >= MAP_TILES as i32
            {
                return false;
            }
            let tile = map.tiles[tile_y as usize * MAP_TILES + tile_x as usize];
            if crate::zone::position_blocking_tile_with_door_seed(tile, door_seed)
                && circle_overlaps_tile(x, y, radius, tile_x, tile_y)
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
fn circle_overlaps_tile(x: f32, y: f32, radius: f32, tile_x: i32, tile_y: i32) -> bool {
    let min_x = tile_x as f32 * TILE_SIZE_PX;
    let min_y = tile_y as f32 * TILE_SIZE_PX;
    let closest_x = x.clamp(min_x, min_x + TILE_SIZE_PX);
    let closest_y = y.clamp(min_y, min_y + TILE_SIZE_PX);
    let dx = x - closest_x;
    let dy = y - closest_y;
    dx * dx + dy * dy <= radius * radius
}

/// Per-accepted-sample leak applied to the cheat-snap-back drift
/// accumulator before adding the new frame's error. < 1.0 so honest
/// sign-random jitter decays instead of random-walking to the threshold
/// over a long session; the closer to 1.0, the longer the detection
/// window and the more sensitive to small systematic gains. The retained
/// Chrome/Firefox calibration matrix measured a 0.921631 px worst stable-
/// active deviation and no honest correction in 2,075 samples. A 0.96 leak gives a 25-sample
/// effective window (~0.42 s at 60 Hz) while smoothing isolated recovery and
/// collision residuals more heavily than the former 0.94 calibration.
const MOVEMENT_DRIFT_LEAK: f32 = 0.96;

/// `protocol_position_coord` rounds each client float to the nearest integer,
/// so one accepted wire coordinate proves that the real coordinate lies in
/// this half-pixel cell. The private parity model may reconcile to the closest
/// point in that cell; movement evidence must count only displacement beyond
/// it. This is a wire-format fact, not a generic movement tolerance.
const POSITION_WIRE_HALF_CELL_PX: f32 = 0.5;

/// Return the closest float compatible with one rounded wire coordinate and
/// the signed unexplained distance from the prediction to that cell.
fn reconcile_rounded_position_component(predicted: f32, reported: u16) -> (f32, f32) {
    let reported = reported as f32;
    let lower = (reported - POSITION_WIRE_HALF_CELL_PX).max(0.0);
    let upper = (reported + POSITION_WIRE_HALF_CELL_PX).min(u16::MAX as f32);
    let reconciled = predicted.clamp(lower, upper);
    (reconciled, reconciled - predicted)
}

/// Accumulated illegitimate displacement (px) at which the backstop
/// bounces the player to the authoritative position. Sits well above the
/// honest-jitter steady-state band. After subtracting the exact half-pixel
/// wire cell, the final browser maximum can contribute at most 0.421631 px
/// along one axis; even if that worst sample repeated forever, the 0.96 leak
/// converges below 10.541 px per axis (14.907 px as a conservative two-axis
/// vector). A 32 px threshold leaves more than 17 px of vector margin while a
/// systematic 1.5 px/frame gain still crosses in ~48 samples.
pub(crate) const MOVEMENT_DRIFT_SNAP_PX: f32 = 32.0;

/// Observe one client → server position. When `ZUBSPACE_SERVER_SIM_OBSERVE`
/// is set, log how far the server's simulated next position lands
/// from the reported position. The accepted client report remains the
/// server-owned snapshot position after validation; this simulation is
/// kept as a private model for drift detection and future reconciliation.
/// If the model drifts beyond `SIM_TRACKING_RESYNC_PX`, it re-seeds from
/// the accepted report so model error cannot accumulate.
///
/// `enemy_brick_tiles` carries the tile indices of bricks dropped by
/// rival frequencies, pre-filtered by the caller. They are treated as
/// solid by the collision sweep. Friendly bricks are *not* in this
/// list, so the server-sim lets the player slide through them —
/// matching the client's
/// `move_entity_with_collision_and_passable_tiles` semantics.
pub(crate) fn observe_simulation_for_accepted_position(
    sender: &mut ClientHandle,
    map: &ZoneMap,
    door_seeds: &[u8],
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    previous_x: Option<u16>,
    previous_y: Option<u16>,
    snapshot_direction: u8,
    snapshot_x: u16,
    snapshot_y: u16,
    snapshot_velocity_x: i16,
    snapshot_velocity_y: i16,
    position_input_sequence: Option<u16>,
    accepted_server_tick: u32,
    tail_bomb_recoil_speed: Option<f32>,
    observation_active: bool,
) -> SimObservationResult {
    observe_simulation_inner_with_tail_recoil(
        sender,
        map,
        door_seeds,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        enemy_brick_tiles,
        previous_x,
        previous_y,
        snapshot_direction,
        snapshot_x,
        snapshot_y,
        snapshot_velocity_x,
        snapshot_velocity_y,
        position_input_sequence,
        accepted_server_tick,
        tail_bomb_recoil_speed,
        observation_active,
    )
}

/// Maximum disagreement where the private server sim keeps integrating
/// forward from its own state. Above this visual-noise floor we re-seed
/// from the accepted client position. The re-seed is internal only; remote
/// snapshots never alternate between sim and reported positions.
const SIM_TRACKING_RESYNC_PX: f32 = 6.0;

/// Phase 2 polish (rotate_right follow-up): EWMA weight for pulling
/// the running sim's velocity toward the client's reported velocity
/// on each observation that stays inside `SIM_TRACKING_RESYNC_PX`.
///
/// Earlier Chrome QA exposed a slow monotonic position drift
/// during pure-rotate and pure-coast inputs (~1.5 px / 30 ms ≈ 50 px/s
/// y-velocity disagreement). The sim was comparing positions only,
/// never velocities, so small persistent velocity errors would
/// accumulate position drift over many observations until the
/// internal resync fired. Blending velocities at α = 1/8 per
/// observation (~5-frame half-life at the ~40-60 Hz position cadence)
/// converges sim velocity toward reported within tolerance while a
/// genuine divergence still triggers full re-seed.
const SIM_VELOCITY_BLEND_ALPHA: f32 = 0.125;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputAlignment {
    /// Unit fixtures predating sequenced input use an explicit unsequenced
    /// path; production observations never report this classification.
    Fixture,
    Stable,
    Transition,
    Missing,
    Invalid,
}

impl InputAlignment {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fixture => "fixture",
            Self::Stable => "stable",
            Self::Transition => "transition",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }

    fn is_comparable(self) -> bool {
        matches!(self, Self::Fixture | Self::Stable | Self::Transition)
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn observe_simulation_inner(
    sender: &mut ClientHandle,
    map: &ZoneMap,
    door_seeds: &[u8],
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    previous_x: Option<u16>,
    previous_y: Option<u16>,
    snapshot_direction: u8,
    snapshot_x: u16,
    snapshot_y: u16,
    snapshot_velocity_x: i16,
    snapshot_velocity_y: i16,
    position_input_sequence: Option<u16>,
    accepted_server_tick: u32,
    observation_active: bool,
) -> SimObservationResult {
    observe_simulation_inner_with_tail_recoil(
        sender,
        map,
        door_seeds,
        is_flagger,
        has_attached_riders,
        is_repel_active,
        upgrades,
        enemy_brick_tiles,
        previous_x,
        previous_y,
        snapshot_direction,
        snapshot_x,
        snapshot_y,
        snapshot_velocity_x,
        snapshot_velocity_y,
        position_input_sequence,
        accepted_server_tick,
        None,
        observation_active,
    )
}

#[allow(clippy::too_many_arguments)]
fn observe_simulation_inner_with_tail_recoil(
    sender: &mut ClientHandle,
    map: &ZoneMap,
    door_seeds: &[u8],
    is_flagger: bool,
    has_attached_riders: bool,
    is_repel_active: bool,
    upgrades: PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    previous_x: Option<u16>,
    previous_y: Option<u16>,
    snapshot_direction: u8,
    snapshot_x: u16,
    snapshot_y: u16,
    snapshot_velocity_x: i16,
    snapshot_velocity_y: i16,
    position_input_sequence: Option<u16>,
    accepted_server_tick: u32,
    tail_bomb_recoil_speed: Option<f32>,
    observation_active: bool,
) -> SimObservationResult {
    let mut result = SimObservationResult::default();

    let ship = sender.player.ship;
    if ship >= 8 {
        return result;
    }

    let reported = ServerSimMovement {
        position_x: snapshot_x as f32,
        position_y: snapshot_y as f32,
        orientation: snapshot_direction as f32 / 40.0,
        velocity_x: snapshot_velocity_x as f32 / 10.0,
        velocity_y: snapshot_velocity_y as f32 / 10.0,
        energy: sender.server_energy as f32,
        max_energy: sender.server_max_energy as f32,
    };
    let matched_input = position_input_sequence
        .and_then(|sequence| {
            sender
                .recent_input_records
                .iter()
                .rev()
                .find(|record| record.sequence == sequence)
                .map(|record| record.input)
        })
        .or_else(|| {
            position_input_sequence
                .is_none()
                .then_some(sender.last_input_record)
                .flatten()
        });
    let position_sequence_steps = sender
        .last_position_sequence
        .zip(position_input_sequence)
        .map(|(previous, current)| current.wrapping_sub(previous));
    let replay_inputs = match (sender.last_position_sequence, position_input_sequence) {
        (Some(previous), Some(current)) => {
            let steps = current.wrapping_sub(previous);
            (steps > 0 && steps <= MAX_OBSERVED_POSITION_SEQUENCE_STEPS)
                .then(|| {
                    (1..=steps)
                        .map(|offset| {
                            let sequence = previous.wrapping_add(offset);
                            sender
                                .recent_input_records
                                .iter()
                                .rev()
                                .find(|record| record.sequence == sequence)
                                .map(|record| record.input)
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .flatten()
        }
        (None, None) => matched_input.map(|input| vec![input]),
        _ => None,
    };
    let replay_door_seeds = match (sender.last_position_sequence, position_input_sequence) {
        (Some(previous), Some(current)) => {
            let steps = current.wrapping_sub(previous);
            (steps > 0 && steps <= MAX_OBSERVED_POSITION_SEQUENCE_STEPS)
                .then(|| {
                    (1..=steps)
                        .map(|offset| {
                            let sequence = previous.wrapping_add(offset);
                            sender
                                .recent_input_records
                                .iter()
                                .rev()
                                .find(|record| record.sequence == sequence)
                                .map(|record| record.door_seed)
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .flatten()
        }
        (None, None) => matched_input.map(|_| Vec::new()),
        _ => None,
    };
    let mut input_alignment = match (position_input_sequence, matched_input) {
        (None, Some(_)) => InputAlignment::Fixture,
        (None, None) => InputAlignment::Missing,
        (Some(_), None) => InputAlignment::Missing,
        (Some(_current), Some(input)) => {
            match position_sequence_steps {
                Some(steps) if steps > 0 && steps <= MAX_OBSERVED_POSITION_SEQUENCE_STEPS => {
                    if replay_inputs.is_none() {
                        InputAlignment::Missing
                    } else if sender.server_sim_input_bits != Some(input.bits)
                        || replay_inputs.as_ref().is_some_and(|inputs| {
                            inputs.windows(2).any(|pair| pair[0].bits != pair[1].bits)
                        })
                    {
                        InputAlignment::Transition
                    } else {
                        InputAlignment::Stable
                    }
                }
                None if sender.last_position_sequence.is_none()
                    && sender.server_sim_input_bits == Some(input.bits) =>
                {
                    // Backwards-compatible unit-fixture/bootstrap path. A real
                    // first accepted position has no prior sim state to compare.
                    InputAlignment::Stable
                }
                None if sender.last_position_sequence.is_none() => InputAlignment::Transition,
                _ => InputAlignment::Missing,
            }
        }
    };

    if observation_active && matches!(input_alignment, InputAlignment::Missing) {
        debug!(
            player_id = sender.player.id,
            ship,
            position_input_sequence,
            latest_input_sequence = sender.last_input_seq,
            "sim observation input missing"
        );
    }

    let mut tracked_step: Option<ServerSimMovement> = None;
    // True once we have integrated a prediction this frame. Used to decide
    // whether a re-seed should reset the drift accumulator: a fresh
    // baseline (first sample / long gap) resets it, but a resync caused by
    // a large single-frame jump must NOT — that jump is itself a cheat
    // signal we want the accumulator to keep.
    let mut had_prediction = false;

    if let (Some(previous), Some(anchor_tick), Some(inputs), Some(px), Some(py)) = (
        sender.server_sim_state,
        sender.server_sim_anchor_tick,
        replay_inputs.as_deref(),
        previous_x,
        previous_y,
    ) {
        // dt_ticks is in Continuum 100 Hz ticks. We bound dt to keep
        // extrapolation sane — long gaps probably mean the player
        // rejoined or the browser tab was unfocused.
        // [REVIEW M14] Signed, wrap-safe tick delta. The previous
        // `wrapping_sub(..) & 0x7fff_ffff` turned a slightly-ahead anchor
        // (clock skew) into a ~2.1B-tick "elapsed" that silently skipped
        // the comparison; a negative diff now falls out via the
        // `dt_seconds > 0.0` guard below and re-seeds cleanly.
        let server_dt_ticks =
            crate::continuum::continuum_tick_diff(accepted_server_tick, anchor_tick);
        let server_dt_seconds = server_dt_ticks as f32 / 100.0;
        let dt_seconds = position_sequence_steps
            .map(|steps| steps as f32 * OBSERVED_POSITION_INTERVAL_SECONDS)
            .unwrap_or(server_dt_seconds);
        let within_observation_window = position_sequence_steps
            .map_or(dt_seconds <= SKIP_OBSERVATION_BEYOND_DT_SECONDS, |steps| {
                steps <= MAX_OBSERVED_POSITION_SEQUENCE_STEPS
            });
        if dt_seconds > 0.0 && within_observation_window {
            let tuning = movement_tuning_from_ship(ship, upgrades);
            let radius = arena_ship_radius_pixels(ship);
            let bounce_factor = arena_bounce_factor();
            let per_step_dt = if position_sequence_steps.is_some() {
                OBSERVED_POSITION_INTERVAL_SECONDS
            } else {
                dt_seconds
            };
            let positive_server_ticks = server_dt_ticks.max(0) as u32;
            let mut integration = SimIntegration {
                state: previous,
                position_x: if previous.position_x.is_finite() {
                    previous.position_x
                } else {
                    px as f32
                },
                position_y: if previous.position_y.is_finite() {
                    previous.position_y
                } else {
                    py as f32
                },
                bounced: false,
            };
            let step_ticks = (0..inputs.len())
                .map(|index| {
                    let step_tick_offset = positive_server_ticks.saturating_mul(index as u32 + 1)
                        / inputs.len().max(1) as u32;
                    crate::continuum::continuum_tick_add(anchor_tick, step_tick_offset)
                })
                .collect::<Vec<_>>();
            if position_sequence_steps.is_some() {
                let rotation_steps = inputs
                    .iter()
                    .zip(&step_ticks)
                    .enumerate()
                    .map(|(index, (_input, &step_tick))| {
                        let input_sequence = sender
                            .last_position_sequence
                            .map(|previous| previous.wrapping_add(index as u16 + 1));
                        let engine_shutdown = input_sequence.map_or_else(
                            || sender.server_engine_shutdown_active(step_tick),
                            |sequence| {
                                sender
                                    .server_engine_shutdown_active_for_sequence(step_tick, sequence)
                            },
                        );
                        input_rotation_step(
                            if engine_shutdown {
                                tuning.shutdown_rotation_per_second
                            } else {
                                tuning.rotation_per_second
                            },
                            per_step_dt,
                        )
                    })
                    .collect::<Vec<_>>();
                if !diagnostic_orientation_history_matches(
                    sender,
                    integration.state.orientation,
                    inputs,
                    &rotation_steps,
                ) {
                    // Production admission rejects this before entering the
                    // observer. Preserve an explicit invalid classification
                    // for direct fixtures/diagnostics rather than hiding an
                    // impossible direction history as missing input.
                    input_alignment = InputAlignment::Invalid;
                }
            }
            for (index, input) in inputs.iter().copied().enumerate() {
                let step_tick = step_ticks[index];
                let input_sequence = sender
                    .last_position_sequence
                    .map(|previous| previous.wrapping_add(index as u16 + 1));
                let engine_shutdown = input_sequence.map_or_else(
                    || sender.server_engine_shutdown_active(step_tick),
                    |sequence| {
                        sender.server_engine_shutdown_active_for_sequence(step_tick, sequence)
                    },
                );
                let emp_shutdown = input_sequence.map_or_else(
                    || sender.server_emp_active(step_tick),
                    |sequence| sender.server_emp_active_for_sequence(step_tick, sequence),
                );
                let repel_active = sender
                    .server_repel_sequence_window
                    .zip(input_sequence)
                    .map_or_else(
                        || {
                            sender
                                .server_repel_observer_until_tick
                                .map_or(is_repel_active, |until| {
                                    !crate::continuum::continuum_tick_reached(step_tick, until)
                                })
                        },
                        |(window, sequence)| {
                            crate::gameplay_rules::repel_sequence_window_contains(window, sequence)
                        },
                    );
                let step_door_seed = replay_door_seeds
                    .as_deref()
                    .and_then(|seeds| seeds.get(index))
                    .copied();
                let step_door_seed_slice = step_door_seed
                    .as_ref()
                    .map(std::slice::from_ref)
                    .unwrap_or(door_seeds);
                let stepped = integrate_with_collision(
                    integration.state,
                    integration.position_x,
                    integration.position_y,
                    decode_input_bits(input.bits),
                    tuning,
                    PlayerMovementEnvironment {
                        engine_shutdown,
                        emp_shutdown,
                        rocket_active: input.bits & (1 << 5) != 0,
                        repel_active,
                        flagger_active: is_flagger,
                        has_attached_riders,
                        ..Default::default()
                    },
                    per_step_dt,
                    radius,
                    bounce_factor,
                    map,
                    step_door_seed_slice,
                    enemy_brick_tiles,
                    ship,
                );
                integration = SimIntegration {
                    bounced: integration.bounced || stepped.bounced,
                    ..stepped
                };
                if !apply_safe_zone_weapon_stop(&mut integration.state, map, input.bits) {
                    // Canonical admission already validated the action at its
                    // exact pose. A private observer outside that safe tile
                    // has diverged; it cannot use an omitted stop as evidence
                    // against the accepted movement. Invalid alignment rebases
                    // the observer below without issuing a correction.
                    input_alignment = InputAlignment::Invalid;
                    break;
                }
            }
            // Bomb recoil is part of the accepted tail movement state. It is
            // applied after movement (and before a hostile Repel replacement)
            // by the browser and canonical admission replay, so the private
            // observer must retain the same post-tail velocity. Omitting it
            // poisons the next prediction and can turn an honest bomb into
            // sustained-drift evidence.
            if let Some(recoil_speed) = tail_bomb_recoil_speed {
                let (heading_x, heading_y) =
                    crate::continuum_position::canonical_discrete_heading_components(
                        integration.state.orientation,
                    );
                integration.state.velocity_x -= heading_x * recoil_speed.max(0.0);
                integration.state.velocity_y -= heading_y * recoil_speed.max(0.0);
            }
            let SimIntegration {
                state: stepped,
                position_x: predicted_x,
                position_y: predicted_y,
                bounced,
            } = integration;
            let input = inputs.last().copied().expect("replay input is nonempty");
            let input_door_seed = replay_door_seeds
                .as_deref()
                .and_then(|seeds| seeds.last())
                .copied();
            let engine_shutdown = position_input_sequence.map_or_else(
                || sender.server_engine_shutdown_active(accepted_server_tick),
                |sequence| {
                    sender
                        .server_engine_shutdown_active_for_sequence(accepted_server_tick, sequence)
                },
            );
            let dx = snapshot_x as f32 - predicted_x;
            let dy = snapshot_y as f32 - predicted_y;
            let deviation = (dx * dx + dy * dy).sqrt();
            let (reconciled_x, residual_x) =
                reconcile_rounded_position_component(predicted_x, snapshot_x);
            let (reconciled_y, residual_y) =
                reconcile_rounded_position_component(predicted_y, snapshot_y);
            if input_alignment.is_comparable() {
                result.deviation_px = Some(deviation.round().clamp(0.0, u16::MAX as f32) as u16);
            }

            // Observation mode is explicitly opt-in and exists to measure
            // client/server parity. Emit every comparison at debug level so
            // browser soaks can calculate a distribution instead of only
            // proving that no sample crossed the coarse alert threshold.
            if observation_active {
                debug!(
                    player_id = sender.player.id,
                    ship,
                    dt_seconds,
                    server_dt_seconds,
                    position_sequence_steps,
                    deviation,
                    input_bits = input.bits,
                    position_input_sequence,
                    input_door_seed,
                    latest_input_sequence = sender.last_input_seq,
                    input_alignment = input_alignment.as_str(),
                    bounced,
                    engine_shutdown,
                    rocket_active = environment_rocket_active(input.bits),
                    repel_active = is_repel_active,
                    flagger_active = is_flagger,
                    has_attached_riders,
                    "sim observation sample"
                );
            }

            // Cheat-snap-back: leak the prior accumulated drift, then add
            // this frame's signed gap beyond the exact half-pixel cell encoded
            // by the rounded report. Projection into that cell below keeps the
            // private float model compatible with honest subpixel state;
            // accumulating the residual separately means a client cannot add
            // one or two whole pixels per report and hide behind that
            // reconciliation.
            if input_alignment.is_comparable() {
                had_prediction = true;
                sender.movement_drift_x =
                    sender.movement_drift_x * MOVEMENT_DRIFT_LEAK + residual_x;
                sender.movement_drift_y =
                    sender.movement_drift_y * MOVEMENT_DRIFT_LEAK + residual_y;
                let drift_magnitude = (sender.movement_drift_x * sender.movement_drift_x
                    + sender.movement_drift_y * sender.movement_drift_y)
                    .sqrt();
                if drift_magnitude > MOVEMENT_DRIFT_SNAP_PX {
                    result.snap_back = Some(ServerSimMovement {
                        position_x: predicted_x,
                        position_y: predicted_y,
                        ..stepped
                    });
                    sender.movement_drift_x = 0.0;
                    sender.movement_drift_y = 0.0;
                }
            }

            if observation_active && deviation > DEVIATION_LOG_THRESHOLD_PX {
                debug!(
                    player_id = sender.player.id,
                    ship,
                    dt_seconds,
                    deviation,
                    input_bits = input.bits,
                    bounced,
                    reported_x = snapshot_x,
                    reported_y = snapshot_y,
                    predicted_x,
                    predicted_y,
                    "sim observation deviation"
                );
            }
            if input_alignment.is_comparable()
                && bounced
                && (residual_x != 0.0 || residual_y != 0.0)
            {
                // A collision under the exact authenticated input/door replay
                // can amplify a subpixel difference into several pixels of
                // endpoint/velocity disagreement. Rebase the private model
                // once to the closest position compatible with the accepted
                // report and its quantized velocity, but keep the constrained
                // replay orientation and all accumulated security evidence.
                // The very next report is therefore validated from this
                // collision-proven baseline instead of inheriting a cascade.
                tracked_step = Some(ServerSimMovement {
                    position_x: reconciled_x,
                    position_y: reconciled_y,
                    orientation: stepped.orientation,
                    ..reported
                });
                result.resynced = true;
            } else if input_alignment.is_comparable() && deviation <= SIM_TRACKING_RESYNC_PX {
                tracked_step = Some(ServerSimMovement {
                    position_x: reconciled_x,
                    position_y: reconciled_y,
                    ..stepped
                });
            } else if input_alignment.is_comparable() {
                result.resynced = true;
                if observation_active {
                    debug!(
                        player_id = sender.player.id,
                        ship, dt_seconds, deviation, "sim tracking resync"
                    );
                }
            }
        }
    }

    if let Some(mut stepped) = tracked_step {
        // Phase 2 polish (rotate_right follow-up): blend sim velocity
        // toward reported velocity. Within internal resync tolerance the
        // sim is closely tracking the client, so small persistent
        // velocity errors are physics-model jitter, not lies. Pulling
        // velocity toward reported each tick prevents that jitter
        // from accumulating position drift past the internal resync
        // threshold.
        if result.snap_back.is_none() {
            stepped.velocity_x +=
                (reported.velocity_x - stepped.velocity_x) * SIM_VELOCITY_BLEND_ALPHA;
            stepped.velocity_y +=
                (reported.velocity_y - stepped.velocity_y) * SIM_VELOCITY_BLEND_ALPHA;
        } else {
            stepped = result
                .snap_back
                .expect("snap-back result must retain its exact authoritative state");
        }
        sender.server_sim_state = Some(stepped);
        sender.server_sim_anchor_tick = Some(accepted_server_tick);
    } else {
        // First sample, long dt, or resync: re-seed from the accepted report.
        sender.server_sim_state = Some(reported);
        sender.server_sim_anchor_tick = Some(accepted_server_tick);
        // Only a genuinely fresh baseline (no prediction this frame) resets
        // the drift accumulator. A resync from a large single-frame jump
        // keeps the accumulated drift so per-frame teleport-style cheating
        // still snaps quickly.
        if !had_prediction {
            sender.movement_drift_x = 0.0;
            sender.movement_drift_y = 0.0;
        }
    }

    sender.server_sim_input_bits = matched_input.map(|input| input.bits);

    result
}

#[inline]
fn environment_rocket_active(input_bits: u16) -> bool {
    input_bits & (1 << 5) != 0
}

/// Continuum's safe-zone weapon request stops the post-movement vector.
/// Both canonical admission and drift observation must apply this effect
/// before the next logical step. Check the raw float tile, not its rounded
/// wire coordinate; the request bit alone cannot authorize a stop elsewhere.
fn apply_safe_zone_weapon_stop(
    state: &mut ServerSimMovement,
    map: &ZoneMap,
    input_bits: u16,
) -> bool {
    if input_bits & crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT == 0 {
        return true;
    }
    if !crate::zone::world_position_f32_in_safe_tile_for_map(
        map,
        state.position_x,
        state.position_y,
    ) {
        return false;
    }
    state.velocity_x = 0.0;
    state.velocity_y = 0.0;
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SimIntegration {
    state: ServerSimMovement,
    position_x: f32,
    position_y: f32,
    bounced: bool,
}

/// Replay the already-retained exact f32 orientation through every shared turn
/// operation. Current-protocol authority never inverts a 40-bin direction or
/// selects a representative inside its encoder preimage.
#[cfg(test)]
fn constrained_initial_orientation(
    preferred_orientation: f32,
    initial_direction: u8,
    inputs: &[ClientInputObservation],
    rotation_steps: &[f32],
) -> Option<f32> {
    exact_orientation_history_matches(
        preferred_orientation,
        initial_direction,
        inputs,
        rotation_steps,
    )
    .then_some(preferred_orientation)
}

/// The private parity simulation may carry sub-bin drift. Its truncating bin is
/// therefore not an admission witness; only the prior server-published
/// canonical direction is useful for diagnostic consistency checks.
fn prior_canonical_direction(sender: &ClientHandle, private_orientation: f32) -> u8 {
    sender
        .last_position
        .as_deref()
        .and_then(|position| position.get(1).copied())
        .filter(|direction| *direction < 40)
        .unwrap_or_else(|| crate::continuum_position::protocol_direction(private_orientation))
}

/// Diagnostic parity must start from the same exact authenticated witness as
/// admission. The private observer can intentionally carry blended/subpixel
/// state and must not relabel an already accepted direction history invalid.
fn diagnostic_orientation_history_matches(
    sender: &ClientHandle,
    private_orientation: f32,
    inputs: &[ClientInputObservation],
    rotation_steps: &[f32],
) -> bool {
    let (orientation, direction) = sender
        .canonical_movement_witness
        .filter(|witness| {
            witness.life_seq == sender.life_seq
                && Some(witness.sequence) == sender.last_position_sequence
        })
        .map_or_else(
            || {
                (
                    private_orientation,
                    prior_canonical_direction(sender, private_orientation),
                )
            },
            |witness| (witness.state.orientation, witness.direction),
        );
    exact_orientation_history_matches(orientation, direction, inputs, rotation_steps)
}

fn input_rotation_step(rotation_per_second: f32, dt: f32) -> f32 {
    rotation_per_second * dt
}

/// Shared live-frame reference-game collision integration. Each authenticated input
/// record performs exactly one complete-dt movement-leading X query and then
/// one complete-dt movement-leading Y query. An X bounce scales Y before Y
/// displacement is recomputed, while the bounce latch advances only after that
/// axis pair. The caller supplies only enemy brick tiles, matching the client's
/// friendly-brick pass-through rule.
fn integrate_with_collision(
    state: ServerSimMovement,
    start_x: f32,
    start_y: f32,
    input: PlayerMovementInput,
    tuning: PlayerMovementTuning,
    mut environment: PlayerMovementEnvironment,
    dt: f32,
    radius_px: f32,
    bounce_factor: f32,
    map: &ZoneMap,
    door_seeds: &[u8],
    enemy_brick_tiles: &[(usize, usize)],
    ship: u8,
) -> SimIntegration {
    // The reference game's game loop runs PlayerManager::Update before
    // ShipController::Update. Integrate/collide with the velocity retained
    // from the prior input; only then sample gravity and apply this input's
    // thrust/rotation/clamp for the next frame.
    let CollisionIntegration {
        position,
        velocity,
        bounced,
    } = integrate_player_collision(
        MovementVector::new(start_x, start_y),
        MovementVector::new(state.velocity_x, state.velocity_y),
        dt,
        bounce_factor,
        |candidate, axis, signed_delta| {
            axis_overlaps_blocked(
                candidate.x,
                candidate.y,
                radius_px,
                map,
                door_seeds,
                enemy_brick_tiles,
                axis,
                signed_delta,
            )
        },
    );
    let gravity = sample_wormhole_gravity_in_grid(
        position,
        arena_ship_wormhole_gravity(ship),
        arena_ship_wormhole_top_speed(ship),
        dt,
        TILE_SIZE_PX,
        WormholeTileGrid {
            tiles: &map.tiles,
            width: MAP_TILES,
            height: MAP_TILES,
            wormhole_tile: 220,
        },
    );
    environment.wormhole_velocity_delta = gravity.velocity_delta;
    environment.gravity_top_speed = gravity.top_speed;
    let movement = step_player_movement(
        PlayerMovementState {
            orientation: state.orientation,
            velocity,
            energy: state.energy,
            max_energy: state.max_energy,
        },
        input,
        tuning,
        environment,
        dt,
    );
    SimIntegration {
        state: ServerSimMovement {
            position_x: position.x,
            position_y: position.y,
            orientation: movement.state.orientation,
            velocity_x: movement.state.velocity.x,
            velocity_y: movement.state.velocity.y,
            energy: movement.state.energy,
            max_energy: movement.state.max_energy,
        },
        position_x: position.x,
        position_y: position.y,
        bounced,
    }
}

fn is_wormhole_tile(map: &ZoneMap, tx: i32, ty: i32) -> bool {
    if tx < 0 || ty < 0 {
        return false;
    }
    let tx = tx as usize;
    let ty = ty as usize;
    if tx >= MAP_TILES || ty >= MAP_TILES {
        return false;
    }
    map.tiles
        .get(ty * MAP_TILES + tx)
        .copied()
        // WORMHOLE_TILE_ID is 220 (Continuum tile id for wormholes).
        // Hardcoded here so the sim doesn't need a shared `tile.rs`
        // import.
        == Some(220)
}

fn axis_overlaps_blocked(
    x: f32,
    y: f32,
    radius_px: f32,
    map: &ZoneMap,
    door_seeds: &[u8],
    enemy_brick_tiles: &[(usize, usize)],
    axis: CollisionAxis,
    signed_delta: f32,
) -> bool {
    movement_axis_overlaps_blocked_tile(
        MovementVector::new(x, y),
        radius_px,
        MAP_TILES,
        TILE_SIZE_PX,
        axis,
        signed_delta,
        |tile_x, tile_y| {
            // Enemy bricks block; the player's own team's bricks
            // were filtered out by the caller, so anything in
            // `enemy_brick_tiles` is treated as solid.
            if enemy_brick_tiles
                .iter()
                .any(|&(bx, by)| bx as i32 == tile_x && by as i32 == tile_y)
            {
                return true;
            }
            if tile_x < 0 || tile_y < 0 {
                return true;
            }
            map.tiles
                .get(tile_y as usize * MAP_TILES + tile_x as usize)
                .is_none_or(|&tile| {
                    crate::zone::position_blocking_tile_for_door_seeds(tile, door_seeds)
                })
        },
    )
}

fn arena_bounce_factor() -> f32 {
    use crate::arena_settings::{CONTINUUM_SHORT_SETTINGS_OFFSET, arena_settings_packet};
    let bytes = arena_settings_packet();
    let raw = i16::from_le_bytes(
        bytes[CONTINUUM_SHORT_SETTINGS_OFFSET + 20..][..2]
            .try_into()
            .unwrap(),
    );
    16.0 / raw.max(1) as f32
}

/// Decode the exact authenticated input bits shared by replay and drift
/// observation. Whether a completed drift decision starts a correction
/// transaction is controlled independently by `movement_snapback`.
fn decode_input_bits(bits: u16) -> PlayerMovementInput {
    PlayerMovementInput {
        thrust: bits & (1 << 0) != 0,
        reverse: bits & (1 << 1) != 0,
        rotate_left: bits & (1 << 2) != 0,
        rotate_right: bits & (1 << 3) != 0,
        afterburner_held: bits & (1 << 4) != 0,
    }
}

/// Per-player live movement values. `live_values=false` is retained only for
/// old zero-upgrade fixtures; production always supplies the exact clamped
/// values so partial-cap reversals are representable.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PlayerMovementUpgrades {
    pub(crate) thrust: f32,
    pub(crate) top_speed: f32,
    pub(crate) rotation: f32,
    pub(crate) recharge: f32,
    pub(crate) live_values: bool,
}

pub(crate) fn movement_upgrades_for_client(
    client: &crate::state::ClientHandle,
) -> PlayerMovementUpgrades {
    movement_upgrades_from_tuning(client.movement_tuning_values_for_replay())
}

/// Live upgrades from (thrust, top speed, rotation, recharge), the order of
/// `ClientHandle::movement_tuning_values_for_replay`.
pub(crate) fn movement_upgrades_from_tuning(
    (thrust, top_speed, rotation, recharge): (f32, f32, f32, f32),
) -> PlayerMovementUpgrades {
    PlayerMovementUpgrades {
        thrust,
        top_speed,
        rotation,
        recharge,
        live_values: true,
    }
}

fn movement_tuning_from_ship(ship: u8, upgrades: PlayerMovementUpgrades) -> PlayerMovementTuning {
    let max_thrust = arena_ship_max_thrust_pixels_per_second_squared(ship);
    let max_speed = arena_ship_max_speed_pixels_per_second(ship);
    let initial_thrust = arena_ship_initial_thrust_pixels_per_second_squared(ship);
    let initial_top_speed = arena_ship_initial_top_speed_pixels_per_second(ship);
    let initial_rotation = arena_ship_initial_rotation_per_second(ship);
    let base_thrust = if upgrades.live_values {
        upgrades.thrust
    } else {
        initial_thrust
    };
    let base_top_speed = if upgrades.live_values {
        upgrades.top_speed
    } else {
        initial_top_speed
    };
    let rotation_per_second = if upgrades.live_values {
        upgrades.rotation
    } else {
        initial_rotation
    };
    let recharge_per_second = if upgrades.live_values {
        upgrades.recharge
    } else {
        arena_ship_initial_recharge_per_second(ship)
    };

    PlayerMovementTuning {
        rotation_per_second,
        shutdown_rotation_per_second: 0.1,
        base_thrust_acceleration: base_thrust,
        afterburner_thrust_acceleration: max_thrust,
        rocket_thrust_acceleration: arena_rocket_thrust_pixels_per_second_squared(),
        base_top_speed,
        afterburner_top_speed: max_speed,
        rocket_top_speed: arena_rocket_speed_pixels_per_second(),
        afterburner_energy_per_second: arena_ship_afterburner_energy_per_second(ship),
        recharge_per_second,
        turret_thrust_penalty: arena_ship_turret_thrust_penalty(ship),
        turret_speed_penalty: arena_ship_turret_speed_penalty(ship),
        repel_top_speed: arena_repel_speed_pixels_per_second(),
        flagger_thrust_adjustment: arena_flagger_thrust_adjustment(),
        flagger_speed_adjustment: arena_flagger_speed_adjustment(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bounce_waiver_requires_an_exact_reported_tail() {
        let state = ServerSimMovement {
            position_x: 10_889.0,
            position_y: 8_150.0,
            orientation: 0.25,
            velocity_x: -375.0,
            velocity_y: -1.4,
            energy: 1_700.0,
            max_energy: 1_700.0,
        };
        let witness = CanonicalPositionWitness {
            sequence: 0,
            life_seq: 1,
            state,
            wire_x: 10_889,
            wire_y: 8_150,
            wire_velocity_x: -3_750,
            wire_velocity_y: -14,
            direction: 10,
            door_seed: 255,
            door_phase_tick: 4_000,
        };
        let replay = CanonicalPositionReplay {
            witnesses: vec![witness],
            bounced: true,
            relocated: false,
            wormhole_relocated: false,
            safe_zone_weapon_stopped: false,
            receipt_door_relocation_branches: Vec::new(),
        };
        let exact = crate::continuum_position::ClientPositionCanonicalState::from_f32(
            state.position_x,
            state.position_y,
            state.velocity_x,
            state.velocity_y,
            state.orientation,
        );
        assert!(replay.proves_exact_f32_bounce_report(exact, 10, 10_889, 8_150, -3_750, -14));
        assert!(!replay.proves_exact_f32_bounce_report(exact, 10, 10_889, 8_150, -3_749, -14));
        assert!(
            !CanonicalPositionReplay {
                bounced: false,
                ..replay
            }
            .proves_exact_f32_bounce_report(exact, 10, 10_889, 8_150, -3_750, -14)
        );
    }

    #[test]
    fn exact_replay_never_widens_prior_raw_f32_into_forged_thrust_velocity() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().expect("state");
        let map = locked.zone_map();
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.server_thrust_acceleration = 170.0;
        client.server_top_speed = 1_000.0;
        client.server_rotation_per_second = 0.0;
        client.server_recharge_per_second = 0.0;
        let orientation = 0.25_f32;
        let initial = ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation,
            velocity_x: 0.049_999_997,
            velocity_y: 0.0,
            energy: 1_700.0,
            max_energy: 1_700.0,
        };
        let mut anchor = CanonicalPositionWitness {
            sequence: 100,
            life_seq: client.life_seq,
            state: initial,
            wire_x: position_wire_component(initial.position_x),
            wire_y: position_wire_component(initial.position_y),
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                initial.velocity_x,
            ),
            wire_velocity_y: 0,
            direction: crate::continuum_position::protocol_direction(orientation),
            door_seed: 0,
            door_phase_tick: 0,
        };
        assert_eq!(anchor.wire_velocity_x, 0);
        client.last_position_sequence = Some(anchor.sequence);
        client.last_position_server_tick = Some(1_000);
        client.canonical_movement_witness = Some(anchor);
        let thrust = ClientInputObservation {
            bits: 1,
            direction: anchor.direction,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 101,
                input: thrust,
                door_seed: 0,
                door_phase_tick: 0,
            });
        let upgrades = PlayerMovementUpgrades {
            thrust: 170.0,
            top_speed: 1_000.0,
            rotation: 0.0,
            recharge: 0.0,
            live_values: true,
        };
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(map);
        let first = replay_canonical_position_gap(
            client,
            map,
            false,
            false,
            false,
            upgrades,
            &[],
            101,
            1_001,
            spawn_x,
            spawn_y,
        )
        .expect("first exact thrust step");
        anchor = *first.witnesses.last().expect("first tail");
        assert_eq!(anchor.wire_velocity_x, 29);

        client.last_position_sequence = Some(101);
        client.last_position_server_tick = Some(1_001);
        client.canonical_movement_witness = Some(anchor);
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 102,
                input: thrust,
                door_seed: 0,
                door_phase_tick: 0,
            });
        let second = replay_canonical_position_gap(
            client,
            map,
            false,
            false,
            false,
            upgrades,
            &[],
            102,
            1_002,
            spawn_x,
            spawn_y,
        )
        .expect("second exact thrust step");
        let honest = *second.witnesses.last().expect("second tail");
        assert_eq!(honest.wire_velocity_x, 57);
        let honest_state = crate::continuum_position::ClientPositionCanonicalState::from_f32(
            honest.state.position_x,
            honest.state.position_y,
            honest.state.velocity_x,
            honest.state.velocity_y,
            honest.state.orientation,
        );
        assert_eq!(
            second
                .matching_exact_branch_witnesses(
                    honest_state,
                    honest.direction,
                    honest.wire_x,
                    honest.wire_y,
                    honest.wire_velocity_x,
                    honest.wire_velocity_y,
                )
                .len(),
            1,
        );
        let forged_state = crate::continuum_position::ClientPositionCanonicalState::from_f32(
            honest.state.position_x,
            honest.state.position_y,
            5.8,
            honest.state.velocity_y,
            honest.state.orientation,
        );
        assert!(
            second
                .matching_exact_branch_witnesses(
                    forged_state,
                    honest.direction,
                    honest.wire_x,
                    honest.wire_y,
                    58,
                    honest.wire_velocity_y,
                )
                .is_empty()
        );

        let open_index = map
            .tiles
            .iter()
            .enumerate()
            .find_map(|(index, tile)| {
                let tile_x = index % MAP_TILES;
                let tile_y = index / MAP_TILES;
                (*tile == 0
                    && (16..MAP_TILES - 16).contains(&tile_x)
                    && (16..MAP_TILES - 16).contains(&tile_y))
                .then_some((tile_x, tile_y))
            })
            .expect("map has an ordinary non-safe empty tile");
        let outside = ServerSimMovement {
            position_x: open_index.0 as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
            position_y: open_index.1 as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_700.0,
            max_energy: 1_700.0,
        };
        let outside_anchor = CanonicalPositionWitness {
            sequence: 200,
            life_seq: client.life_seq,
            state: outside,
            wire_x: position_wire_component(outside.position_x),
            wire_y: position_wire_component(outside.position_y),
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 0,
            door_phase_tick: 0,
        };
        assert!(!crate::zone::world_position_in_safe_tile_for_map(
            map,
            outside_anchor.wire_x,
            outside_anchor.wire_y,
        ));
        client.last_position_sequence = Some(outside_anchor.sequence);
        client.last_position_server_tick = Some(2_000);
        client.canonical_movement_witness = Some(outside_anchor);
        client.recent_input_records.clear();
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 201,
                input: ClientInputObservation {
                    bits: crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT,
                    direction: 0,
                },
                door_seed: 0,
                door_phase_tick: 0,
            });
        assert!(
            replay_canonical_position_gap(
                client,
                map,
                false,
                false,
                false,
                upgrades,
                &[],
                201,
                2_001,
                spawn_x,
                spawn_y,
            )
            .is_none(),
            "a client stop bit outside server-owned safe geometry must fail closed",
        );
    }

    #[test]
    fn exact_safe_stop_uses_raw_subpixel_tile_before_wire_rounding() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().expect("state");
        let map = locked.zone_map();
        let (safe_tile_x, safe_tile_y) = map
            .tiles
            .windows(2)
            .enumerate()
            .find_map(|(index, pair)| {
                let x = index % MAP_TILES;
                let y = index / MAP_TILES;
                (x + 1 < MAP_TILES && pair[0] == crate::zone::SAFE_TILE_ID && pair[1] == 0)
                    .then_some((x, y))
            })
            .expect("map has a safe tile with an ordinary right neighbor");
        let raw_x = (safe_tile_x + 1) as f32 * TILE_SIZE_PX - 0.125;
        let raw_y = safe_tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5;
        let wire_x = position_wire_component(raw_x);
        let wire_y = position_wire_component(raw_y);
        assert!(crate::zone::world_position_f32_in_safe_tile_for_map(
            map, raw_x, raw_y,
        ));
        assert!(
            !crate::zone::world_position_in_safe_tile_for_map(map, wire_x, wire_y),
            "the legacy rounded coordinate must demonstrate the adjacent-tile boundary"
        );

        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(map);
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.server_thrust_acceleration = 0.0;
        client.server_top_speed = 1_000.0;
        client.server_rotation_per_second = 0.0;
        client.server_recharge_per_second = 0.0;
        let initial = ServerSimMovement {
            position_x: raw_x,
            position_y: raw_y,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_700.0,
            max_energy: 1_700.0,
        };
        let anchor = CanonicalPositionWitness {
            sequence: 300,
            life_seq: client.life_seq,
            state: initial,
            wire_x,
            wire_y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.last_position_sequence = Some(anchor.sequence);
        client.last_position_server_tick = Some(3_000);
        client.canonical_movement_witness = Some(anchor);
        client.recent_input_records.clear();
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 301,
                input: ClientInputObservation {
                    bits: crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT,
                    direction: 0,
                },
                door_seed: 0,
                door_phase_tick: 0,
            });

        let replay = replay_canonical_position_gap(
            client,
            map,
            false,
            false,
            false,
            PlayerMovementUpgrades {
                thrust: 0.0,
                top_speed: 1_000.0,
                rotation: 0.0,
                recharge: 0.0,
                live_values: true,
            },
            &[],
            301,
            3_001,
            spawn_x,
            spawn_y,
        )
        .expect("raw canonical safe geometry admits the exact stop");
        assert!(replay.safe_zone_weapon_stopped);
        let tail = replay.witnesses.last().expect("stop tail");
        assert_eq!(tail.state.position_x.to_bits(), raw_x.to_bits());
        assert_eq!(tail.state.position_y.to_bits(), raw_y.to_bits());
        assert_eq!(tail.state.velocity_x.to_bits(), 0.0_f32.to_bits());
        assert_eq!(tail.state.velocity_y.to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn repel_sequence_window_matches_shared_logical_timer_and_wraps() {
        let application = CanonicalRepelApplication {
            sequence: u16::MAX - 1,
            origin_x: 0.0,
            origin_y: 0.0,
            speed: 500.0,
            radius: 500.0,
            expected_velocity_x: 500.0,
            expected_velocity_y: 0.0,
            delivered_snapshot_tick: 1_076,
            active_steps: 135,
        };
        let window = canonical_repel_sequence_window(application);
        let active_steps = window.1.wrapping_sub(window.0) - 1;
        assert_eq!(active_steps, 135);
        assert!(repel_sequence_window_contains(
            window,
            application.sequence.wrapping_add(1)
        ));
        assert!(repel_sequence_window_contains(
            window,
            application.sequence.wrapping_add(active_steps)
        ));
        assert!(!repel_sequence_window_contains(
            window,
            application.sequence
        ));
        assert!(!repel_sequence_window_contains(
            window,
            application.sequence.wrapping_add(active_steps + 1)
        ));

        assert_eq!(
            window,
            crate::gameplay_rules::repel_sequence_window(application.sequence, 224)
        );
    }

    #[test]
    fn lost_first_repel_report_replays_the_retained_post_movement_boundary_across_wrap() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().expect("state");
        let map = locked.zone_map();
        let client = locked.clients.get_mut(&player_id).expect("client");
        let anchor_sequence = u16::MAX - 1;
        let application_sequence = u16::MAX;
        let tail_sequence = 0;
        let initial = ServerSimMovement {
            position_x: 600.0,
            position_y: 700.0,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_700.0,
            max_energy: 1_700.0,
        };
        client.last_position_sequence = Some(anchor_sequence);
        client.last_position_server_tick = Some(1_000);
        client.canonical_movement_witness = Some(CanonicalPositionWitness {
            sequence: anchor_sequence,
            life_seq: client.life_seq,
            state: initial,
            wire_x: 600,
            wire_y: 700,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 0,
            door_phase_tick: 0,
        });
        for sequence in [application_sequence, tail_sequence] {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input: ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: 0,
                    door_phase_tick: 0,
                });
        }
        let upgrades = PlayerMovementUpgrades::default();
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(map);
        let application = CanonicalRepelApplication {
            sequence: application_sequence,
            origin_x: 500.0,
            origin_y: 700.0,
            speed: 500.0,
            radius: 500.0,
            expected_velocity_x: 500.0,
            expected_velocity_y: 0.0,
            delivered_snapshot_tick: 1_000,
            active_steps: 600,
        };
        let replay = replay_canonical_position_gap_with_repel_application(
            client,
            map,
            false,
            false,
            false,
            upgrades,
            &[],
            tail_sequence,
            1_002,
            spawn_x,
            spawn_y,
            None,
            Some(application),
        )
        .expect("two-step replay with a retained first-tail Repel boundary");
        let application_tail = replay.witnesses[0];
        let final_tail = replay.witnesses[1];
        assert_eq!(application_tail.sequence, application_sequence);
        assert_eq!(
            application_tail.state.position_x.to_bits(),
            600.0_f32.to_bits()
        );
        assert_eq!(
            application_tail.state.velocity_x.to_bits(),
            500.0_f32.to_bits()
        );
        let expected_x = 600.0_f32 + 500.0_f32 * OBSERVED_POSITION_INTERVAL_SECONDS;
        assert_eq!(final_tail.state.position_x.to_bits(), expected_x.to_bits());
        assert_eq!(final_tail.state.velocity_x.to_bits(), 500.0_f32.to_bits());

        let late_application = replay_canonical_position_gap_with_repel_application(
            client,
            map,
            false,
            false,
            false,
            upgrades,
            &[],
            tail_sequence,
            1_002,
            spawn_x,
            spawn_y,
            None,
            Some(CanonicalRepelApplication {
                sequence: tail_sequence,
                ..application
            }),
        )
        .expect("alternate marker remains a well-defined but different replay");
        let late_tail = late_application.witnesses[1];
        assert_eq!(late_tail.state.position_x.to_bits(), 600.0_f32.to_bits());
        assert_ne!(
            late_tail.state.position_x.to_bits(),
            final_tail.state.position_x.to_bits()
        );

        let exact = crate::continuum_position::ClientPositionCanonicalState::from_f32(
            final_tail.state.position_x,
            final_tail.state.position_y,
            final_tail.state.velocity_x,
            final_tail.state.velocity_y,
            final_tail.state.orientation,
        );
        assert_eq!(
            replay
                .matching_exact_branch_witnesses(
                    exact,
                    final_tail.direction,
                    final_tail.wire_x,
                    final_tail.wire_y,
                    final_tail.wire_velocity_x,
                    final_tail.wire_velocity_y,
                )
                .len(),
            1
        );
        assert!(
            late_application
                .matching_exact_branch_witnesses(
                    exact,
                    final_tail.direction,
                    final_tail.wire_x,
                    final_tail.wire_y,
                    final_tail.wire_velocity_x,
                    final_tail.wire_velocity_y,
                )
                .is_empty(),
            "changing the application sequence cannot authorize the retained exact tail"
        );
    }

    fn synthetic_dynamic_door_map(tile_x: usize, tile_y: usize, tile_id: u8) -> ZoneMap {
        let mut tiles = vec![0; MAP_TILES * MAP_TILES];
        tiles[tile_y * MAP_TILES + tile_x] = tile_id;
        ZoneMap {
            zone_id: "dynamic-door-test".into(),
            display_name: "Dynamic Door Test".into(),
            filename: "dynamic-door-test.lvl".into(),
            news_filename: "".into(),
            news_bytes: (&[][..]).into(),
            tiles,
            no_flag_drop_tiles: Default::default(),
            checksum: 0,
            sha256: [0; crate::zone_wire::SHA256_BYTES],
            compressed: Vec::new(),
            map_checksum: 0,
            continuum_crc32: 0,
            flags: Vec::new(),
            objective_profile: crate::objectives::ServerObjectiveProfile::disabled(),
            lvz_files: Vec::new(),
            settings_packet: crate::arena_settings::arena_settings_packet_for_zone(
                crate::zone::CHAOS_ZONE_ID,
            )
            .expect("chaos settings")
            .into(),
            brick_span: 10,
            resource_usage: Default::default(),
        }
    }

    #[test]
    fn canonical_private_live_frame_uses_one_full_dt_pair_for_wall_brick_door_and_bounds() {
        use crate::state::ServerSimMovement;

        let ship = 0;
        let tuning = movement_tuning_from_ship(ship, PlayerMovementUpgrades::default());
        let initial = ServerSimMovement {
            position_x: 31.0,
            position_y: 56.0,
            orientation: 0.0,
            velocity_x: 200.0,
            velocity_y: 0.0,
            energy: 1_500.0,
            max_energy: 1_700.0,
        };
        let run = |map: &ZoneMap, door_seeds: &[u8], bricks: &[(usize, usize)]| {
            integrate_with_collision(
                initial,
                initial.position_x,
                initial.position_y,
                decode_input_bits(0),
                tuning,
                PlayerMovementEnvironment::default(),
                1.0 / 60.0,
                14.0,
                0.5,
                map,
                door_seeds,
                bricks,
                ship,
            )
        };

        let wall = synthetic_dynamic_door_map(3, 3, 1);
        let wall_result = run(&wall, &[], &[]);
        assert!(wall_result.bounced);
        assert_eq!(wall_result.position_x.to_bits(), 31.0_f32.to_bits());
        assert_eq!(wall_result.position_y.to_bits(), 56.0_f32.to_bits());
        assert_eq!(
            wall_result.state.velocity_x.to_bits(),
            (-100.0_f32).to_bits()
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(wall_result.state.velocity_x),
            -1_000,
        );

        let empty = synthetic_dynamic_door_map(3, 3, 0);
        let brick_result = run(&empty, &[], &[(3, 3)]);
        assert_eq!(brick_result.position_x.to_bits(), 31.0_f32.to_bits());
        assert_eq!(
            brick_result.state.velocity_x.to_bits(),
            (-100.0_f32).to_bits()
        );

        let door = synthetic_dynamic_door_map(3, 3, 162);
        let closed = run(&door, &[1], &[]);
        assert_eq!(closed.position_x.to_bits(), 31.0_f32.to_bits());
        assert_eq!(closed.state.velocity_x.to_bits(), (-100.0_f32).to_bits());
        let open = run(&door, &[0], &[]);
        assert_eq!(open.position_x.to_bits(), 0x4209_5555);
        assert_eq!(open.state.velocity_x.to_bits(), 200.0_f32.to_bits());

        let boundary_initial = ServerSimMovement {
            position_x: 15.0,
            velocity_x: -200.0,
            ..initial
        };
        let boundary = integrate_with_collision(
            boundary_initial,
            boundary_initial.position_x,
            boundary_initial.position_y,
            decode_input_bits(0),
            tuning,
            PlayerMovementEnvironment::default(),
            1.0 / 60.0,
            14.0,
            0.5,
            &empty,
            &[],
            &[],
            ship,
        );
        assert_eq!(boundary.position_x.to_bits(), 15.0_f32.to_bits());
        assert_eq!(boundary.state.velocity_x.to_bits(), 100.0_f32.to_bits());
    }

    #[test]
    fn correction_shadow_replay_covers_delayed_wall_door_brick_and_gravity() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().expect("state");
        let chaos = locked.zone_map();
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.player.ship = 0;
        let sequence = 11;
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence,
                input: ClientInputObservation {
                    bits: 0,
                    direction: 0,
                },
                door_seed: 1,
                door_phase_tick: 100,
            });
        let initial = ServerSimMovement {
            position_x: 31.0,
            position_y: 56.0,
            orientation: 0.0,
            velocity_x: 200.0,
            velocity_y: 0.0,
            energy: 1_500.0,
            max_energy: 1_700.0,
        };
        let witness = CanonicalPositionWitness {
            sequence: 10,
            life_seq: client.life_seq,
            state: initial,
            wire_x: 31,
            wire_y: 56,
            wire_velocity_x: 2_000,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 1,
            door_phase_tick: 100,
        };
        let replay = |map: &ZoneMap, bricks: &[(usize, usize)]| {
            replay_canonical_position_gap_from_witness(
                client,
                witness,
                100,
                map,
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                bricks,
                sequence,
                101,
                8_200,
                8_200,
                None,
                None,
            )
            .expect("one-step correction shadow replay")
        };

        let wall = synthetic_dynamic_door_map(3, 3, 1);
        let wall_replay = replay(&wall, &[]);
        let wall_tail = *wall_replay.witnesses.last().expect("wall tail");
        assert!(wall_replay.bounced);
        assert!(wall_tail.state.velocity_x < 0.0);

        let empty = synthetic_dynamic_door_map(3, 3, 0);
        let brick_tail = *replay(&empty, &[(3, 3)])
            .witnesses
            .last()
            .expect("brick tail");
        assert!(brick_tail.state.velocity_x < 0.0);

        let door = synthetic_dynamic_door_map(3, 3, 162);
        let door_tail = *replay(&door, &[]).witnesses.last().expect("door tail");
        assert!(door_tail.state.velocity_x < 0.0);

        let wormhole_index = chaos
            .tiles
            .iter()
            .position(|&tile| tile == 220)
            .expect("chaos wormhole");
        let wormhole_x = wormhole_index % MAP_TILES;
        let wormhole_y = wormhole_index / MAP_TILES;
        let gravity_state = ServerSimMovement {
            position_x: (wormhole_x as f32 + 2.5) * TILE_SIZE_PX + 64.0,
            position_y: (wormhole_y as f32 + 2.5) * TILE_SIZE_PX,
            velocity_x: 0.0,
            ..initial
        };
        let gravity_witness = CanonicalPositionWitness {
            state: gravity_state,
            wire_x: gravity_state.position_x.round() as u16,
            wire_y: gravity_state.position_y.round() as u16,
            wire_velocity_x: 0,
            ..witness
        };
        let gravity_tail = replay_canonical_position_gap_from_witness(
            client,
            gravity_witness,
            100,
            chaos,
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            sequence,
            101,
            8_200,
            8_200,
            None,
            None,
        )
        .and_then(|replay| replay.witnesses.last().copied())
        .expect("gravity correction shadow tail");
        assert!(gravity_tail.state.velocity_x < 0.0);
    }

    #[test]
    fn canonical_replay_owns_exact_wormhole_terminal_in_the_entering_step() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        const WORMHOLE_X: usize = 3;
        const WORMHOLE_Y: usize = 3;
        let map = synthetic_dynamic_door_map(WORMHOLE_X, WORMHOLE_Y, 220);
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.player.ship = 0;
        client.server_thrust_acceleration = 0.0;
        client.server_top_speed = 1_000.0;
        client.server_rotation_per_second = 0.0;
        client.server_recharge_per_second = 0.0;
        let previous_sequence = 40;
        let current_sequence = previous_sequence + 1;
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: current_sequence,
                input: ClientInputObservation {
                    bits: 0,
                    direction: 10,
                },
                door_seed: 0,
                door_phase_tick: 100,
            });
        let initial = ServerSimMovement {
            position_x: WORMHOLE_X as f32 * TILE_SIZE_PX - 1.0,
            position_y: WORMHOLE_Y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
            orientation: 0.25,
            velocity_x: 120.0,
            velocity_y: 0.0,
            energy: 1_699.75,
            max_energy: 1_700.0,
        };
        let witness = CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: initial,
            wire_x: position_wire_component(initial.position_x),
            wire_y: position_wire_component(initial.position_y),
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                initial.velocity_x,
            ),
            wire_velocity_y: 0,
            direction: 10,
            door_seed: 0,
            door_phase_tick: 100,
        };
        let replay = replay_canonical_position_gap_from_witness(
            client,
            witness,
            100,
            &map,
            false,
            false,
            false,
            movement_upgrades_for_client(client),
            &[],
            current_sequence,
            101,
            8_200,
            8_200,
            None,
            None,
        )
        .expect("the entering step has complete canonical input");
        let tail = *replay.witnesses.last().expect("wormhole terminal tail");
        assert!(replay.wormhole_relocated);
        assert!(
            !replay.relocated,
            "wormhole authority is distinct from doors"
        );
        assert_eq!(tail.state.position_x.to_bits(), 8_200.0_f32.to_bits());
        assert_eq!(tail.state.position_y.to_bits(), 8_200.0_f32.to_bits());
        assert_eq!(tail.state.velocity_x.to_bits(), 0.0_f32.to_bits());
        assert_eq!(tail.state.velocity_y.to_bits(), 0.0_f32.to_bits());
        assert_eq!(
            tail.state.energy.to_bits(),
            crate::gameplay_rules::wormhole_energy_after(initial.energy).to_bits(),
        );
        let exact = crate::continuum_position::ClientPositionCanonicalState::from_f32(
            tail.state.position_x,
            tail.state.position_y,
            tail.state.velocity_x,
            tail.state.velocity_y,
            tail.state.orientation,
        );
        assert!(
            replay
                .matching_exact_wormhole_branch_witnesses(
                    exact,
                    tail.direction,
                    tail.wire_x,
                    tail.wire_y,
                    tail.wire_velocity_x,
                    tail.wire_velocity_y,
                )
                .is_some(),
            "only the exact server-replayed terminal is wormhole authority",
        );
        assert!(
            replay
                .matching_exact_wormhole_branch_witnesses(
                    crate::continuum_position::ClientPositionCanonicalState::from_f32(
                        8_200.0_f32.next_up(),
                        tail.state.position_y,
                        tail.state.velocity_x,
                        tail.state.velocity_y,
                        tail.state.orientation,
                    ),
                    tail.direction,
                    tail.wire_x,
                    tail.wire_y,
                    tail.wire_velocity_x,
                    tail.wire_velocity_y,
                )
                .is_none(),
            "a same-wire forged raw float cannot borrow the terminal",
        );
    }

    #[test]
    fn door_relocation_miss_diagnostic_requires_an_authenticated_close_transition() {
        assert!(!door_transition_closes_candidate(0b0000_0000, 0b0000_0000));
        assert!(!door_transition_closes_candidate(0b1111_1111, 0b1111_1111));
        assert!(!door_transition_closes_candidate(0b0000_0001, 0b0000_0000));
        assert!(door_transition_closes_candidate(0b0000_0000, 0b0000_0001));
        assert!(door_transition_closes_candidate(0b0000_0101, 0b0000_0110));
    }

    #[test]
    fn canonical_gap_replay_proves_open_cross_then_close_and_rejects_closed_cross_atomically() {
        use crate::state::{CanonicalPositionWitness, SequencedInputObservation};

        const DOOR_TILE: u8 = 162;
        const DOOR_X: usize = 480;
        const DOOR_Y: usize = 500;
        let map = synthetic_dynamic_door_map(DOOR_X, DOOR_Y, DOOR_TILE);
        let anchor = 0x7fff_ff00;
        let door_state = crate::doors::ClientDoorState::for_challenge(anchor);
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (open_phase, closed_phase) = (0_u32..512)
            .find_map(|count| {
                let before_tick =
                    crate::continuum::continuum_tick_add(anchor, count.saturating_mul(delay));
                let after_tick = crate::continuum::continuum_tick_add(before_tick, delay);
                let phases = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    before_tick,
                    after_tick,
                )?;
                let before = *phases.first()?;
                let after = *phases.last()?;
                (!crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, before.seed)
                    && crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, after.seed))
                .then_some((before, after))
            })
            .expect("wrapped canonical stream reaches an open-to-closed phase");

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.player.ship = 0;
        client.door_state = Some(door_state);
        let radius = arena_ship_radius_pixels(client.player.ship);
        let start_x = DOOR_X as f32 * TILE_SIZE_PX - radius - 1.0;
        let start_y = DOOR_Y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5;
        let initial = ServerSimMovement {
            position_x: start_x,
            position_y: start_y,
            orientation: 0.0,
            velocity_x: arena_ship_initial_top_speed_pixels_per_second(client.player.ship),
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let previous_sequence = u16::MAX - 20;
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(open_phase.started_tick);
        let previous_witness = CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: initial,
            wire_x: start_x.round() as u16,
            wire_y: start_y.round() as u16,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                initial.velocity_x,
            ),
            wire_velocity_y: 0,
            direction: 0,
            door_seed: open_phase.seed,
            door_phase_tick: open_phase.started_tick,
        };
        client
            .canonical_position_witnesses
            .push_back(previous_witness);
        client.canonical_movement_witness = Some(previous_witness);
        let coast = ClientInputObservation {
            bits: 0,
            direction: 0,
        };
        // Thirty-two open steps carry the entire circle hull across the tile;
        // the following closed-phase tail occurs only after it is clear.
        for offset in 1..=33_u16 {
            let phase = if offset <= 32 {
                open_phase
            } else {
                closed_phase
            };
            client
                .recent_input_records
                .push_back(SequencedInputObservation {
                    sequence: previous_sequence.wrapping_add(offset),
                    input: coast,
                    door_seed: phase.seed,
                    door_phase_tick: phase.started_tick,
                });
        }
        let before_history = client.canonical_position_witnesses.clone();
        let before_sim = client.server_sim_state;
        let current_sequence = previous_sequence.wrapping_add(33);
        let replay = replay_canonical_position_gap(
            client,
            &map,
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            current_sequence,
            closed_phase.started_tick,
            8_200,
            8_200,
        )
        .expect("complete wrapped history replays");
        assert!(!replay.relocated, "the hull cleared before the close");
        let tail = *replay.witnesses.last().expect("tail witness");
        assert!(
            tail.state.position_x - radius > (DOOR_X + 1) as f32 * TILE_SIZE_PX,
            "open records must carry the whole hull past the door"
        );
        assert!(
            crate::continuum_position::reported_position_path_hull_overlaps_blocked_tile(
                start_x.round() as u16,
                start_y.round() as u16,
                tail.wire_x,
                tail.wire_y,
                radius,
                MAP_TILES,
                TILE_SIZE_PX,
                |tile_x, tile_y| map
                    .tiles
                    .get(tile_y * MAP_TILES + tile_x)
                    .is_none_or(|&tile| crate::zone::position_blocking_tile_with_door_seed(
                        tile,
                        closed_phase.seed
                    )),
            )
        );
        assert!(replay.final_matches_report(
            tail.direction,
            tail.wire_x,
            tail.wire_y,
            tail.wire_velocity_x,
            tail.wire_velocity_y,
        ));
        assert_eq!(client.canonical_position_witnesses, before_history);
        assert_eq!(
            client.server_sim_state, before_sim,
            "prospective replay is pure"
        );

        // Re-run the same claimed endpoint with every step closed. Shared
        // collision bounces before the door, so the across-door report is no
        // longer a quantization-compatible tail witness.
        client.recent_input_records.clear();
        client.canonical_position_witnesses.clear();
        let closed_witness = CanonicalPositionWitness {
            door_seed: closed_phase.seed,
            door_phase_tick: closed_phase.started_tick,
            ..before_history[0]
        };
        client
            .canonical_position_witnesses
            .push_back(closed_witness);
        client.canonical_movement_witness = Some(closed_witness);
        client.last_position_server_tick = Some(closed_phase.started_tick);
        for offset in 1..=33_u16 {
            client
                .recent_input_records
                .push_back(SequencedInputObservation {
                    sequence: previous_sequence.wrapping_add(offset),
                    input: coast,
                    door_seed: closed_phase.seed,
                    door_phase_tick: closed_phase.started_tick,
                });
        }
        let closed_replay = replay_canonical_position_gap(
            client,
            &map,
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            current_sequence,
            closed_phase.started_tick,
            8_200,
            8_200,
        )
        .expect("closed history replays to a bounce");
        assert!(closed_replay.bounced);
        assert!(!closed_replay.final_matches_report(
            tail.direction,
            tail.wire_x,
            tail.wire_y,
            tail.wire_velocity_x,
            tail.wire_velocity_y,
        ));
    }

    #[test]
    fn constrained_direction_history_rejects_adjacent_bin_bias_and_handles_wrap() {
        let no_turn = ClientInputObservation {
            bits: 0,
            direction: 39,
        };
        assert!(
            constrained_initial_orientation(0.987_49, 39, &[no_turn; 32], &[0.0; 32]).is_some()
        );

        assert_eq!(
            constrained_initial_orientation(
                0.760_42,
                30,
                &[ClientInputObservation {
                    bits: 0,
                    direction: 30,
                }],
                &[0.0]
            ),
            Some(0.760_42),
            "a valid canonical sub-bin orientation remains exact instead of snapping to its bin center"
        );

        let mut biased = [no_turn; 32];
        for (index, input) in biased.iter_mut().enumerate() {
            input.direction = (39 + index as u8) % 40;
        }
        assert_eq!(
            constrained_initial_orientation(0.987_49, 39, &biased, &[0.0; 32]),
            None,
            "a neighboring bin cannot accumulate unauthorized no-turn rotation"
        );

        let wrap_right = [
            ClientInputObservation {
                bits: 8,
                direction: 39,
            },
            ClientInputObservation {
                bits: 8,
                direction: 0,
            },
        ];
        assert!(constrained_initial_orientation(0.98, 39, &wrap_right, &[0.01, 0.01]).is_some());
        let honest_after_private_bin_drift = [ClientInputObservation {
            bits: 8,
            direction: 0,
        }];
        assert_eq!(
            constrained_initial_orientation(0.012_6, 39, &honest_after_private_bin_drift, &[0.01],),
            None,
            "a drifted private float cannot be inverted through a published bin"
        );
        assert_eq!(
            constrained_initial_orientation(0.012_6, 1, &honest_after_private_bin_drift, &[0.01],),
            None,
            "this fixture detects the old false-positive private-bin anchor"
        );

        let opposite = [
            ClientInputObservation {
                bits: 4,
                direction: 0,
            },
            ClientInputObservation {
                bits: 4,
                direction: 1,
            },
        ];
        assert_eq!(
            constrained_initial_orientation(0.01, 0, &opposite, &[0.01, 0.01]),
            None,
        );

        let first_right = [ClientInputObservation {
            bits: 8,
            direction: 0,
        }];
        let rotation = [0.5 / 60.0];
        assert!(constrained_initial_orientation(0.995, 39, &first_right, &rotation).is_some());
        assert!(
            constrained_initial_orientation(
                0.995,
                39,
                &[ClientInputObservation {
                    bits: 8,
                    direction: 1,
                }],
                &rotation,
            )
            .is_none(),
            "an attacker cannot reselect a faster sub-bin inside direction 39"
        );

        let second_orientation = (0.995 + rotation[0]).rem_euclid(1.0);
        assert!(
            constrained_initial_orientation(
                second_orientation,
                0,
                &[ClientInputObservation {
                    bits: 8,
                    direction: 0,
                }],
                &rotation,
            )
            .is_some(),
            "consecutive right turns remain bounded by the configured rotation"
        );
        assert!(
            constrained_initial_orientation(
                second_orientation,
                0,
                &[ClientInputObservation {
                    bits: 8,
                    direction: 1,
                }],
                &rotation,
            )
            .is_none(),
            "the second right turn still cannot publish direction 1 early"
        );

        let boundary_orientation = 0.744_166_1;
        let boundary_rotation = 0.575_f32 / 60.0;
        assert_eq!(
            crate::continuum_position::protocol_direction(
                (boundary_orientation - boundary_rotation * 2.0).rem_euclid(1.0)
            ),
            28,
            "fixture must land on the f32 side of the truncating 28/29 boundary"
        );
        let drifted_private_orientation = 0.744_166_8;
        assert_eq!(
            crate::continuum_position::protocol_direction(
                (drifted_private_orientation - boundary_rotation * 2.0).rem_euclid(1.0)
            ),
            29,
            "fixture must distinguish the private diagnostic sub-bin"
        );
        assert!(
            constrained_initial_orientation(
                boundary_orientation,
                29,
                &[
                    ClientInputObservation {
                        bits: 4,
                        direction: 29,
                    },
                    ClientInputObservation {
                        bits: 4,
                        direction: 28,
                    },
                ],
                &[boundary_rotation; 2],
            )
            .is_some(),
            "an exact honest left turn uses the retained canonical point, not private sub-bin drift"
        );

        assert!(
            constrained_initial_orientation(
                0.722_083_3,
                28,
                &[
                    ClientInputObservation {
                        bits: 4,
                        direction: 28,
                    },
                    ClientInputObservation {
                        bits: 4,
                        direction: 28,
                    },
                ],
                &[boundary_rotation; 2],
            )
            .is_some(),
            "the captured post-wrap canonical point replays from the exact state, not private drift"
        );
    }

    #[test]
    fn exact_f32_direction_history_replays_captured_left_turn_without_interval_math() {
        let left_step = 0.575_f32 / 60.0;
        let inputs = [5_u8, 5, 4].map(|direction| ClientInputObservation {
            bits: 1 << 2,
            direction,
        });
        assert!(exact_orientation_history_matches(
            0.15_f32,
            crate::continuum_position::protocol_direction(0.15_f32),
            &inputs,
            &[left_step; 3],
        ));
        let forged = [5_u8, 5, 5].map(|direction| ClientInputObservation {
            bits: 1 << 2,
            direction,
        });
        assert!(!exact_orientation_history_matches(
            0.15_f32,
            6,
            &forged,
            &[left_step; 3],
        ));

        // Captured Chaos rotation: eight reference Left branches from 0.1f
        // round the last tiny negative residue plus one to exact 1.0f.  The
        // raw state and wrapped wire direction zero remain valid through a
        // neutral frame; changing that last wire bin is still rejected.
        let chaos_step = 0.75_f32 * (1.0_f32 / 60.0_f32);
        let mut wrapped = [ClientInputObservation {
            bits: 1 << 2,
            direction: 0,
        }; 9];
        for (record, direction) in wrapped[..8].iter_mut().zip([3_u8, 2, 2, 1, 1, 0, 0, 0]) {
            record.direction = direction;
        }
        wrapped[8].bits = 0;
        assert!(exact_orientation_history_matches(
            0.1_f32,
            4,
            &wrapped,
            &[chaos_step; 9],
        ));
        let mut forged_wrap = wrapped;
        forged_wrap[7].direction = 1;
        assert!(!exact_orientation_history_matches(
            0.1_f32,
            4,
            &forged_wrap,
            &[chaos_step; 9],
        ));
    }

    #[test]
    fn diagnostic_direction_history_uses_exact_authority_not_private_observer_drift() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client");
        let sequence = 77;
        let exact = ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 0.15,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_700.0,
            max_energy: 1_700.0,
        };
        client.last_position_sequence = Some(sequence);
        client.canonical_movement_witness = Some(CanonicalPositionWitness {
            sequence,
            life_seq: client.life_seq,
            state: exact,
            wire_x: 8_200,
            wire_y: 8_200,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: crate::continuum_position::protocol_direction(exact.orientation),
            door_seed: 0,
            door_phase_tick: 0,
        });
        let inputs = [5_u8, 5, 4].map(|direction| ClientInputObservation {
            bits: 1 << 2,
            direction,
        });
        let steps = [0.575_f32 / 60.0; 3];
        assert!(diagnostic_orientation_history_matches(
            client,
            0.151_249_99,
            &inputs,
            &steps,
        ));
        let mut forged = inputs;
        forged[2].direction = 5;
        assert!(!diagnostic_orientation_history_matches(
            client,
            exact.orientation,
            &forged,
            &steps,
        ));
    }

    #[test]
    fn missing_position_witness_trace_is_bounded_optional_and_read_only() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let state = Arc::new(Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position_sequence = Some(35194);
        client.last_input_seq = Some(35194);
        client.recent_input_records.clear();
        // Even a broken oversized ring must not produce unbounded diagnostics.
        for sequence in 0..=crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16 {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input: ClientInputObservation {
                        bits: 0,
                        direction: 28,
                    },
                    door_seed: 0,
                    door_phase_tick: 0,
                });
        }
        let before = validate_position_input_witness(client, 35193, 28, 17101);
        assert_eq!(before, Err(PositionInputWitnessError::Missing));
        for (level, enabled) in [(tracing::Level::INFO, false), (tracing::Level::DEBUG, true)] {
            let bytes = Arc::new(Mutex::new(Vec::new()));
            let writer = Capture(bytes.clone());
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_max_level(level)
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                trace_missing_position_input_witness(
                    client, player_id, 35193, 28, 707, 17101, "test",
                );
            });
            let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
            if enabled {
                assert!(output.contains("position_sequence=35193"));
                assert!(output.contains("reported_weapon=707"));
                assert!(output.contains("last_position_sequence=Some(35194)"));
                let expected =
                    (0..crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16).collect::<Vec<_>>();
                assert!(output.contains(&format!("retained_sequences={expected:?}")));
            } else {
                assert!(output.is_empty());
            }
            assert_eq!(
                validate_position_input_witness(client, 35193, 28, 17101),
                before
            );
            assert_eq!(client.last_position_sequence, Some(35194));
            assert_eq!(
                client.recent_input_records.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS + 1
            );
        }
    }

    #[test]
    fn exact_witness_is_wrap_safe_reordered_safe_and_life_scoped() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.last_position_sequence = Some(u16::MAX);
        let exact_anchor = ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 0.995,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        client.server_sim_state = Some(exact_anchor);
        client.server_sim_anchor_tick = Some(1_000);
        client.last_position_server_tick = Some(1_000);
        client.canonical_movement_witness = Some(CanonicalPositionWitness {
            sequence: u16::MAX,
            life_seq: client.life_seq,
            state: exact_anchor,
            wire_x: 8_200,
            wire_y: 8_200,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 39,
            door_seed: 0,
            door_phase_tick: 0,
        });
        let wrapped = ClientInputObservation {
            bits: 8,
            direction: 0,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 0,
                input: wrapped,
                door_seed: 0,
                door_phase_tick: 0,
            });
        assert_eq!(
            validate_position_input_witness(client, 0, 0, 1_001),
            Ok(wrapped),
            "an honest right turn across 39→0 must keep a non-empty truncating-bin witness"
        );

        let reordered = ClientInputObservation {
            bits: 0,
            direction: 39,
        };
        client
            .recent_input_records
            .push_front(crate::state::SequencedInputObservation {
                sequence: u16::MAX,
                input: reordered,
                door_seed: 0,
                door_phase_tick: 0,
            });
        client.last_position_sequence = Some(0);
        assert_eq!(
            validate_position_input_witness(client, u16::MAX, 39, 1_001),
            Ok(reordered),
            "a reordered position still needs, and may use, its own exact witness"
        );

        client.clear_server_runtime();
        assert_eq!(
            validate_position_input_witness(client, u16::MAX, 39, 1_001),
            Err(PositionInputWitnessError::Missing),
            "a prior-life input record must never witness the next life"
        );
        let next_life = ClientInputObservation {
            bits: 0,
            direction: 20,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 1,
                input: next_life,
                door_seed: 0,
                door_phase_tick: 0,
            });
        assert_eq!(
            validate_position_input_witness(client, 1, 20, 1_002),
            Ok(next_life),
            "the new life accepts its own exact initial witness"
        );
    }

    #[test]
    fn exact_transmitted_directions_match_long_gap_turn_and_thrust_replay() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let map = state.lock().expect("server state").zone_map();
        let tuning = movement_tuning_from_ship(0, PlayerMovementUpgrades::default());
        let radius = arena_ship_radius_pixels(0);
        let bounce = arena_bounce_factor();

        for (initial_orientation, steps) in [
            (0.987_49_f32, 6_u16),
            (0.012_51, 10),
            (0.487_49, 18),
            (0.737_49, 32),
        ] {
            let initial = ServerSimMovement {
                position_x: f32::NAN,
                position_y: f32::NAN,
                orientation: initial_orientation,
                velocity_x: 240.0,
                velocity_y: -120.0,
                energy: 1_300.0,
                max_energy: 1_700.0,
            };
            let mut client = SimIntegration {
                state: initial,
                position_x: 8_200.0,
                position_y: 8_200.0,
                bounced: false,
            };
            let server_initial = client;
            let mut records = Vec::new();

            for _ in 0..steps {
                client = integrate_with_collision(
                    client.state,
                    client.position_x,
                    client.position_y,
                    decode_input_bits(0x09),
                    tuning,
                    PlayerMovementEnvironment::default(),
                    OBSERVED_POSITION_INTERVAL_SECONDS,
                    radius,
                    bounce,
                    map,
                    &[],
                    &[],
                    0,
                );
                records.push(ClientInputObservation {
                    bits: 0x09,
                    direction: crate::continuum_position::protocol_direction(
                        client.state.orientation,
                    ),
                });
            }
            let rotation_steps = records
                .iter()
                .map(|_input| {
                    input_rotation_step(
                        tuning.rotation_per_second,
                        OBSERVED_POSITION_INTERVAL_SECONDS,
                    )
                })
                .collect::<Vec<_>>();
            let initial_server_orientation = constrained_initial_orientation(
                server_initial.state.orientation,
                crate::continuum_position::protocol_direction(server_initial.state.orientation),
                &records,
                &rotation_steps,
            )
            .expect("honest direction history");
            let mut server = SimIntegration {
                state: ServerSimMovement {
                    orientation: initial_server_orientation,
                    ..server_initial.state
                },
                ..server_initial
            };
            for input in records {
                server = integrate_with_collision(
                    server.state,
                    server.position_x,
                    server.position_y,
                    decode_input_bits(input.bits),
                    tuning,
                    PlayerMovementEnvironment::default(),
                    OBSERVED_POSITION_INTERVAL_SECONDS,
                    radius,
                    bounce,
                    map,
                    &[],
                    &[],
                    0,
                );
            }

            let deviation = (client.position_x - server.position_x)
                .hypot(client.position_y - server.position_y);
            assert!(
                deviation < 2.0,
                "{steps}-step half/wrap replay deviated {deviation}: client={client:?} server={server:?}"
            );
            assert_eq!(
                crate::continuum_position::protocol_direction(server.state.orientation),
                crate::continuum_position::protocol_direction(client.state.orientation),
            );
        }
    }

    #[test]
    fn per_record_door_seed_replays_the_artifact_bounce_before_the_final_phase() {
        use crate::state::{SequencedInputObservation, ServerSimMovement};

        const DOOR_TILE: u8 = 164;
        const DOOR_X: usize = 498;
        const DOOR_Y: usize = 512;
        let map = synthetic_dynamic_door_map(DOOR_X, DOOR_Y, DOOR_TILE);
        let closed_seed = (0..=u8::MAX)
            .find(|&seed| crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, seed))
            .expect("door 164 has a closed seed");
        let open_seed = (0..=u8::MAX)
            .find(|&seed| !crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, seed))
            .expect("door 164 has an open seed");
        let ship = 1;
        let radius = arena_ship_radius_pixels(ship);
        let tuning = movement_tuning_from_ship(ship, PlayerMovementUpgrades::default());
        let initial = ServerSimMovement {
            position_x: DOOR_X as f32 * TILE_SIZE_PX + TILE_SIZE_PX + radius + 3.5,
            position_y: DOOR_Y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
            orientation: 0.75,
            velocity_x: -374.4,
            velocity_y: 0.0,
            energy: 1_500.0,
            max_energy: 1_700.0,
        };
        let seeds = [closed_seed, open_seed, open_seed, open_seed];
        let mut exact = SimIntegration {
            state: initial,
            position_x: initial.position_x,
            position_y: initial.position_y,
            bounced: false,
        };
        let mut static_final = exact;
        for seed in seeds {
            exact = integrate_with_collision(
                exact.state,
                exact.position_x,
                exact.position_y,
                decode_input_bits(0),
                tuning,
                PlayerMovementEnvironment::default(),
                OBSERVED_POSITION_INTERVAL_SECONDS,
                radius,
                arena_bounce_factor(),
                &map,
                &[seed],
                &[],
                ship,
            );
            static_final = integrate_with_collision(
                static_final.state,
                static_final.position_x,
                static_final.position_y,
                decode_input_bits(0),
                tuning,
                PlayerMovementEnvironment::default(),
                OBSERVED_POSITION_INTERVAL_SECONDS,
                radius,
                arena_bounce_factor(),
                &map,
                &[open_seed],
                &[],
                ship,
            );
        }
        assert!(
            exact.state.velocity_x > 0.0,
            "the first closed phase must bounce"
        );
        assert!(
            (exact.position_x - static_final.position_x).abs() > 10.0,
            "a final-phase-only replay must reproduce the Round 16 divergence"
        );

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.player.ship = ship;
        client.server_sim_state = Some(initial);
        client.server_sim_anchor_tick = Some(1_000);
        client.server_sim_input_bits = Some(0);
        client.last_position_sequence = Some(100);
        client.server_energy = initial.energy as u16;
        client.server_max_energy = initial.max_energy as u16;
        for (index, seed) in seeds.into_iter().enumerate() {
            client
                .recent_input_records
                .push_back(SequencedInputObservation {
                    sequence: 101 + index as u16,
                    input: ClientInputObservation {
                        bits: 0,
                        direction: crate::continuum_position::protocol_direction(
                            initial.orientation,
                        ),
                    },
                    door_seed: seed,
                    door_phase_tick: 1_001 + index as u32,
                });
        }
        client.last_input_record = client
            .recent_input_records
            .back()
            .map(|record| record.input);
        let result = observe_simulation_for_accepted_position(
            client,
            &map,
            &[open_seed],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(initial.position_x.round() as u16),
            Some(initial.position_y.round() as u16),
            crate::continuum_position::protocol_direction(exact.state.orientation),
            exact.position_x.round() as u16,
            exact.position_y.round() as u16,
            crate::continuum_position::protocol_velocity_component(exact.state.velocity_x),
            crate::continuum_position::protocol_velocity_component(exact.state.velocity_y),
            Some(104),
            1_007,
            None,
            false,
        );
        assert!(
            result.deviation_px.is_some_and(|deviation| deviation <= 1),
            "per-record phase replay must match the artifact path: {result:?}"
        );
        assert_eq!(result.snap_back, None);
    }

    #[test]
    fn private_subpixel_position_and_wire_rounding_both_ignore_a_nonleading_door() {
        use crate::state::{SequencedInputObservation, ServerSimMovement};

        const DOOR_TILE: u8 = 169;
        const DOOR_X: usize = 480;
        const DOOR_Y: usize = 558;
        let map = synthetic_dynamic_door_map(DOOR_X, DOOR_Y, DOOR_TILE);
        let closed_seed = (0..=u8::MAX)
            .find(|&seed| crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, seed))
            .expect("door 169 has a closed seed");
        let ship = 1;
        let radius = arena_ship_radius_pixels(ship);
        let tuning = movement_tuning_from_ship(ship, PlayerMovementUpgrades::default());
        let initial = ServerSimMovement {
            position_x: DOOR_X as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
            position_y: DOOR_Y as f32 * TILE_SIZE_PX - radius - 0.25,
            orientation: 0.625,
            velocity_x: -120.0,
            velocity_y: -60.0,
            energy: 1_500.0,
            max_energy: 1_700.0,
        };
        let exact = integrate_with_collision(
            initial,
            initial.position_x,
            initial.position_y,
            decode_input_bits(0),
            tuning,
            PlayerMovementEnvironment::default(),
            OBSERVED_POSITION_INTERVAL_SECONDS,
            radius,
            arena_bounce_factor(),
            &map,
            &[closed_seed],
            &[],
            ship,
        );
        let rounded = integrate_with_collision(
            initial,
            initial.position_x.round(),
            initial.position_y.round(),
            decode_input_bits(0),
            tuning,
            PlayerMovementEnvironment::default(),
            OBSERVED_POSITION_INTERVAL_SECONDS,
            radius,
            arena_bounce_factor(),
            &map,
            &[closed_seed],
            &[],
            ship,
        );
        assert!(!exact.bounced, "the exact post-bounce hull is clear");
        assert!(
            !rounded.bounced,
            "the reference game queries only the movement-leading row/column, so the nearby door behind the ship cannot recreate the old full-hull false bounce",
        );
        assert!(exact.state.velocity_x < 0.0 && rounded.state.velocity_x < 0.0);

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.player.ship = ship;
        client.server_sim_state = Some(initial);
        client.server_sim_anchor_tick = Some(2_000);
        client.server_sim_input_bits = Some(0);
        client.last_position_sequence = Some(200);
        client.server_energy = initial.energy as u16;
        client.server_max_energy = initial.max_energy as u16;
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: 201,
                input: ClientInputObservation {
                    bits: 0,
                    direction: crate::continuum_position::protocol_direction(initial.orientation),
                },
                door_seed: closed_seed,
                door_phase_tick: 2_001,
            });
        client.last_input_record = client
            .recent_input_records
            .back()
            .map(|record| record.input);
        let result = observe_simulation_for_accepted_position(
            client,
            &map,
            &[closed_seed],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(initial.position_x.round() as u16),
            Some(initial.position_y.round() as u16),
            crate::continuum_position::protocol_direction(exact.state.orientation),
            exact.position_x.round() as u16,
            exact.position_y.round() as u16,
            crate::continuum_position::protocol_velocity_component(exact.state.velocity_x),
            crate::continuum_position::protocol_velocity_component(exact.state.velocity_y),
            Some(201),
            2_002,
            None,
            false,
        );
        assert!(
            result.deviation_px.is_some_and(|deviation| deviation <= 1),
            "the private float state must match the exact client path: {result:?}"
        );
        assert!(
            client
                .server_sim_state
                .is_some_and(|movement| movement.velocity_x < 0.0),
            "the observer must retain the non-rebounded velocity"
        );
    }

    #[test]
    fn multi_phase_door_close_relocation_requires_started_transition_spawn_vector_and_life_witness()
    {
        use crate::state::{SequencedInputObservation, ServerSimMovement};

        const DOOR_TILE: u8 = 162;
        const DOOR_X: usize = 480;
        const DOOR_Y: usize = 500;
        let map = synthetic_dynamic_door_map(DOOR_X, DOOR_Y, DOOR_TILE);
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (before_tick, before_seed, after_tick, after_seed) = (0_u32..512)
            .find_map(|count| {
                let before_tick = count.saturating_mul(delay);
                let after_tick = before_tick.saturating_add(3 * delay);
                let phases = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    before_tick,
                    after_tick,
                )?;
                let before_seed = phases.first()?.seed;
                let after_seed = phases.last()?.seed;
                let closes_during_skip = phases.windows(2).any(|transition| {
                    !crate::zone::position_blocking_tile_with_door_seed(
                        DOOR_TILE,
                        transition[0].seed,
                    ) && crate::zone::position_blocking_tile_with_door_seed(
                        DOOR_TILE,
                        transition[1].seed,
                    )
                });
                (!crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, before_seed)
                    && crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, after_seed)
                    && closes_during_skip)
                    .then_some((before_tick, before_seed, after_tick, after_seed))
            })
            .expect("canonical RNG reaches an open-to-closed door 162 transition across 3 phases");
        let ship = 1;
        let radius = arena_ship_radius_pixels(ship);
        let door_x = DOOR_X as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5;
        let door_y = DOOR_Y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5;
        assert!(circle_hull_fits_with_door_seed(
            door_x,
            door_y,
            radius,
            &map,
            before_seed
        ));
        assert!(!circle_hull_fits_with_door_seed(
            door_x, door_y, radius, &map, after_seed
        ));

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.player.ship = ship;
        client.door_state = Some(door_state);
        let initial = ServerSimMovement {
            position_x: door_x,
            position_y: door_y,
            orientation: 0.0,
            velocity_x: 90.0,
            velocity_y: -30.0,
            energy: 1_500.0,
            max_energy: 1_700.0,
        };
        client.server_sim_state = Some(initial);
        client.server_sim_anchor_tick = Some(before_tick);
        client.server_sim_input_bits = Some(0);
        client.last_position_sequence = Some(u16::MAX);
        client.server_energy = initial.energy as u16;
        client.server_max_energy = initial.max_energy as u16;
        let input = ClientInputObservation {
            bits: 1,
            direction: 0,
        };
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: u16::MAX,
                input: ClientInputObservation {
                    bits: 0,
                    direction: 0,
                },
                door_seed: before_seed,
                door_phase_tick: before_tick,
            });
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: 0,
                input,
                door_seed: after_seed,
                door_phase_tick: after_tick,
            });
        let spawn_x = 8_200_u16;
        let spawn_y = 8_200_u16;
        let expected = integrate_with_collision(
            ServerSimMovement {
                position_x: spawn_x as f32,
                position_y: spawn_y as f32,
                velocity_x: 0.0,
                velocity_y: 0.0,
                ..initial
            },
            spawn_x as f32,
            spawn_y as f32,
            decode_input_bits(input.bits),
            movement_tuning_from_ship(ship, PlayerMovementUpgrades::default()),
            PlayerMovementEnvironment::default(),
            OBSERVED_POSITION_INTERVAL_SECONDS,
            radius,
            arena_bounce_factor(),
            &map,
            &[after_seed],
            &[],
            ship,
        );
        let expected_x = expected.position_x.round() as u16;
        let expected_y = expected.position_y.round() as u16;
        let expected_direction =
            crate::continuum_position::protocol_direction(expected.state.orientation);
        let expected_velocity_x =
            crate::continuum_position::protocol_velocity_component(expected.state.velocity_x);
        let expected_velocity_y =
            crate::continuum_position::protocol_velocity_component(expected.state.velocity_y);
        let matches =
            |client: &crate::state::ClientHandle, x, y, direction, velocity_x, velocity_y| {
                position_replay_matches_door_close_relocation(
                    client,
                    &map,
                    false,
                    false,
                    false,
                    PlayerMovementUpgrades::default(),
                    &[],
                    door_x.round() as u16,
                    door_y.round() as u16,
                    0,
                    after_tick.saturating_add(2),
                    spawn_x,
                    spawn_y,
                    direction,
                    x,
                    y,
                    velocity_x,
                    velocity_y,
                )
            };

        assert!(matches(
            client,
            expected_x,
            expected_y,
            expected_direction,
            expected_velocity_x,
            expected_velocity_y,
        ));
        client.server_sim_state.as_mut().unwrap().position_x = door_x + radius + TILE_SIZE_PX;
        assert!(
            matches(
                client,
                expected_x,
                expected_y,
                expected_direction,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "bounded private-observer drift must not erase the exact admitted door AABB"
        );
        client.server_sim_state = Some(initial);
        assert!(
            !position_replay_matches_door_close_relocation(
                client,
                &map,
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &[],
                door_x.round() as u16,
                door_y.round() as u16,
                0,
                after_tick.saturating_sub(1),
                spawn_x,
                spawn_y,
                expected_direction,
                expected_x,
                expected_y,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "even a canonical close cannot relocate before its transition starts"
        );
        assert!(
            !matches(
                client,
                expected_x.wrapping_add(1),
                expected_y,
                expected_direction,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "a forged destination is not a relocation witness"
        );
        assert!(
            !matches(
                client,
                expected_x,
                expected_y,
                expected_direction,
                expected_velocity_x.wrapping_add(100),
                expected_velocity_y,
            ),
            "a forged post-spawn vector is not a relocation witness"
        );
        assert!(
            !matches(
                client,
                expected_x,
                expected_y,
                expected_direction.wrapping_add(1) % 40,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "a forged post-spawn direction is not a relocation witness"
        );

        client.recent_input_records.back_mut().unwrap().door_seed = before_seed;
        assert!(
            !matches(
                client,
                expected_x,
                expected_y,
                expected_direction,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "no physical closure cannot relocate"
        );
        client.recent_input_records.back_mut().unwrap().door_seed = after_seed;
        client
            .recent_input_records
            .back_mut()
            .unwrap()
            .door_phase_tick = after_tick.saturating_add(delay);
        assert!(
            !matches(
                client,
                expected_x,
                expected_y,
                expected_direction,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "a noncanonical endpoint seed cannot synthesize relocation authority"
        );
        client
            .recent_input_records
            .back_mut()
            .unwrap()
            .door_phase_tick = after_tick;
        client.last_position_sequence = Some(0);
        assert!(
            !matches(
                client,
                expected_x,
                expected_y,
                expected_direction,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "the wrap-safe positive sequence is consumed after acceptance"
        );
        client.clear_server_runtime();
        assert!(
            !matches(
                client,
                expected_x,
                expected_y,
                expected_direction,
                expected_velocity_x,
                expected_velocity_y,
            ),
            "a cleared prior-life witness cannot authorize the next life"
        );
    }

    #[test]
    fn canonical_door_relocation_ignores_private_divergence_and_resets_exactly_once() {
        use crate::state::{
            CanonicalPositionWitness, SequencedInputObservation, ServerSimMovement,
        };

        #[derive(Debug, PartialEq)]
        struct AdmissionState {
            energy: u16,
            bounty: u16,
            weapons: crate::authority::WeaponInventory,
            last_position: Option<Vec<u8>>,
            visibility_cell: Option<crate::visibility::VisibilityCell>,
            last_position_sequence: Option<u16>,
            last_position_server_tick: Option<u32>,
            recent_input_records: Vec<SequencedInputObservation>,
            canonical_witnesses: Vec<CanonicalPositionWitness>,
            canonical_movement_witness: Option<CanonicalPositionWitness>,
            private_state: Option<ServerSimMovement>,
            private_anchor: Option<u32>,
            private_input: Option<u16>,
            drift_x: f32,
            drift_y: f32,
            position_history: Vec<crate::state::RecentPositionSample>,
            pending_snapshot_weapon: u16,
            active_projectiles: crate::authority_projectiles::ProjectileRegistry,
        }

        fn admission_state(
            state: &std::sync::Arc<std::sync::Mutex<crate::state::ServerState>>,
            player_id: u16,
        ) -> AdmissionState {
            let state = state.lock().expect("server state");
            let client = &state.clients[&player_id];
            AdmissionState {
                energy: client.server_energy,
                bounty: client.server_bounty,
                weapons: client.server_weapons,
                last_position: client.last_position.clone(),
                visibility_cell: client.visibility_cell,
                last_position_sequence: client.last_position_sequence,
                last_position_server_tick: client.last_position_server_tick,
                recent_input_records: client.recent_input_records.iter().copied().collect(),
                canonical_witnesses: client
                    .canonical_position_witnesses
                    .iter()
                    .copied()
                    .collect(),
                canonical_movement_witness: client.canonical_movement_witness,
                private_state: client.server_sim_state,
                private_anchor: client.server_sim_anchor_tick,
                private_input: client.server_sim_input_bits,
                drift_x: client.movement_drift_x,
                drift_y: client.movement_drift_y,
                position_history: client.position_history.iter().copied().collect(),
                pending_snapshot_weapon: client.pending_snapshot_weapon,
                active_projectiles: state.active_projectiles.clone(),
            }
        }

        fn position_packet(
            direction: u8,
            x: u16,
            y: u16,
            exact_x: f32,
            exact_y: f32,
            velocity_x: f32,
            velocity_y: f32,
            orientation: f32,
            energy: u16,
            sequence: u16,
        ) -> Vec<u8> {
            let mut packet = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
            packet[0] = crate::continuum_position::CLIENT_POSITION_TYPE;
            packet[1] = direction;
            packet[6..8].copy_from_slice(
                &crate::continuum_position::protocol_velocity_component(velocity_x).to_le_bytes(),
            );
            packet[8..10].copy_from_slice(&y.to_le_bytes());
            packet[12..14].copy_from_slice(&x.to_le_bytes());
            packet[14..16].copy_from_slice(
                &crate::continuum_position::protocol_velocity_component(velocity_y).to_le_bytes(),
            );
            packet[18..20].copy_from_slice(&energy.to_le_bytes());
            packet[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(sequence)
                    .to_le_bytes(),
            );
            crate::continuum_position::ClientPositionCanonicalState::from_f32(
                exact_x,
                exact_y,
                velocity_x,
                velocity_y,
                orientation,
            )
            .encode(&mut packet);
            packet
        }

        const DOOR_TILE: u8 = 162;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (before_tick, before_seed, after_tick, after_seed) = (0_u32..512)
            .find_map(|count| {
                let before_tick = count.saturating_mul(delay);
                let after_tick = before_tick.saturating_add(delay);
                let before_seed = crate::doors::client_door_simulation_seed_at_tick(
                    Some(door_state),
                    before_tick,
                )?;
                let after_seed = crate::doors::client_door_simulation_seed_at_tick(
                    Some(door_state),
                    after_tick,
                )?;
                (!crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, before_seed)
                    && crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, after_seed))
                .then_some((before_tick, before_seed, after_tick, after_seed))
            })
            .expect("canonical RNG reaches an open-to-closed transition");

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let identity = state
            .lock()
            .expect("server state")
            .active_life_identity(player_id)
            .expect("active life");

        const STEPS: u16 = 12;
        const ORIENTATION: f32 = 0.739_58;
        let previous_sequence = 40_000_u16;
        let current_sequence = previous_sequence.wrapping_add(STEPS);
        let accepted_tick = after_tick.saturating_add(2);
        let (door_x, door_y, spawn_x, spawn_y, energy, canonical_replay, private_tail) = {
            let mut state = state.lock().expect("server state");
            let map = state.zone_map();
            let radius = arena_ship_radius_pixels(state.clients[&player_id].player.ship);
            let door_index = map
                .tiles
                .iter()
                .enumerate()
                .find_map(|(index, &tile)| {
                    if tile != DOOR_TILE {
                        return None;
                    }
                    let x = (index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5;
                    let y = (index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5;
                    (circle_hull_fits_with_door_seed(x, y, radius, map, before_seed)
                        && !circle_hull_fits_with_door_seed(x, y, radius, map, after_seed))
                    .then_some(index)
                })
                .expect("real zone contains an isolated door 162 relocation hull");
            let door_x = ((door_index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5)
                .round() as u16;
            let door_y = ((door_index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5)
                .round() as u16;
            let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(map);
            let direction = crate::continuum_position::protocol_direction(ORIENTATION);
            let previous_packet = position_packet(
                direction,
                door_x,
                door_y,
                door_x as f32,
                door_y as f32,
                0.0,
                0.0,
                ORIENTATION,
                state.clients[&player_id].server_energy,
                previous_sequence,
            );
            let previous_position =
                crate::arena::large_position_from_client(player_id, &previous_packet)
                    .expect("previous door position");
            state.set_client_position(player_id, previous_position);

            let client = state.clients.get_mut(&player_id).expect("client present");
            let energy = client.server_energy;
            let canonical_initial = ServerSimMovement {
                position_x: door_x as f32,
                position_y: door_y as f32,
                orientation: ORIENTATION,
                velocity_x: 0.0,
                velocity_y: 0.0,
                energy: energy as f32,
                max_energy: client.server_max_energy as f32,
            };
            client.door_state = Some(door_state);
            // Deliberately starve only the private observer. Its historical
            // relocation replay predicts a slower, different tail, while the
            // accepted canonical witness retains the real full-energy state.
            let private_initial = ServerSimMovement {
                energy: 0.0,
                max_energy: 0.0,
                ..canonical_initial
            };
            client.server_sim_state = Some(private_initial);
            client.server_sim_anchor_tick = Some(before_tick);
            client.server_sim_input_bits = Some(0);
            client.last_position_sequence = Some(previous_sequence);
            client.last_position_server_tick = Some(before_tick);
            let previous_witness = CanonicalPositionWitness {
                sequence: previous_sequence,
                life_seq: client.life_seq,
                state: canonical_initial,
                wire_x: door_x,
                wire_y: door_y,
                wire_velocity_x: 0,
                wire_velocity_y: 0,
                direction,
                door_seed: before_seed,
                door_phase_tick: before_tick,
            };
            client
                .canonical_position_witnesses
                .push_back(previous_witness);
            client.canonical_movement_witness = Some(previous_witness);
            let input = ClientInputObservation {
                bits: (1 << 0) | (1 << 4),
                direction,
            };
            client
                .recent_input_records
                .push_back(SequencedInputObservation {
                    sequence: previous_sequence,
                    input: ClientInputObservation { bits: 0, direction },
                    door_seed: before_seed,
                    door_phase_tick: before_tick,
                });
            for offset in 1..=STEPS {
                client
                    .recent_input_records
                    .push_back(SequencedInputObservation {
                        sequence: previous_sequence.wrapping_add(offset),
                        input,
                        door_seed: after_seed,
                        door_phase_tick: after_tick,
                    });
            }
            client.last_input_seq = Some(current_sequence);
            client.last_input_record = Some(input);
            client.server_thrust_acceleration =
                crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(
                    client.player.ship,
                );
            client.server_top_speed =
                crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(
                    client.player.ship,
                );
            let upgrades = PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            };
            let canonical_replay = replay_canonical_position_gap(
                client,
                map,
                false,
                false,
                false,
                upgrades,
                &[],
                current_sequence,
                accepted_tick,
                spawn_x,
                spawn_y,
            )
            .expect("canonical relocation replay");
            assert!(canonical_replay.relocated);
            assert!(
                canonical_replay
                    .witnesses
                    .iter()
                    .all(|witness| witness.state.orientation.is_finite())
            );

            let tuning = movement_tuning_from_ship(client.player.ship, upgrades);
            let mut private_tail = SimIntegration {
                state: ServerSimMovement {
                    position_x: spawn_x as f32,
                    position_y: spawn_y as f32,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    ..private_initial
                },
                position_x: spawn_x as f32,
                position_y: spawn_y as f32,
                bounced: false,
            };
            for _ in 0..STEPS {
                private_tail = integrate_with_collision(
                    private_tail.state,
                    private_tail.position_x,
                    private_tail.position_y,
                    decode_input_bits(input.bits),
                    tuning,
                    PlayerMovementEnvironment::default(),
                    OBSERVED_POSITION_INTERVAL_SECONDS,
                    arena_ship_radius_pixels(client.player.ship),
                    arena_bounce_factor(),
                    map,
                    &[after_seed],
                    &[],
                    client.player.ship,
                );
            }
            (
                door_x,
                door_y,
                spawn_x,
                spawn_y,
                energy,
                canonical_replay,
                private_tail,
            )
        };

        let canonical_tail = *canonical_replay.witnesses.last().expect("canonical tail");
        assert_eq!(canonical_tail.state.orientation, ORIENTATION);
        assert_ne!(
            canonical_tail.wire_velocity_y, 0,
            "the relocation regression must retain a visible transverse sub-bin component"
        );
        let private_direction =
            crate::continuum_position::protocol_direction(private_tail.state.orientation);
        let private_x = private_tail.position_x.round() as u16;
        let private_y = private_tail.position_y.round() as u16;
        let private_velocity_x =
            crate::continuum_position::protocol_velocity_component(private_tail.state.velocity_x);
        let private_velocity_y =
            crate::continuum_position::protocol_velocity_component(private_tail.state.velocity_y);
        assert_ne!(
            (private_x, private_y, private_velocity_x, private_velocity_y),
            (
                canonical_tail.wire_x,
                canonical_tail.wire_y,
                canonical_tail.wire_velocity_x,
                canonical_tail.wire_velocity_y,
            ),
            "fixture must make private and canonical relocation tails diverge"
        );
        // Thrust and recoil now use the discrete wire heading, so alternate
        // floats inside one bin deliberately cannot steer a transverse tail.
        // The exact point interval retained above remains immutable authority;
        // it matters when subsequent rotation crosses a truncating boundary.
        {
            let locked = state.lock().expect("server state");
            let client = &locked.clients[&player_id];
            assert!(position_replay_matches_door_close_relocation(
                client,
                locked.zone_map(),
                false,
                false,
                false,
                PlayerMovementUpgrades {
                    thrust: client.server_thrust_acceleration,
                    top_speed: client.server_top_speed,
                    rotation: client.server_rotation_per_second,
                    recharge: client.server_recharge_per_second,
                    live_values: true,
                },
                &[],
                door_x,
                door_y,
                current_sequence,
                accepted_tick,
                spawn_x,
                spawn_y,
                private_direction,
                private_x,
                private_y,
                private_velocity_x,
                private_velocity_y,
            ));
        }
        let private_packet = position_packet(
            private_direction,
            private_x,
            private_y,
            private_tail.state.position_x,
            private_tail.state.position_y,
            private_tail.state.velocity_x,
            private_tail.state.velocity_y,
            private_tail.state.orientation,
            energy,
            current_sequence,
        );
        let before_private_only = admission_state(&state, player_id);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &private_packet,
                accepted_tick,
            )
            .is_none(),
            "a tail proved only by private observer state has no admission authority"
        );
        assert_eq!(admission_state(&state, player_id), before_private_only);

        let relocation_packet = position_packet(
            canonical_tail.direction,
            canonical_tail.wire_x,
            canonical_tail.wire_y,
            canonical_tail.state.position_x,
            canonical_tail.state.position_y,
            canonical_tail.state.velocity_x,
            canonical_tail.state.velocity_y,
            canonical_tail.state.orientation,
            energy,
            current_sequence,
        );
        let relay = crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &relocation_packet,
            accepted_tick,
        );
        assert!(
            relay.is_some(),
            "the exact real-map relocation must be admitted"
        );
        let locked = state.lock().expect("server state");
        let client = locked.clients.get(&player_id).expect("client present");
        assert_eq!(client.last_position_sequence, Some(current_sequence));
        assert_eq!(
            client
                .last_position
                .as_deref()
                .and_then(crate::arena::large_position_coordinates),
            Some((canonical_tail.wire_x, canonical_tail.wire_y)),
        );
        assert_eq!(client.position_history.len(), 1);
        assert_eq!(
            client.canonical_position_witnesses.len(),
            canonical_replay.witnesses.len() + 1,
            "relocation rebases current movement without erasing prior/intermediate weapon witnesses"
        );
        assert_eq!(
            client
                .canonical_position_witnesses
                .front()
                .map(|witness| witness.sequence),
            Some(previous_sequence)
        );
        let mut committed_tail = canonical_tail;
        committed_tail.state.energy = client.server_energy as f32;
        committed_tail.state.max_energy = client.server_max_energy as f32;
        assert_eq!(
            client.canonical_position_witnesses.back().copied(),
            Some(committed_tail),
            "the movement anchor and newest weapon witness retain the exact canonical tail"
        );
        drop(locked);

        let after_commit = admission_state(&state, player_id);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &relocation_packet,
                accepted_tick,
            )
            .is_none(),
            "the one-shot relocation tail cannot reset authority twice"
        );
        assert_eq!(admission_state(&state, player_id), after_commit);
    }

    #[test]
    fn chained_gap_admission_retains_subpixel_tail_for_next_door_close() {
        use crate::state::{
            CanonicalPositionWitness, SequencedInputObservation, ServerSimMovement,
        };

        fn position_packet(
            witness: CanonicalPositionWitness,
            energy: u16,
            sequence: u16,
        ) -> Vec<u8> {
            let mut packet = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
            packet[0] = crate::continuum_position::CLIENT_POSITION_TYPE;
            packet[1] = witness.direction;
            packet[6..8].copy_from_slice(&witness.wire_velocity_x.to_le_bytes());
            packet[8..10].copy_from_slice(&witness.wire_y.to_le_bytes());
            packet[12..14].copy_from_slice(&witness.wire_x.to_le_bytes());
            packet[14..16].copy_from_slice(&witness.wire_velocity_y.to_le_bytes());
            packet[18..20].copy_from_slice(&energy.to_le_bytes());
            packet[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(sequence)
                    .to_le_bytes(),
            );
            crate::continuum_position::ClientPositionCanonicalState::from_f32(
                witness.state.position_x,
                witness.state.position_y,
                witness.state.velocity_x,
                witness.state.velocity_y,
                witness.state.orientation,
            )
            .encode(&mut packet);
            packet
        }

        const FIRST_GAP: u16 = 2;
        const SECOND_GAP: u16 = 2;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let phase_candidates = (162_u8..=169)
            .filter_map(|door_tile| {
                (0_u32..512).find_map(|count| {
                    let before_tick = count.saturating_mul(delay);
                    let after_tick = before_tick.saturating_add(delay);
                    let phases = crate::doors::client_door_phase_sequence(
                        Some(door_state),
                        before_tick,
                        after_tick,
                    )?;
                    let before = *phases.first()?;
                    let after = *phases.last()?;
                    (!crate::zone::position_blocking_tile_with_door_seed(door_tile, before.seed)
                        && crate::zone::position_blocking_tile_with_door_seed(
                            door_tile, after.seed,
                        ))
                    .then_some((door_tile, before, after))
                })
            })
            .collect::<Vec<_>>();
        assert!(!phase_candidates.is_empty());

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let (
            ship,
            exact_x,
            exact_y,
            wire_x,
            wire_y,
            spawn_x,
            spawn_y,
            open_phase,
            closed_phase,
            exact_relocates,
            rounded_relocates,
        ) = {
            let locked = state.lock().expect("server state");
            let map = locked.zone_map();
            let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(map);
            let candidate = (0_u8..8)
                .find_map(|ship| {
                    let radius = arena_ship_radius_pixels(ship);
                    phase_candidates.iter().copied().find_map(
                        |(door_tile, open_phase, closed_phase)| {
                            map.tiles
                                .iter()
                                .enumerate()
                                .filter(|(_, tile)| **tile == door_tile)
                                .find_map(|(index, _)| {
                                    let tile_x = index % MAP_TILES;
                                    let tile_y = index / MAP_TILES;
                                    let tile_min_x = tile_x as f32 * TILE_SIZE_PX;
                                    let tile_min_y = tile_y as f32 * TILE_SIZE_PX;
                                    let tile_max_x = tile_min_x + TILE_SIZE_PX;
                                    let tile_max_y = tile_min_y + TILE_SIZE_PX;
                                    let mut positions = Vec::new();
                                    for orthogonal in ((tile_min_y - radius).max(0.0) as u16)
                                        ..=((tile_max_y + radius).min(u16::MAX as f32) as u16)
                                    {
                                        for fraction in [0.25_f32, 0.51, 0.75, 0.9] {
                                            positions.push((
                                                tile_max_x + radius + fraction,
                                                orthogonal as f32,
                                            ));
                                            positions.push((
                                                tile_min_x - radius - fraction,
                                                orthogonal as f32,
                                            ));
                                        }
                                    }
                                    for orthogonal in ((tile_min_x - radius).max(0.0) as u16)
                                        ..=((tile_max_x + radius).min(u16::MAX as f32) as u16)
                                    {
                                        for fraction in [0.25_f32, 0.51, 0.75, 0.9] {
                                            positions.push((
                                                orthogonal as f32,
                                                tile_max_y + radius + fraction,
                                            ));
                                            positions.push((
                                                orthogonal as f32,
                                                tile_min_y - radius - fraction,
                                            ));
                                        }
                                    }
                                    positions.into_iter().find_map(|(exact_x, exact_y)| {
                                        if exact_x < 0.0 || exact_y < 0.0 {
                                            return None;
                                        }
                                        let wire_x = exact_x.round() as u16;
                                        let wire_y = exact_y.round() as u16;
                                        let relocates = |x: f32, y: f32| {
                                            crate::gameplay_rules::newly_closed_door_overlap(
                                                x,
                                                y,
                                                radius,
                                                MAP_TILES,
                                                TILE_SIZE_PX,
                                                open_phase.seed,
                                                closed_phase.seed,
                                                |candidate_x, candidate_y| {
                                                    if candidate_x < 0 || candidate_y < 0 {
                                                        return None;
                                                    }
                                                    map.tiles
                                                        .get(
                                                            candidate_y as usize * MAP_TILES
                                                                + candidate_x as usize,
                                                        )
                                                        .copied()
                                                },
                                                crate::zone::position_blocking_tile_with_door_seed,
                                            )
                                        };
                                        let exact_relocates = relocates(exact_x, exact_y);
                                        let rounded_relocates =
                                            relocates(wire_x as f32, wire_y as f32);
                                        (exact_relocates != rounded_relocates
                                            && circle_hull_fits_with_door_seed(
                                                exact_x,
                                                exact_y,
                                                radius,
                                                map,
                                                open_phase.seed,
                                            )
                                            && circle_hull_fits_with_door_seed(
                                                wire_x as f32,
                                                wire_y as f32,
                                                radius,
                                                map,
                                                open_phase.seed,
                                            )
                                            && circle_hull_fits_with_door_seed(
                                                wire_x as f32,
                                                wire_y as f32,
                                                (radius - 1.0).max(0.0),
                                                map,
                                                closed_phase.seed,
                                            ))
                                        .then_some((
                                            ship,
                                            exact_x,
                                            exact_y,
                                            wire_x,
                                            wire_y,
                                            open_phase,
                                            closed_phase,
                                            exact_relocates,
                                            rounded_relocates,
                                        ))
                                    })
                                })
                        },
                    )
                })
                .expect("real map has a subpixel-sensitive isolated door edge");
            (
                candidate.0,
                candidate.1,
                candidate.2,
                candidate.3,
                candidate.4,
                spawn_x,
                spawn_y,
                candidate.5,
                candidate.6,
                candidate.7,
                candidate.8,
            )
        };

        let previous_sequence = 50_000_u16;
        let first_sequence = previous_sequence.wrapping_add(FIRST_GAP);
        let second_sequence = first_sequence.wrapping_add(SECOND_GAP);
        let identity = state
            .lock()
            .expect("server state")
            .active_life_identity(player_id)
            .expect("active life");
        let energy = {
            let mut locked = state.lock().expect("server state");
            let initial = ServerSimMovement {
                position_x: exact_x,
                position_y: exact_y,
                orientation: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                energy: locked.clients[&player_id].server_energy as f32,
                max_energy: locked.clients[&player_id].server_max_energy as f32,
            };
            let previous_witness = CanonicalPositionWitness {
                sequence: previous_sequence,
                life_seq: identity.life_seq,
                state: initial,
                wire_x,
                wire_y,
                wire_velocity_x: 0,
                wire_velocity_y: 0,
                direction: 0,
                door_seed: open_phase.seed,
                door_phase_tick: open_phase.started_tick,
            };
            let previous = position_packet(
                previous_witness,
                locked.clients[&player_id].server_energy,
                previous_sequence,
            );
            let previous = crate::arena::large_position_from_client(player_id, &previous)
                .expect("previous position");
            locked.set_client_position(player_id, previous);
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.player.ship = ship;
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(previous_sequence);
            client.last_position_server_tick = Some(open_phase.started_tick);
            client.server_sim_state = Some(initial);
            client.server_sim_anchor_tick = Some(open_phase.started_tick);
            client.server_sim_input_bits = Some(0);
            client
                .canonical_position_witnesses
                .push_back(previous_witness);
            client.canonical_movement_witness = Some(previous_witness);
            let coast = ClientInputObservation {
                bits: 0,
                direction: 0,
            };
            client.begin_actor_motion_segment();
            client
                .append_actor_motion_witness(open_phase.started_tick, previous_witness)
                .unwrap();
            for offset in 1..=FIRST_GAP {
                client
                    .recent_input_records
                    .push_back(SequencedInputObservation {
                        sequence: previous_sequence.wrapping_add(offset),
                        input: coast,
                        door_seed: open_phase.seed,
                        door_phase_tick: open_phase.started_tick,
                    });
            }
            client.last_input_seq = Some(first_sequence);
            client.last_input_record = Some(coast);
            client.server_energy
        };

        let first_replay = {
            let locked = state.lock().expect("server state");
            let client = &locked.clients[&player_id];
            replay_canonical_position_gap(
                client,
                locked.zone_map(),
                false,
                false,
                false,
                PlayerMovementUpgrades {
                    thrust: client.server_thrust_acceleration,
                    top_speed: client.server_top_speed,
                    rotation: client.server_rotation_per_second,
                    recharge: client.server_recharge_per_second,
                    live_values: true,
                },
                &[],
                first_sequence,
                open_phase.started_tick,
                spawn_x,
                spawn_y,
            )
            .expect("first gap replay")
        };
        assert!(!first_replay.relocated);
        let first_tail = *first_replay.witnesses.last().expect("first tail");
        assert_eq!((first_tail.wire_x, first_tail.wire_y), (wire_x, wire_y));
        assert_eq!(first_tail.state.position_x, exact_x);
        assert_ne!(first_tail.state.position_x, first_tail.wire_x as f32);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &position_packet(first_tail, energy, first_sequence),
                open_phase.started_tick,
            )
            .is_some(),
            "first exact replay tail must admit"
        );
        {
            let locked = state.lock().expect("server state");
            let stored = locked.clients[&player_id]
                .canonical_position_witnesses
                .back()
                .copied()
                .expect("stored first tail");
            assert_eq!(stored.state.position_x, exact_x);
            assert_eq!(stored.state.position_y, exact_y);
        }

        {
            let mut locked = state.lock().expect("server state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            let coast = ClientInputObservation {
                bits: 0,
                direction: 0,
            };
            for offset in 1..=SECOND_GAP {
                client
                    .recent_input_records
                    .push_back(SequencedInputObservation {
                        sequence: first_sequence.wrapping_add(offset),
                        input: coast,
                        door_seed: closed_phase.seed,
                        door_phase_tick: closed_phase.started_tick,
                    });
            }
            client.last_input_seq = Some(second_sequence);
            client.last_input_record = Some(coast);
        }

        let second_replay = {
            let locked = state.lock().expect("server state");
            let client = &locked.clients[&player_id];
            let upgrades = PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            };
            let exact = replay_canonical_position_gap(
                client,
                locked.zone_map(),
                false,
                false,
                false,
                upgrades,
                &[],
                second_sequence,
                closed_phase.started_tick,
                spawn_x,
                spawn_y,
            )
            .expect("second exact gap replay");
            let mut quantized = client.clone();
            let quantized_tail = quantized
                .canonical_movement_witness
                .as_mut()
                .filter(|witness| witness.sequence == first_sequence)
                .expect("first movement anchor in quantized clone");
            quantized_tail.state.position_x = quantized_tail.wire_x as f32;
            quantized_tail.state.position_y = quantized_tail.wire_y as f32;
            let rounded = replay_canonical_position_gap(
                &quantized,
                locked.zone_map(),
                false,
                false,
                false,
                upgrades,
                &[],
                second_sequence,
                closed_phase.started_tick,
                spawn_x,
                spawn_y,
            )
            .expect("quantized comparison replay");
            assert_eq!(
                exact.relocated, exact_relocates,
                "the second gap must use the retained subpixel AABB"
            );
            assert_eq!(rounded.relocated, rounded_relocates);
            assert_ne!(exact.relocated, rounded.relocated);
            assert_ne!(
                exact
                    .witnesses
                    .last()
                    .map(|witness| (witness.wire_x, witness.wire_y)),
                rounded
                    .witnesses
                    .last()
                    .map(|witness| (witness.wire_x, witness.wire_y))
            );
            exact
        };
        let second_tail = *second_replay.witnesses.last().expect("second tail");
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &position_packet(second_tail, energy, second_sequence),
                closed_phase.started_tick,
            )
            .is_some(),
            "the second gap must replay from the retained full-float tail"
        );
        let locked = state.lock().expect("server state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.last_position_sequence, Some(second_sequence));
        if exact_relocates {
            assert_eq!(client.position_history.len(), 1);
        } else {
            assert_eq!(client.position_history.len(), 2);
            assert_eq!(
                client
                    .canonical_position_witnesses
                    .back()
                    .map(|witness| witness.sequence),
                Some(second_sequence)
            );
        }
        assert_eq!(
            client.canonical_position_witnesses.len(),
            1 + FIRST_GAP as usize + SECOND_GAP as usize,
            "relocation preserves bounded weapon history while rebasing only the current movement anchor"
        );
        assert_eq!(
            client
                .last_position
                .as_deref()
                .and_then(crate::arena::large_position_coordinates),
            Some((second_tail.wire_x, second_tail.wire_y))
        );
    }

    #[test]
    fn acked_unconsumed_hostile_repel_preserves_exact_chaos_wall_bounce_cap() {
        use crate::state::{SequencedInputObservation, ServerSimMovement};

        fn position_packet(
            direction: u8,
            x: u16,
            y: u16,
            exact_x: f32,
            exact_y: f32,
            velocity_x: f32,
            velocity_y: f32,
            orientation: f32,
            energy: u16,
            sequence: u16,
        ) -> Vec<u8> {
            let mut packet = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
            packet[0] = crate::continuum_position::CLIENT_POSITION_TYPE;
            packet[1] = direction;
            packet[6..8].copy_from_slice(
                &crate::continuum_position::protocol_velocity_component(velocity_x).to_le_bytes(),
            );
            packet[8..10].copy_from_slice(&y.to_le_bytes());
            packet[12..14].copy_from_slice(&x.to_le_bytes());
            packet[14..16].copy_from_slice(
                &crate::continuum_position::protocol_velocity_component(velocity_y).to_le_bytes(),
            );
            packet[18..20].copy_from_slice(&energy.to_le_bytes());
            packet[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(sequence)
                    .to_le_bytes(),
            );
            crate::continuum_position::ClientPositionCanonicalState::from_f32(
                exact_x,
                exact_y,
                velocity_x,
                velocity_y,
                orientation,
            )
            .encode(&mut packet);
            packet
        }

        fn accepts_case(start_x: u16, start_y: u16, report_real_velocity: bool) -> bool {
            let state =
                std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
            let (tx, _rx) = std::sync::mpsc::channel();
            let player_id = crate::arena::register_connection(&state, tx);
            crate::arena::join_arena(&state, player_id, 0);
            let initial_sequence = 21_107_u16;
            let current_sequence = initial_sequence.wrapping_add(3);
            let initial = ServerSimMovement {
                position_x: start_x as f32,
                position_y: start_y as f32,
                orientation: 0.56,
                velocity_x: 124.2,
                velocity_y: -484.3,
                energy: 1_580.0,
                max_energy: 1_700.0,
            };
            let tuning = movement_tuning_from_ship(1, PlayerMovementUpgrades::default());
            let mut direct = SimIntegration {
                state: initial,
                position_x: start_x as f32,
                position_y: start_y as f32,
                bounced: false,
            };
            let exact_repel_velocity = crate::gameplay_rules::repel_velocity_after(
                crate::gameplay_rules::RulesVector::new(8_000.0, 8_000.0),
                crate::gameplay_rules::RulesVector::new(initial.position_x, initial.position_y),
                crate::arena_settings::arena_repel_speed_pixels_per_second(),
                crate::arena_settings::arena_repel_radius_pixels(),
            )
            .expect("fixture is inside the authoritative repel radius");
            {
                let mut state = state.lock().expect("server state");
                let map = state.zone_map();
                let previous_packet = position_packet(
                    crate::continuum_position::protocol_direction(initial.orientation),
                    start_x,
                    start_y,
                    initial.position_x,
                    initial.position_y,
                    initial.velocity_x,
                    initial.velocity_y,
                    initial.orientation,
                    initial.energy as u16,
                    initial_sequence,
                );
                let previous_position =
                    crate::arena::large_position_from_client(player_id, &previous_packet)
                        .expect("previous position");
                state.set_client_position(player_id, previous_position);

                let mut records = Vec::new();
                for offset in 1_u16..=3 {
                    direct = integrate_with_collision(
                        direct.state,
                        direct.position_x,
                        direct.position_y,
                        decode_input_bits(8),
                        tuning,
                        PlayerMovementEnvironment {
                            repel_active: offset > 1,
                            ..Default::default()
                        },
                        OBSERVED_POSITION_INTERVAL_SECONDS,
                        arena_ship_radius_pixels(1),
                        arena_bounce_factor(),
                        map,
                        &[u8::MAX],
                        &[],
                        1,
                    );
                    if offset == 1 {
                        direct.state.velocity_x = exact_repel_velocity.x;
                        direct.state.velocity_y = exact_repel_velocity.y;
                    }
                    records.push(SequencedInputObservation {
                        sequence: initial_sequence.wrapping_add(offset),
                        input: ClientInputObservation {
                            bits: 8,
                            direction: crate::continuum_position::protocol_direction(
                                direct.state.orientation,
                            ),
                        },
                        door_seed: u8::MAX,
                        door_phase_tick: 0,
                    });
                }

                let runtime = state
                    .active_player_runtime_identity(player_id)
                    .expect("active runtime");
                let client = state.clients.get_mut(&player_id).expect("client");
                client.player.ship = 1;
                client.server_thrust_acceleration = tuning.base_thrust_acceleration;
                client.server_top_speed = tuning.base_top_speed;
                client.server_rotation_per_second = tuning.rotation_per_second;
                client.server_recharge_per_second = tuning.recharge_per_second;
                client.server_energy = initial.energy as u16;
                client.server_max_energy = initial.max_energy as u16;
                client.last_reported_energy = initial.energy as u16;
                client.last_position_sequence = Some(initial_sequence);
                client.last_position_server_tick = Some(1_518);
                let anchor = CanonicalPositionWitness {
                    sequence: initial_sequence,
                    life_seq: client.life_seq,
                    state: initial,
                    wire_x: start_x,
                    wire_y: start_y,
                    wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                        initial.velocity_x,
                    ),
                    wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                        initial.velocity_y,
                    ),
                    direction: crate::continuum_position::protocol_direction(initial.orientation),
                    door_seed: u8::MAX,
                    door_phase_tick: 0,
                };
                client.canonical_position_witnesses.push_back(anchor);
                client.canonical_movement_witness = Some(anchor);
                client.server_sim_state = Some(initial);
                client.server_sim_anchor_tick = Some(1_518);
                client
                    .pending_repel_observations
                    .push_back(crate::state::ServerRepelObservation {
                        source: runtime,
                        event_tick: 1_518,
                        expires_at_tick: 1_743,
                        origin_x: 8_000.0,
                        origin_y: 8_000.0,
                        // Deliberately not the accepted anchor: the transient
                        // replacement-vector observation remains unconsumed while
                        // the exact delivery ACK supplies the timed environment.
                        expected_velocity_x: exact_repel_velocity.x,
                        expected_velocity_y: exact_repel_velocity.y,
                        weapon_data: 5,
                        source_frequency: 2,
                        source_ship: 0,
                        delivery_generation: 1,
                        active_steps: 135,
                        delivered_snapshots: {
                            let mut delivered = [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS];
                            delivered[0] = Some(crate::state::ServerRepelDelivery {
                                snapshot_id: 71,
                                snapshot_tick: 1_518,
                                active_steps: 135,
                                acknowledged_at_tick: Some(1_518),
                            });
                            delivered
                        },
                        delivered_snapshot_cursor: 1,
                        accepted_application: None,
                        delivered_snapshot_id: Some(71),
                        delivered_snapshot_tick: Some(1_518),
                        delivered_active_steps: Some(135),
                        delivery_acked: false,
                    });
                client.last_input_seq = Some(current_sequence);
                client.last_input_record = records.last().map(|record| record.input);
                client.recent_input_records.extend(records);
            }

            let ack = crate::snapshot_wire::SnapshotAckPacket { snapshot_id: 71 }.encode();
            crate::connection::record_snapshot_ack_at(
                &state,
                player_id,
                &ack,
                std::time::Instant::now(),
            );
            {
                let state = state.lock().expect("server state");
                let client = &state.clients[&player_id];
                assert_eq!(client.server_repel_until_tick, None);
                assert!(
                    !client.pending_repel_observations.is_empty(),
                    "the ACK must not consume or waive the exact velocity observation"
                );
            }

            let reported_velocity_y = if report_real_velocity {
                direct.state.velocity_y
            } else {
                -direct.state.velocity_y
            };
            let mut packet = position_packet(
                crate::continuum_position::protocol_direction(direct.state.orientation),
                direct.position_x.round() as u16,
                direct.position_y.round() as u16,
                direct.position_x,
                direct.position_y,
                direct.state.velocity_x,
                reported_velocity_y,
                direct.state.orientation,
                initial.energy as u16,
                current_sequence,
            );
            packet[24..26].copy_from_slice(&71_u16.to_le_bytes());
            packet[26..28].copy_from_slice(
                &crate::continuum_position::CLIENT_POSITION_REPEL_MARKER_MAGIC.to_le_bytes(),
            );
            packet.push(crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_TYPE);
            packet
                .push(crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_VERSION);
            packet.extend_from_slice(&71_u16.to_le_bytes());
            packet.extend_from_slice(&initial_sequence.wrapping_add(1).to_le_bytes());
            packet.extend_from_slice(&1_u32.to_le_bytes());
            let identity = state
                .lock()
                .expect("server state")
                .active_life_identity(player_id)
                .expect("active life");
            crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &packet, 1_524)
                .is_some()
        }

        assert!(
            accepts_case(8_214, 8_152, true),
            "the exact 500px/s Chaos wall bounce must retain the ACKed RepelSpeed environment"
        );
        assert!(
            !accepts_case(8_200, 8_200, false),
            "the same high-speed sign flip in open space must remain rejected"
        );
    }

    #[test]
    fn input_decoder_matches_shared_movement_bits() {
        for bits in 0u16..=0x1f {
            let input = decode_input_bits(bits);
            assert_eq!(input.thrust, bits & 0x01 != 0);
            assert_eq!(input.reverse, bits & 0x02 != 0);
            assert_eq!(input.rotate_left, bits & 0x04 != 0);
            assert_eq!(input.rotate_right, bits & 0x08 != 0);
            assert_eq!(input.afterburner_held, bits & 0x10 != 0);
        }
    }

    #[test]
    fn input_history_accepts_full_bounded_coalescing_window_and_rejects_beyond_it() {
        use crate::state::{ClientInputObservation, SequencedInputObservation};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.last_position_sequence = Some(u16::MAX - 15);
        for offset in 1..=MAX_OBSERVED_POSITION_SEQUENCE_STEPS {
            client
                .recent_input_records
                .push_back(SequencedInputObservation {
                    sequence: client
                        .last_position_sequence
                        .expect("position sequence")
                        .wrapping_add(offset),
                    input: ClientInputObservation {
                        bits: offset,
                        direction: (offset % 40) as u8,
                    },
                    door_seed: 0,
                    door_phase_tick: 0,
                });
        }
        let full_window_tail = client
            .last_position_sequence
            .expect("position sequence")
            .wrapping_add(MAX_OBSERVED_POSITION_SEQUENCE_STEPS);
        assert!(position_input_history_complete(client, full_window_tail));
        assert!(!position_input_history_complete(
            client,
            full_window_tail.wrapping_add(1),
        ));
    }

    #[test]
    fn sequenced_observer_uses_exact_input_instead_of_newest_unordered_arrival() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.server_sim_input_bits = Some(0x01);
        client.last_position_sequence = Some(6);
        let thrust = ClientInputObservation {
            bits: 0x01,
            direction: 0,
        };
        let newer_reverse = ClientInputObservation {
            bits: 0x02,
            direction: 0,
        };
        client.recent_input_records.extend([
            SequencedInputObservation {
                sequence: 7,
                input: thrust,
                door_seed: 0,
                door_phase_tick: 0,
            },
            SequencedInputObservation {
                sequence: 8,
                input: newer_reverse,
                door_seed: 0,
                door_phase_tick: 0,
            },
        ]);
        client.last_input_seq = Some(8);
        client.last_input_record = Some(newer_reverse);

        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8200,
            8200,
            0,
            -25,
            Some(7),
            10,
            false,
        );

        assert!(result.deviation_px.is_some_and(|px| px <= 1));
        assert!(
            client.server_sim_state.expect("tracked sim").velocity_y < 0.0,
            "sequence 7 thrust must win over the newer sequence 8 reverse"
        );
    }

    #[test]
    fn sequenced_observer_uses_sample_cadence_instead_of_receive_tick_quantization() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 200.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(100);
        client.server_sim_input_bits = Some(0);
        client.last_position_sequence = Some(20);
        let coast = ClientInputObservation {
            bits: 0,
            direction: 0,
        };
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: 21,
                input: coast,
                door_seed: 0,
                door_phase_tick: 0,
            });

        // One 60 Hz sample at 200 px/s advances 3.33 px. The receive tick is
        // deliberately quantized to 30 ms; replaying that clock would land
        // 3 px too far ahead and fail this assertion.
        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8203,
            8200,
            2000,
            0,
            Some(21),
            103,
            false,
        );

        assert!(result.deviation_px.is_some_and(|deviation| deviation <= 1));
    }

    #[test]
    fn accepted_position_replays_repel_prefix_before_retiring_crossed_window() {
        use crate::state::{
            CanonicalPositionWitness, ClientInputObservation, SequencedInputObservation,
            ServerSimMovement,
        };

        fn position_packet(witness: CanonicalPositionWitness, energy: u16) -> Vec<u8> {
            let mut packet = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
            packet[0] = crate::continuum_position::CLIENT_POSITION_TYPE;
            packet[1] = witness.direction;
            packet[6..8].copy_from_slice(&witness.wire_velocity_x.to_le_bytes());
            packet[8..10].copy_from_slice(&witness.wire_y.to_le_bytes());
            packet[12..14].copy_from_slice(&witness.wire_x.to_le_bytes());
            packet[14..16].copy_from_slice(&witness.wire_velocity_y.to_le_bytes());
            packet[18..20].copy_from_slice(&energy.to_le_bytes());
            packet[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(witness.sequence)
                    .to_le_bytes(),
            );
            crate::continuum_position::ClientPositionCanonicalState::from_f32(
                witness.state.position_x,
                witness.state.position_y,
                witness.state.velocity_x,
                witness.state.velocity_y,
                witness.state.orientation,
            )
            .encode(&mut packet);
            packet
        }

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let anchor_tick = 1_000_u32;
        let accepted_tick = anchor_tick + 20;
        let initial_sequence = 100_u16;
        let current_sequence = 112_u16;
        let repel_window = (initial_sequence, 107_u16);
        let (identity, expected, tuning, energy) = {
            let mut locked = state.lock().expect("server state");
            locked.config.sim_observe = true;
            locked.config.movement_snapback = true;
            let identity = locked.active_life_identity(player_id).expect("active life");
            let client = locked.clients.get(&player_id).expect("client present");
            let energy = client.server_energy;
            let initial = ServerSimMovement {
                position_x: 8_200.0,
                position_y: 8_200.0,
                orientation: 0.0,
                velocity_x: arena_repel_speed_pixels_per_second(),
                velocity_y: 0.0,
                energy: energy as f32,
                max_energy: client.server_max_energy as f32,
            };
            let upgrades = PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            };
            let tuning = movement_tuning_from_ship(0, upgrades);
            assert!(initial.velocity_x > tuning.base_top_speed);
            let input = ClientInputObservation {
                bits: 0,
                direction: 0,
            };
            let (expected, records) = {
                let map = locked.zone_map();
                let mut expected = SimIntegration {
                    state: initial,
                    position_x: initial.position_x,
                    position_y: initial.position_y,
                    bounced: false,
                };
                let records = (initial_sequence + 1..=current_sequence)
                    .map(|sequence| {
                        let repel_active = crate::gameplay_rules::repel_sequence_window_contains(
                            repel_window,
                            sequence,
                        );
                        expected = integrate_with_collision(
                            expected.state,
                            expected.position_x,
                            expected.position_y,
                            decode_input_bits(input.bits),
                            tuning,
                            PlayerMovementEnvironment {
                                repel_active,
                                ..Default::default()
                            },
                            OBSERVED_POSITION_INTERVAL_SECONDS,
                            arena_ship_radius_pixels(0),
                            arena_bounce_factor(),
                            map,
                            &[],
                            &[],
                            0,
                        );
                        SequencedInputObservation {
                            sequence,
                            input,
                            door_seed: 0,
                            door_phase_tick: anchor_tick,
                        }
                    })
                    .collect::<Vec<_>>();
                (expected, records)
            };
            assert_eq!(
                records
                    .iter()
                    .map(
                        |record| crate::gameplay_rules::repel_sequence_window_contains(
                            repel_window,
                            record.sequence,
                        )
                    )
                    .collect::<Vec<_>>(),
                [
                    true, true, true, true, true, true, false, false, false, false, false, false,
                ],
            );
            assert!(expected.state.velocity_x <= tuning.base_top_speed);

            let previous_witness = CanonicalPositionWitness {
                sequence: initial_sequence,
                life_seq: identity.life_seq,
                state: initial,
                wire_x: initial.position_x.round() as u16,
                wire_y: initial.position_y.round() as u16,
                wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                    initial.velocity_x,
                ),
                wire_velocity_y: 0,
                direction: 0,
                door_seed: 0,
                door_phase_tick: anchor_tick,
            };
            let previous_position = crate::arena::large_position_from_client(
                player_id,
                &position_packet(previous_witness, energy),
            )
            .expect("previous position");
            locked.set_client_position(player_id, previous_position);
            let client = locked.clients.get_mut(&player_id).expect("client present");
            client.player.ship = 0;
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(initial_sequence);
            client.last_position_server_tick = Some(anchor_tick);
            client.last_input_seq = Some(current_sequence);
            client.last_input_record = Some(input);
            client.recent_input_records.clear();
            client.recent_input_records.extend(records);
            client.canonical_position_witnesses.clear();
            client
                .canonical_position_witnesses
                .push_back(previous_witness);
            client.canonical_movement_witness = Some(previous_witness);
            client.server_sim_state = Some(initial);
            client.server_sim_anchor_tick = Some(anchor_tick);
            client.begin_actor_motion_segment();
            client
                .append_actor_motion_witness(anchor_tick, previous_witness)
                .unwrap();
            client.server_sim_input_bits = Some(0);
            client.server_repel_sequence_window = Some(repel_window);
            client.server_repel_until_tick = Some(anchor_tick + 225);
            client.server_repel_observer_until_tick = Some(anchor_tick + 225);
            client.movement_drift_x = 0.0;
            client.movement_drift_y = 0.0;
            (identity, expected, tuning, energy)
        };
        let expected_witness = CanonicalPositionWitness {
            sequence: current_sequence,
            life_seq: identity.life_seq,
            state: expected.state,
            wire_x: expected.position_x.round() as u16,
            wire_y: expected.position_y.round() as u16,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                expected.state.velocity_x,
            ),
            wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                expected.state.velocity_y,
            ),
            direction: crate::continuum_position::protocol_direction(expected.state.orientation),
            door_seed: 0,
            door_phase_tick: anchor_tick,
        };
        let packet = position_packet(expected_witness, energy);
        let relay = crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .expect("the exact mixed Repel gap must be accepted");

        assert_eq!(
            crate::arena::large_position_coordinates(&relay.position),
            Some((expected_witness.wire_x, expected_witness.wire_y)),
            "the private observer must not snap an exact canonical relay",
        );
        let locked = state.lock().expect("server state");
        let client = locked.clients.get(&player_id).expect("client present");
        assert!(client.movement_drift_x.abs() < 1.0);
        assert!(client.movement_drift_y.abs() < 1.0);
        assert!(
            client
                .server_sim_state
                .expect("tracked exact mixed Repel gap")
                .velocity_x
                <= tuning.base_top_speed
        );
        assert_eq!(client.server_repel_sequence_window, None);
        assert_eq!(client.server_repel_until_tick, None);
        assert_eq!(client.server_repel_observer_until_tick, None);
    }

    #[test]
    fn sequenced_observer_requires_every_input_in_a_position_gap() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(100);
        client.server_sim_input_bits = Some(1);
        client.last_position_sequence = Some(30);
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: 32,
                input: ClientInputObservation {
                    bits: 1,
                    direction: 0,
                },
                door_seed: 0,
                door_phase_tick: 0,
            });

        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8200,
            8200,
            0,
            0,
            Some(32),
            103,
            false,
        );

        assert_eq!(result.deviation_px, None);
        assert_eq!(result.snap_back, None);
    }

    #[test]
    fn sequenced_observer_replays_two_through_twelve_actual_core_steps() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        let initial = ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.125,
            velocity_x: 30.0,
            velocity_y: -200.0,
            energy: 1_100.0,
            max_energy: 1_700.0,
        };
        let inputs = [0x09_u16, 0x15, 0x0a, 0x25];
        let tuning = movement_tuning_from_ship(0, PlayerMovementUpgrades::default());
        let radius = arena_ship_radius_pixels(0);
        let bounce = arena_bounce_factor();
        let brick = [(512, 510)];
        let mut observed_goldens = Vec::new();

        for steps in 2_u16..=12 {
            let mut integration = SimIntegration {
                state: initial,
                position_x: 8_200.0,
                position_y: 8_200.0,
                bounced: false,
            };
            let records = (1..=steps)
                .map(|sequence| SequencedInputObservation {
                    sequence,
                    input: ClientInputObservation {
                        bits: inputs[(sequence as usize - 1) % inputs.len()],
                        direction: 5,
                    },
                    door_seed: 0,
                    door_phase_tick: 0,
                })
                .collect::<Vec<_>>();
            for record in &records {
                let stepped = integrate_with_collision(
                    integration.state,
                    integration.position_x,
                    integration.position_y,
                    decode_input_bits(record.input.bits),
                    tuning,
                    PlayerMovementEnvironment {
                        rocket_active: record.input.bits & (1 << 5) != 0,
                        ..Default::default()
                    },
                    OBSERVED_POSITION_INTERVAL_SECONDS,
                    radius,
                    bounce,
                    map,
                    &[],
                    &brick,
                    0,
                );
                integration = SimIntegration {
                    bounced: integration.bounced || stepped.bounced,
                    ..stepped
                };
            }
            observed_goldens.push((
                (integration.position_x * 100.0).round() as i32,
                (integration.position_y * 100.0).round() as i32,
                (integration.state.velocity_x * 100.0).round() as i32,
                (integration.state.velocity_y * 100.0).round() as i32,
                (integration.state.orientation * 1_000_000.0).round() as i32,
                (integration.state.energy * 100.0).round() as i32,
                integration.bounced,
            ));

            client.server_sim_state = Some(initial);
            client.server_sim_anchor_tick = Some(1_000);
            client.server_sim_input_bits = Some(inputs[0]);
            client.last_position_sequence = Some(0);
            client.recent_input_records.clear();
            client.recent_input_records.extend(records);
            client.last_input_seq = Some(steps);
            client.last_input_record = client
                .recent_input_records
                .back()
                .map(|record| record.input);
            let result = observe_simulation_inner(
                client,
                map,
                &[],
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &brick,
                Some(8_200),
                Some(8_200),
                crate::continuum_position::protocol_direction(integration.state.orientation),
                integration.position_x.round() as u16,
                integration.position_y.round() as u16,
                (integration.state.velocity_x * 10.0).round() as i16,
                (integration.state.velocity_y * 10.0).round() as i16,
                Some(steps),
                1_000 + (steps as u32 * 5 / 3),
                false,
            );
            assert!(
                result.deviation_px.is_some_and(|deviation| deviation <= 1),
                "{steps}-step replay diverged: {result:?}"
            );
        }
        assert_eq!(
            observed_goldens,
            vec![
                (820102, 819336, 3350, -20079, 125000, 109933, false),
                (820158, 819001, 3165, -19849, 133333, 110000, false),
                (820211, 819001, 2242, 11920, 125000, 110067, true),
                (820248, 819200, 2419, 11744, 133333, 110133, true),
                (820289, 819396, 2643, 11520, 125000, 110000, true),
                (820333, 819588, 2466, 11696, 133333, 110067, true),
                (820374, 819782, 2761, 11402, 125000, 110133, true),
                (820420, 819972, 2938, 11225, 133333, 110200, true),
                (820469, 820160, 3161, 11001, 125000, 110067, true),
                (820521, 820343, 2985, 11178, 133333, 110133, true),
                (820571, 820529, 3279, 10883, 125000, 110200, true),
            ],
            "N=2..12 pins evolving thrust/rotation/rocket/energy and the pre-input-order wall bounce; \
             the post-bounce velocities are damped by the authored Chaos bounce factor (BounceFactor=26)",
        );
    }

    #[test]
    fn sequenced_observer_recomputes_wormhole_gravity_each_of_two_through_twelve_steps() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let wormhole_index = map
            .tiles
            .iter()
            .enumerate()
            .find_map(|(index, &tile)| {
                let x = index % MAP_TILES;
                let y = index / MAP_TILES;
                (tile == 220
                    && (x == 0 || map.tiles[index - 1] != 220)
                    && (y == 0 || map.tiles[index - MAP_TILES] != 220))
                    .then_some(index)
            })
            .expect("chaos wormhole");
        let wormhole_x = wormhole_index % MAP_TILES;
        let wormhole_y = wormhole_index / MAP_TILES;
        assert_eq!((wormhole_x, wormhole_y), (661, 256));
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        let initial = ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_100.0,
            max_energy: 1_700.0,
        };
        let tuning = movement_tuning_from_ship(0, PlayerMovementUpgrades::default());
        let radius = arena_ship_radius_pixels(0);
        let bounce = arena_bounce_factor();
        let start_x = (wormhole_x as f32 + 2.5) * TILE_SIZE_PX + 64.0;
        let start_y = (wormhole_y as f32 + 2.5) * TILE_SIZE_PX;
        let mut goldens = Vec::new();
        for steps in 2_u16..=12 {
            let mut integration = SimIntegration {
                state: initial,
                position_x: start_x,
                position_y: start_y,
                bounced: false,
            };
            for _ in 0..steps {
                integration = integrate_with_collision(
                    integration.state,
                    integration.position_x,
                    integration.position_y,
                    decode_input_bits(0),
                    tuning,
                    PlayerMovementEnvironment::default(),
                    OBSERVED_POSITION_INTERVAL_SECONDS,
                    radius,
                    bounce,
                    map,
                    &[],
                    &[],
                    0,
                );
            }
            goldens.push((
                (integration.position_x * 100.0).round() as i32,
                (integration.position_y * 100.0).round() as i32,
                (integration.state.velocity_x * 100.0).round() as i32,
                (integration.state.velocity_y * 100.0).round() as i32,
            ));
            client.server_sim_state = Some(initial);
            client.server_sim_anchor_tick = Some(2_000);
            client.server_sim_input_bits = Some(0);
            client.last_position_sequence = Some(0);
            client.recent_input_records = (1..=steps)
                .map(|sequence| SequencedInputObservation {
                    sequence,
                    input: ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: 0,
                    door_phase_tick: 0,
                })
                .collect();
            client.last_input_seq = Some(steps);
            client.last_input_record = Some(ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            let result = observe_simulation_inner(
                client,
                map,
                &[],
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &[],
                Some(start_x as u16),
                Some(start_y as u16),
                0,
                integration.position_x.round() as u16,
                integration.position_y.round() as u16,
                (integration.state.velocity_x * 10.0).round() as i16,
                (integration.state.velocity_y * 10.0).round() as i16,
                Some(steps),
                2_000 + (steps as u32 * 5 / 3),
                false,
            );
            assert!(result.deviation_px.is_some_and(|deviation| deviation <= 1));
        }
        assert_eq!(
            goldens,
            vec![
                (1067870, 413600, -15600, 0),
                (1067610, 413600, -20100, 0),
                (1067275, 413600, -20100, 0),
                (1066940, 413600, -20100, 0),
                (1066605, 413600, -20100, 0),
                (1066270, 413600, -20100, 0),
                (1065935, 413600, -20100, 0),
                (1065600, 413600, -20100, 0),
                (1065265, 413600, -20100, 0),
                (1064930, 413600, -20100, 0),
                (1064595, 413600, -20100, 0),
            ],
        );
    }

    #[test]
    fn input_transition_is_diagnostic_and_reseeds_before_stable_comparisons() {
        use crate::state::{ClientInputObservation, SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: -19.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(10);
        client.server_sim_input_bits = Some(0x01);
        client.movement_drift_x = 9.0;
        let reverse = ClientInputObservation {
            bits: 0x02,
            direction: 0,
        };
        client
            .recent_input_records
            .push_back(SequencedInputObservation {
                sequence: 9,
                input: reverse,
                door_seed: 0,
                door_phase_tick: 0,
            });

        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8198),
            0,
            8200,
            8200,
            0,
            190,
            Some(9),
            20,
            false,
        );

        assert_eq!(result.deviation_px, None);
        assert_eq!(result.snap_back, None);
        assert_eq!(client.movement_drift_x, 0.0);
        assert_eq!(client.server_sim_input_bits, Some(0x02));
        assert_eq!(
            client
                .server_sim_state
                .expect("transition re-seed")
                .velocity_y,
            19.0
        );
    }

    #[test]
    fn simulation_tracking_retains_private_state_when_within_tolerance() {
        // The private server sim should keep tracking when it agrees with
        // the accepted report, but it must not publish an alternate position
        // into remote snapshots.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        // Seed a previous sim state at (8200, 8200) with low velocity.
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0, // no input → ship sits still
            direction: 0,
        });
        client.alive = true;

        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8200,
            8200,
            0,
            0,
            None,
            10,    // dt = 10 ticks = 0.1s
            false, // observation off
        );

        assert_eq!(result.deviation_px, Some(0));
        assert!(!result.resynced);
        let final_state = client.server_sim_state.expect("sim state retained");
        assert!(final_state.velocity_x.abs() < 0.01);
        assert!(final_state.velocity_y.abs() < 0.01);
    }

    #[test]
    fn simulation_tracking_runs_even_when_observation_logging_is_off() {
        // Observation logging is optional; the private sim model still
        // advances so future reconciliation has a continuous server-side
        // reference.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8200,
            8200,
            0,
            0,
            None,
            10,
            false,
        );
        assert_eq!(result.deviation_px, Some(0));
        assert!(!result.resynced);
        assert_eq!(client.server_sim_anchor_tick, Some(10));
    }

    #[test]
    fn simulation_tracking_resyncs_private_state_on_large_deviation() {
        // If the sim drifts more than the internal tracking threshold from
        // the accepted report, re-seed from reported. The accepted position
        // stays report-based for this frame; only the private model resets.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            // Big velocity that the sim would project well past the
            // internal resync tolerance even after friction. 1000 px/s
            // over 0.1s ≈ 100 px each axis (~141 px diagonal); the
            // ship's drag still leaves a large deviation.
            velocity_x: 1000.0,
            velocity_y: 1000.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        // Reported says the client teleported to (8280, 8280). The
        // sim seeded at (8200, 8200) with velocity (1000, 1000) integrates
        // (clamped at the ship's max speed) to roughly (8214, 8214) over
        // 0.1s. The ~93 px gap each axis ≈ 132 px diagonal is well past
        // the internal resync threshold.
        let result = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8280,
            8280,
            0,
            0,
            None,
            10,
            false,
        );
        assert!(result.resynced);
        let final_state = client.server_sim_state.expect("sim state retained");
        assert_eq!(final_state.velocity_x, 0.0);
        assert_eq!(final_state.velocity_y, 0.0);
    }

    #[test]
    fn simulation_velocity_blends_toward_reported_when_within_tolerance() {
        // Phase 2 polish (rotate_right follow-up). Seed the sim with a
        // y-velocity the client doesn't share, drive multiple
        // observations with no input, and verify the sim's velocity
        // converges toward the reported (zero) velocity instead of
        // accumulating position drift past the internal resync threshold.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        // Seed: sim thinks ship has y-velocity 30 px/s, reported says
        // 0 (client thinks ship is at rest).
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 30.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        // Drive 30 observations at 0.03s each (~1s total). The reported
        // position stays put at (8200, 8200) with zero velocity each
        // tick. The sim's initial 30 px/s y-velocity would yield ~0.9
        // px drift per observation un-blended; even within the
        // internal resync window that accumulates over many ticks. With the
        // velocity blend, sim velocity decays geometrically:
        // (1 - 1/8)^30 ≈ 0.018, so velocity_y should be well under
        // 1 px/s.
        for tick in 1u32..=45 {
            let _ = observe_simulation_inner(
                client,
                map,
                &[],
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &[],
                Some(8200),
                Some(8200),
                0,
                8200,
                8200,
                0,
                0,
                None,
                tick * 3,
                false,
            );
        }

        let final_state = client.server_sim_state.expect("sim state retained");
        assert!(
            final_state.velocity_y.abs() < 1.0,
            "velocity_y should converge toward reported 0; got {}",
            final_state.velocity_y
        );
        assert!(
            final_state.velocity_x.abs() < 0.01,
            "velocity_x should stay near 0; got {}",
            final_state.velocity_x
        );
    }

    #[test]
    fn observation_handles_skewed_and_wrapping_anchor_ticks() {
        // [REVIEW M14] A slightly-ahead anchor (clock skew) must skip the
        // comparison and re-seed cleanly — not fabricate a ~2.1B-tick
        // elapsed — while a genuine forward delta across the 31-bit wrap
        // boundary must still be observed.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        let sim_at_rest = ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        };
        client.server_sim_state = Some(sim_at_rest);
        client.server_sim_anchor_tick = Some(100); // ahead of the accepted tick
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        let skewed = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8200,
            8200,
            0,
            0,
            None,
            90, // accepted tick BEHIND the anchor
            false,
        );
        assert_eq!(
            skewed.deviation_px, None,
            "a skewed (negative) tick delta must skip the comparison"
        );
        assert!(!skewed.resynced);
        assert_eq!(
            client.server_sim_anchor_tick,
            Some(90),
            "the model re-seeds at the accepted tick"
        );

        // Forward delta across the 31-bit wrap: anchor near the top of the
        // tick space, accepted just past 0 — a real 10-tick step.
        client.server_sim_state = Some(sim_at_rest);
        client.server_sim_anchor_tick = Some(0x7fff_fffa);
        let wrapped = observe_simulation_inner(
            client,
            map,
            &[],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8200),
            Some(8200),
            0,
            8200,
            8200,
            0,
            0,
            None,
            4,
            false,
        );
        assert_eq!(
            wrapped.deviation_px,
            Some(0),
            "a forward delta across the wrap boundary must still be observed"
        );
    }

    #[test]
    fn movement_snapback_never_fires_for_honest_sign_random_jitter() {
        // Honest play wobbles around the prediction by a pixel or two with
        // random sign (u16 quantization + float rounding). The leaky
        // accumulator must keep that bounded well below the snap threshold,
        // so a real client is never bounced.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        let base = 8200u16;
        for tick in 1u32..=200 {
            let jitter = if tick % 2 == 0 { 1 } else { -1 };
            let reported_x = (base as i32 + jitter) as u16;
            let reported_y = (base as i32 - jitter) as u16;
            let result = observe_simulation_inner(
                client,
                map,
                &[],
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &[],
                Some(base),
                Some(base),
                0,
                reported_x,
                reported_y,
                0,
                0,
                None,
                tick * 3,
                false,
            );
            assert!(
                result.snap_back.is_none(),
                "honest sign-random jitter must never snap (tick {tick})"
            );
        }
        assert!(client.movement_drift_x.abs() < MOVEMENT_DRIFT_SNAP_PX);
        assert!(client.movement_drift_y.abs() < MOVEMENT_DRIFT_SNAP_PX);
    }

    #[test]
    fn rounded_position_reconciliation_counts_only_distance_beyond_the_wire_cell() {
        assert_eq!(
            reconcile_rounded_position_component(8_199.5, 8_200),
            (8_199.5, 0.0)
        );
        assert_eq!(
            reconcile_rounded_position_component(8_200.5, 8_200),
            (8_200.5, 0.0)
        );
        assert_eq!(
            reconcile_rounded_position_component(8_198.0, 8_200),
            (8_199.5, 1.5)
        );
        assert_eq!(
            reconcile_rounded_position_component(8_202.0, 8_200),
            (8_200.5, -1.5)
        );
    }

    #[test]
    fn movement_snapback_reconciles_persistent_honest_rounding_bias_once() {
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_000.0,
            max_energy: 1_700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        // A previously quantized/private baseline can sit two pixels from a
        // stable honest report. The first sample contributes the exact 1.5 px
        // beyond the wire cell, then projection reaches a compatible float;
        // the same report must not be charged again forever.
        for tick in 1_u32..=200 {
            let result = observe_simulation_inner(
                client,
                map,
                &[],
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &[],
                Some(8_202),
                Some(8_200),
                0,
                8_202,
                8_200,
                0,
                0,
                None,
                tick * 3,
                false,
            );
            assert!(
                result.snap_back.is_none(),
                "a stable quantized report must not accumulate repeatedly (tick {tick})"
            );
        }
        assert!(client.movement_drift_x.abs() < 0.001);
        assert_eq!(
            client.server_sim_state.expect("private state").position_x,
            8_201.5
        );
    }

    #[test]
    fn accepted_bomb_recoil_cannot_turn_near_threshold_honest_state_into_snapback() {
        use crate::state::{SequencedInputObservation, ServerSimMovement};

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        let ship = client.player.ship;
        let initial = ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1_000.0,
            max_energy: 1_700.0,
        };
        client.server_sim_state = Some(initial);
        client.server_sim_anchor_tick = Some(100);
        client.server_sim_input_bits = Some(0);
        client.last_position_sequence = Some(100);
        client.server_energy = initial.energy as u16;
        client.server_max_energy = initial.max_energy as u16;
        client.movement_drift_x = MOVEMENT_DRIFT_SNAP_PX - 0.5;
        for sequence in [101, 102] {
            client
                .recent_input_records
                .push_back(SequencedInputObservation {
                    sequence,
                    input: ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: 0,
                    door_phase_tick: 100 + u32::from(sequence - 100),
                });
        }
        client.last_input_record = Some(ClientInputObservation {
            bits: 0,
            direction: 0,
        });

        let recoil_speed = crate::arena_settings::arena_ship_bomb_recoil_speed(ship);
        let (heading_x, heading_y) =
            crate::continuum_position::canonical_discrete_heading_components(0.0);
        let recoil_velocity_x = -heading_x * recoil_speed;
        let recoil_velocity_y = -heading_y * recoil_speed;
        let bomb_tail = observe_simulation_for_accepted_position(
            client,
            &map,
            &[0],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8_200),
            Some(8_200),
            0,
            8_200,
            8_200,
            crate::continuum_position::protocol_velocity_component(recoil_velocity_x),
            crate::continuum_position::protocol_velocity_component(recoil_velocity_y),
            Some(101),
            101,
            Some(recoil_speed),
            false,
        );
        assert_eq!(bomb_tail.snap_back, None);
        let post_bomb = client.server_sim_state.expect("post-bomb private state");
        assert!((post_bomb.velocity_x - recoil_velocity_x).abs() <= 0.05);
        assert!((post_bomb.velocity_y - recoil_velocity_y).abs() <= 0.05);

        client.last_position_sequence = Some(101);
        let honest_next = integrate_with_collision(
            post_bomb,
            post_bomb.position_x,
            post_bomb.position_y,
            decode_input_bits(0),
            movement_tuning_from_ship(ship, PlayerMovementUpgrades::default()),
            PlayerMovementEnvironment::default(),
            OBSERVED_POSITION_INTERVAL_SECONDS,
            arena_ship_radius_pixels(ship),
            arena_bounce_factor(),
            &map,
            &[0],
            &[],
            ship,
        );
        let follow = observe_simulation_for_accepted_position(
            client,
            &map,
            &[0],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(8_200),
            Some(8_200),
            crate::continuum_position::protocol_direction(honest_next.state.orientation),
            honest_next.position_x.round() as u16,
            honest_next.position_y.round() as u16,
            crate::continuum_position::protocol_velocity_component(honest_next.state.velocity_x),
            crate::continuum_position::protocol_velocity_component(honest_next.state.velocity_y),
            Some(102),
            103,
            None,
            false,
        );
        assert_eq!(
            follow.snap_back, None,
            "an honest post-bomb tail must not add false drift near the threshold"
        );
        assert!(client.movement_drift_x.abs() < MOVEMENT_DRIFT_SNAP_PX);
        assert!(client.movement_drift_y.abs() < MOVEMENT_DRIFT_SNAP_PX);
    }

    #[test]
    fn collision_proven_rebase_resumes_validation_on_the_next_sample() {
        use crate::state::ServerSimMovement;

        const DOOR_TILE: u8 = 169;
        const DOOR_X: usize = 480;
        const DOOR_Y: usize = 558;
        let map = synthetic_dynamic_door_map(DOOR_X, DOOR_Y, DOOR_TILE);
        let closed_seed = (0..=u8::MAX)
            .find(|&seed| crate::zone::position_blocking_tile_with_door_seed(DOOR_TILE, seed))
            .expect("door 169 has a closed seed");
        let ship = 1;
        let radius = arena_ship_radius_pixels(ship);
        let initial = ServerSimMovement {
            position_x: DOOR_X as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
            position_y: DOOR_Y as f32 * TILE_SIZE_PX - radius - 0.25,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 120.0,
            energy: 1_500.0,
            max_energy: 1_700.0,
        };
        let tuning = movement_tuning_from_ship(ship, PlayerMovementUpgrades::default());
        let first = integrate_with_collision(
            initial,
            initial.position_x,
            initial.position_y,
            decode_input_bits(0),
            tuning,
            PlayerMovementEnvironment::default(),
            0.02,
            radius,
            arena_bounce_factor(),
            &map,
            &[closed_seed],
            &[],
            ship,
        );
        assert!(first.bounced, "fixture must prove a collision replay");

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client present");
        client.player.ship = ship;
        client.server_sim_state = Some(initial);
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.server_energy = initial.energy as u16;
        client.server_max_energy = initial.max_energy as u16;
        client.alive = true;

        let first_report_x = first.position_x.round() as u16 + 3;
        let first_result = observe_simulation_inner(
            client,
            &map,
            &[closed_seed],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(initial.position_x.round() as u16),
            Some(initial.position_y.round() as u16),
            0,
            first_report_x,
            first.position_y.round() as u16,
            crate::continuum_position::protocol_velocity_component(first.state.velocity_x),
            crate::continuum_position::protocol_velocity_component(first.state.velocity_y),
            None,
            2,
            false,
        );
        assert!(first_result.resynced, "proven bounce should rebase once");
        assert!(
            client.movement_drift_x.abs() >= 2.0,
            "collision rebase must preserve unexplained movement evidence"
        );

        let rebased = client.server_sim_state.expect("collision baseline");
        let second = integrate_with_collision(
            rebased,
            rebased.position_x,
            rebased.position_y,
            decode_input_bits(0),
            tuning,
            PlayerMovementEnvironment::default(),
            0.02,
            radius,
            arena_bounce_factor(),
            &map,
            &[closed_seed],
            &[],
            ship,
        );
        let second_result = observe_simulation_inner(
            client,
            &map,
            &[closed_seed],
            false,
            false,
            false,
            PlayerMovementUpgrades::default(),
            &[],
            Some(first_report_x),
            Some(first.position_y.round() as u16),
            0,
            second.position_x.round() as u16,
            second.position_y.round() as u16,
            crate::continuum_position::protocol_velocity_component(second.state.velocity_x),
            crate::continuum_position::protocol_velocity_component(second.state.velocity_y),
            None,
            4,
            false,
        );
        assert!(
            second_result
                .deviation_px
                .is_some_and(|deviation| deviation <= 1),
            "next sample must validate from the collision baseline: {second_result:?}"
        );
        assert_eq!(second_result.snap_back, None);
    }

    #[test]
    fn movement_snapback_fires_for_sustained_systematic_drift() {
        // A client that reports a couple pixels of extra ground every frame
        // beyond what the legal one-step prediction allows accumulates in
        // one direction. The backstop must eventually fire, bounce the
        // published position back toward the legal prediction (not the
        // cheated report), reset, and be able to fire again.
        use crate::state::ServerSimMovement;

        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut state_lock = state.lock().expect("server state");
        let map = state_lock.zone_map();
        let client = state_lock
            .clients
            .get_mut(&player_id)
            .expect("client present");
        client.server_sim_state = Some(ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1700.0,
        });
        client.server_sim_anchor_tick = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.alive = true;

        let mut prev = 8200u16;
        let mut first_fire_tick: Option<u32> = None;
        let mut first_snap_x: Option<u16> = None;
        let mut fires = 0u32;
        for tick in 1u32..=64 {
            let reported = prev + 2; // 2 px/frame beyond the legal prediction
            let result = observe_simulation_inner(
                client,
                map,
                &[],
                false,
                false,
                false,
                PlayerMovementUpgrades::default(),
                &[],
                Some(prev),
                Some(8200),
                0,
                reported,
                8200,
                0,
                0,
                None,
                tick * 3,
                false,
            );
            if let Some(correction) = result.snap_back {
                fires += 1;
                if first_fire_tick.is_none() {
                    first_fire_tick = Some(tick);
                    first_snap_x =
                        Some(correction.position_x.round().clamp(0.0, u16::MAX as f32) as u16);
                }
                // Backstop fired this frame, so the accumulator reset.
                assert!(client.movement_drift_x.abs() < MOVEMENT_DRIFT_SNAP_PX);
            }
            prev = reported;
        }

        let first_fire_tick = first_fire_tick.expect("sustained drift should snap back");
        assert!(
            (3..=64).contains(&first_fire_tick),
            "snap should fire within the calibrated detection window, got tick {first_fire_tick}"
        );
        let cheated_report = 8200 + 2 * first_fire_tick as u16;
        assert!(
            first_snap_x.unwrap() < cheated_report,
            "snap should bounce backward toward the legal prediction"
        );
        assert!(fires >= 1, "backstop should fire at least once");
    }
}
