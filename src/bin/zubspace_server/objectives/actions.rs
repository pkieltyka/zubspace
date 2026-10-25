use std::sync::{Arc, Mutex};

use super::engine::{
    ObjectiveLifecycleReason, ObjectiveMutation, ObjectiveMutationError, ObjectiveParticipant,
    ObjectivePlayer, apply_player_objective_lifecycle, authenticate_objective_action, claim_flag,
    evaluate_flag_win, fire_powerball, pickup_powerball, score_powerball_goal,
};
use super::packets::{
    flag_state_packet, flag_victory_packet, objective_score_packet, powerball_state_packet,
    soccer_goal_packet,
};
use super::profile::{ServerFlagProfile, ServerPowerballProfile};
use super::registry::{
    FlagIdentity, ObjectiveRegistry, ObjectiveRegistryError, ObjectiveVersion, PowerballIdentity,
    PowerballSnapshot, PowerballState,
};
use crate::objective_protocol::{
    ObjectiveActionKind, ObjectiveActionRequest, ObjectiveStateUpdate,
};
use crate::reliable::{broadcast_reliable_to_joined, send_reliable_to_runtime};
use crate::security::log_security_reject;
use crate::state::{
    CanonicalPositionWitness, PendingObjectiveAction, PendingReliableGameplayMutation,
    PendingReliableGameplayMutationKind, ReliableLifecycleMutationOutcome, ServerState,
};

const OBJECTIVE_POSITION_RECOVERY_TICKS: u32 = 12;
const OBJECTIVE_POSITION_CAUSAL_WINDOW: u16 = 12;

#[cfg(test)]
pub(crate) fn relay_objective_action(
    shared: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
) {
    let reliable_delivery_id = ObjectiveActionRequest::parse(payload)
        .map(|request| request.action_id)
        .unwrap_or(0);
    relay_objective_action_at_delivery(shared, player_id, reliable_delivery_id, payload);
}

pub(crate) fn relay_objective_action_at_delivery(
    shared: &Arc<Mutex<ServerState>>,
    player_id: u16,
    reliable_delivery_id: u32,
    payload: &[u8],
) -> bool {
    let Ok(request) = ObjectiveActionRequest::parse(payload) else {
        log_security_reject(player_id, "objective", "malformed_authenticated_action");
        return true;
    };
    tracing::debug!(
        player_id,
        kind = ?request.kind,
        action_id = request.action_id,
        life_seq = request.life_seq,
        arena_stream_id = request.arena_stream_id,
        position_sequence = request.position_sequence,
        "objective action received"
    );
    let queued_at_tick = crate::wire::current_tick();
    {
        let mut state = shared.lock().expect("server state should not be poisoned");
        let Some(identity) = state.active_player_runtime_identity(player_id) else {
            log_security_reject(player_id, "objective", "inactive_authenticated_action");
            return true;
        };
        let Some(client) = state.clients.get(&player_id) else {
            return true;
        };
        if request.player_id != player_id
            || request.server_session_id != crate::wire::server_session_id()
            || request.life_seq != identity.life.life_seq
            || request.arena_stream_id != client.arena_stream_id
        {
            log_security_reject(player_id, "objective", "stale_authenticated_action");
            return true;
        }
        let queued = state.queue_objective_action_for_runtime(
            identity,
            reliable_delivery_id,
            request,
            queued_at_tick,
        );
        if !queued {
            log_security_reject(player_id, "objective", "reliable_gameplay_queue_capacity");
        }
        queued
    }
}

#[cfg(test)]
pub(crate) fn process_pending_objective_actions(
    shared: &Arc<Mutex<ServerState>>,
    tick: u32,
) -> usize {
    let pending_actions = shared
        .lock()
        .expect("server state should not be poisoned")
        .objective_action_heads();
    pending_actions
        .into_iter()
        .filter(|(sequenced, _)| {
            process_reliable_objective_head(shared, sequenced.clone(), tick)
                != ReliableLifecycleMutationOutcome::Deferred
        })
        .count()
}

pub(crate) fn process_reliable_objective_head(
    shared: &Arc<Mutex<ServerState>>,
    sequenced: PendingReliableGameplayMutation,
    tick: u32,
) -> ReliableLifecycleMutationOutcome {
    let PendingReliableGameplayMutationKind::Objective(pending) = &sequenced.kind else {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    let pending = *pending;
    let (broadcasts, rejection, outcome) = {
        let mut state = shared.lock().expect("server state should not be poisoned");
        match apply_pending_request(&mut state, pending, tick) {
            PendingObjectiveOutcome::Deferred => {
                tracing::debug!(
                    player_id = pending.identity.player_id(),
                    kind = ?pending.request.kind,
                    action_id = pending.request.action_id,
                    position_sequence = pending.request.position_sequence,
                    queued_at_tick = pending.queued_at_tick,
                    tick,
                    "objective action deferred: no canonical position witness yet"
                );
                return ReliableLifecycleMutationOutcome::Deferred;
            }
            PendingObjectiveOutcome::Stale => {
                tracing::debug!(
                    player_id = pending.identity.player_id(),
                    kind = ?pending.request.kind,
                    action_id = pending.request.action_id,
                    "objective action discarded: runtime no longer active"
                );
                let removed = state.pop_reliable_gameplay_terminal(&sequenced);
                debug_assert!(removed, "terminal objective head must remain resident");
                (
                    Vec::new(),
                    None,
                    ReliableLifecycleMutationOutcome::TerminalNoop,
                )
            }
            PendingObjectiveOutcome::Terminal(Ok(packets)) => {
                let removed = state.pop_reliable_gameplay_terminal(&sequenced);
                debug_assert!(removed, "terminal objective head must remain resident");
                (packets, None, ReliableLifecycleMutationOutcome::Applied)
            }
            PendingObjectiveOutcome::Terminal(Err((error, rejection))) => {
                let removed = state.pop_reliable_gameplay_terminal(&sequenced);
                debug_assert!(removed, "terminal objective head must remain resident");
                tracing::debug!(
                    player_id = pending.identity.player_id(),
                    ?error,
                    "ignored rejected objective action"
                );
                (
                    Vec::new(),
                    rejection.map(|packet| (pending.identity, packet)),
                    ReliableLifecycleMutationOutcome::TerminalNoop,
                )
            }
        }
    };
    for packet in broadcasts {
        broadcast_reliable_to_joined(shared, packet);
    }
    if let Some((identity, packet)) = rejection {
        send_reliable_to_runtime(shared, identity, packet);
    }
    outcome
}

pub(crate) fn objective_action_rejection_packets(
    state: &ServerState,
    pending: &[PendingObjectiveAction],
    tick: u32,
) -> Vec<Vec<u8>> {
    objective_action_rejection_packets_for_registry(&state.objectives, pending, tick)
}

pub(crate) fn objective_action_rejection_packets_for_registry(
    registry: &ObjectiveRegistry,
    pending: &[PendingObjectiveAction],
    tick: u32,
) -> Vec<Vec<u8>> {
    pending
        .iter()
        .filter_map(|objective| {
            current_state_packet_for_registry(registry, objective.request, tick)
        })
        .collect()
}

enum PendingObjectiveOutcome {
    Deferred,
    Stale,
    Terminal(Result<Vec<Vec<u8>>, (ObjectiveMutationError, Option<Vec<u8>>)>),
}

fn apply_pending_request(
    state: &mut ServerState,
    pending: PendingObjectiveAction,
    tick: u32,
) -> PendingObjectiveOutcome {
    if !state.player_runtime_matches(pending.identity) {
        return PendingObjectiveOutcome::Stale;
    }
    let player_id = pending.identity.player_id();
    let client = &state.clients[&player_id];
    let witness = client
        .canonical_position_witnesses
        .iter()
        .rev()
        .find(|witness| {
            witness.life_seq == pending.request.life_seq
                && witness.sequence == pending.request.position_sequence
        })
        .copied();
    let witness = witness.filter(|_| {
        client.last_position_sequence.is_some_and(|tail| {
            let behind = tail.wrapping_sub(pending.request.position_sequence);
            behind < (1 << 15) && behind <= OBJECTIVE_POSITION_CAUSAL_WINDOW
        })
    });
    if witness.is_none() {
        let last = client.last_position_sequence;
        let request_still_forward = last.is_none_or(|last| {
            let advance = pending.request.position_sequence.wrapping_sub(last);
            advance != 0 && advance < (1 << 15)
        });
        if request_still_forward
            && crate::continuum::continuum_tick_elapsed(tick, pending.queued_at_tick)
                <= OBJECTIVE_POSITION_RECOVERY_TICKS
        {
            return PendingObjectiveOutcome::Deferred;
        }
    }
    let Some(player) =
        witness.and_then(|witness| objective_player_for_witness(state, pending.identity, witness))
    else {
        return PendingObjectiveOutcome::Terminal(Err((
            ObjectiveMutationError::StalePosition,
            current_state_packet(state, pending.request, tick),
        )));
    };
    PendingObjectiveOutcome::Terminal(apply_request(state, player, pending.request, tick))
}

/// Runs a flag claim plus its win evaluation through one set of engine
/// preconditions. Shared by the authenticated action pipeline and the legacy
/// `0x13` relay so the two cannot drift apart.
fn apply_flag_claim(
    state: &mut ServerState,
    player: ObjectivePlayer,
    identity: FlagIdentity,
    expected_version: ObjectiveVersion,
    tick: u32,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let mut staged = state.objectives.clone();
    let claimed = claim_flag(&mut staged, player, identity, expected_version, tick)?;
    let participants = objective_participants(state);
    let mut mutation = ObjectiveMutation {
        flags: vec![claimed],
        ..ObjectiveMutation::default()
    };
    merge_flag_win(
        &mut mutation,
        evaluate_flag_win(&mut staged, &participants, tick)?,
    );
    state.objectives = staged;
    Ok(mutation)
}

/// Releases every flag the player carries under the profile's requested-drop
/// policy, plus any resulting win. The engine scopes the `Requested` lifecycle
/// reason to flags, so powerballs stay untouched.
fn apply_requested_flag_drop(
    state: &mut ServerState,
    player: ObjectivePlayer,
    tick: u32,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let participants = objective_participants(state);
    let map = state.zone_map();
    let mut staged = state.objectives.clone();
    let mut mutation = apply_player_objective_lifecycle(
        &mut staged,
        player,
        ObjectiveLifecycleReason::Requested,
        tick,
        &participants,
        |x, y| {
            let tile = map
                .tiles
                .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                .copied()
                .unwrap_or(20);
            super::FlagDropTile::from_map_tile(tile, map.no_flag_drops_at(x, y))
        },
        |x, y| {
            map.tiles
                .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                .copied()
                .is_none_or(|tile| tile != 0)
        },
        |x, y| crate::zone::tile_at_world(map, x, y) == 172,
    )?;
    merge_flag_win(
        &mut mutation,
        evaluate_flag_win(&mut staged, &participants, tick)?,
    );
    state.objectives = staged;
    Ok(mutation)
}

fn apply_request(
    state: &mut ServerState,
    player: ObjectivePlayer,
    request: ObjectiveActionRequest,
    tick: u32,
) -> Result<Vec<Vec<u8>>, (ObjectiveMutationError, Option<Vec<u8>>)> {
    authenticate_objective_action(&mut state.objectives, player, request)
        .map_err(|error| (error, current_state_packet(state, request, tick)))?;
    let expected_version = ObjectiveVersion::new(request.expected_version)
        .ok_or((ObjectiveMutationError::StaleObjective, None))?;
    let mutation = match request.kind {
        ObjectiveActionKind::FlagClaim => apply_flag_claim(
            state,
            player,
            FlagIdentity {
                id: request.objective_id,
                generation: request.objective_generation,
            },
            expected_version,
            tick,
        )
        .map_err(|error| (error, current_state_packet(state, request, tick)))?,
        ObjectiveActionKind::FlagDrop => {
            let anchor = state
                .objectives
                .flag(request.objective_id)
                .filter(|flag| {
                    flag.identity.generation == request.objective_generation
                        && flag.version == expected_version
                })
                .ok_or((
                    ObjectiveMutationError::StaleObjective,
                    current_state_packet(state, request, tick),
                ))?;
            if !matches!(anchor.state, super::FlagState::Carried { carrier, .. } if carrier == player.runtime)
            {
                return Err((
                    ObjectiveMutationError::InvalidState,
                    current_state_packet(state, request, tick),
                ));
            }
            apply_requested_flag_drop(state, player, tick)
                .map_err(|error| (error, current_state_packet(state, request, tick)))?
        }
        ObjectiveActionKind::BallPickup => {
            let identity = ball_identity(request);
            let door_seeds = state.server_door_seeds(tick);
            let brick_tiles = blocking_brick_tiles_for_frequency(state, player.frequency);
            let target = state
                .objectives
                .powerball_motion(identity.id)
                .map(|motion| {
                    (
                        motion.position_x as f32 / super::PowerballMotion::TILE_SCALE as f32,
                        motion.position_y as f32 / super::PowerballMotion::TILE_SCALE as f32,
                    )
                });
            let line_of_sight = state
                .objectives
                .powerball(identity.id)
                .and_then(|ball| match ball.state {
                    PowerballState::OnMap { x, y, .. } => {
                        let (target_x, target_y) =
                            target.unwrap_or((f32::from(x) / 16.0, f32::from(y) / 16.0));
                        Some(ball_line_is_clear(
                            state.zone_map(),
                            f32::from(player.position_x) / 16.0,
                            f32::from(player.position_y) / 16.0,
                            target_x,
                            target_y,
                            &door_seeds,
                            &brick_tiles,
                        ))
                    }
                    _ => None,
                })
                .unwrap_or(false);
            let ball = pickup_powerball(
                &mut state.objectives,
                player,
                identity,
                expected_version,
                tick,
                line_of_sight,
            )
            .map_err(|error| (error, current_state_packet(state, request, tick)))?;
            let pickup_delay =
                crate::arena_settings::arena_ship_weapon_fire_delay_ticks(player.ship, 3);
            if let Some(client) = state.clients.get_mut(&player.runtime.player_id()) {
                client.extend_objective_weapon_lock(tick, pickup_delay);
            }
            ObjectiveMutation {
                powerballs: vec![ball],
                ..ObjectiveMutation::default()
            }
        }
        ObjectiveActionKind::BallFire => {
            let client = &state.clients[&player.runtime.player_id()];
            if client.objective_weapon_lock_active(tick)
                || client.accepted_weapon_witness_consumed(request.position_sequence)
            {
                return Err((
                    ObjectiveMutationError::PickupLocked,
                    current_state_packet(state, request, tick),
                ));
            }
            let identity = ball_identity(request);
            let mut staged = state.objectives.clone();
            let ball = fire_powerball(&mut staged, player, identity, expected_version, tick, false)
                .map_err(|error| (error, current_state_packet(state, request, tick)))?;
            let PowerballState::OnMap { x, y, .. } = ball.state else {
                unreachable!("accepted powerball fire must release on map")
            };
            let ServerPowerballProfile::Enabled(profile) = staged.profile().powerballs.clone()
            else {
                return Err((ObjectiveMutationError::Disabled, None));
            };
            let mutation = if crate::zone::tile_at_world(state.zone_map(), x, y) == 172
                && super::soccer::soccer_goal_counts_world(
                    x,
                    y,
                    player.frequency,
                    profile.policy.soccer_mode,
                ) {
                let participants = objective_participants(state);
                let map = state.zone_map();
                score_powerball_goal(
                    &mut staged,
                    ball.identity,
                    ball.version,
                    player.frequency,
                    x,
                    y,
                    &participants,
                    tick,
                    |tile_x, tile_y| {
                        map.tiles
                            .get(usize::from(tile_y) * crate::zone::MAP_TILES + usize::from(tile_x))
                            .copied()
                            .is_none_or(|tile| tile != 0)
                    },
                )
                .map_err(|error| (error, current_state_packet(state, request, tick)))?
            } else {
                ObjectiveMutation {
                    powerballs: vec![ball],
                    ..ObjectiveMutation::default()
                }
            };
            state.objectives = staged;
            if let Some(client) = state.clients.get_mut(&player.runtime.player_id()) {
                client.extend_objective_weapon_lock(tick, 50);
                client.remember_objective_ball_fire_witness(request.position_sequence);
            }
            mutation
        }
        ObjectiveActionKind::BallGoal => {
            let identity = ball_identity(request);
            let ball = state
                .objectives
                .powerball(identity.id)
                .filter(|ball| ball.identity == identity && ball.version == expected_version)
                .ok_or((
                    ObjectiveMutationError::StaleObjective,
                    current_state_packet(state, request, tick),
                ))?;
            let PowerballState::OnMap { .. } = ball.state else {
                return Err((
                    ObjectiveMutationError::InvalidState,
                    current_state_packet(state, request, tick),
                ));
            };
            let motion = state
                .objectives
                .powerball_motion(identity.id)
                .filter(|motion| motion.last_touch == Some(player.runtime))
                .ok_or((
                    ObjectiveMutationError::InvalidState,
                    current_state_packet(state, request, tick),
                ))?;
            let ServerPowerballProfile::Enabled(profile) =
                state.objectives.profile().powerballs.clone()
            else {
                return Err((ObjectiveMutationError::Disabled, None));
            };
            let (world_x, world_y) = motion.wire_position();
            if crate::zone::tile_at_world(state.zone_map(), world_x, world_y) != 172
                || !super::soccer::soccer_goal_counts_world(
                    world_x,
                    world_y,
                    player.frequency,
                    profile.policy.soccer_mode,
                )
            {
                return Err((
                    ObjectiveMutationError::InvalidState,
                    current_state_packet(state, request, tick),
                ));
            }
            let participants = objective_participants(state);
            let map = state.zone_map();
            let mut staged = state.objectives.clone();
            score_powerball_goal(
                &mut staged,
                identity,
                expected_version,
                motion.last_touch_frequency.unwrap_or(player.frequency),
                world_x,
                world_y,
                &participants,
                tick,
                |tile_x, tile_y| {
                    map.tiles
                        .get(usize::from(tile_y) * crate::zone::MAP_TILES + usize::from(tile_x))
                        .copied()
                        .is_none_or(|tile| tile != 0)
                },
            )
            .map(|mutation| {
                state.objectives = staged;
                mutation
            })
            .map_err(|error| (error, current_state_packet(state, request, tick)))?
        }
    };
    sync_player_flag_counts(state);
    Ok(mutation_packets(state, &mutation, tick, request.action_id))
}

/// Executes a legacy `0x13` flag claim. The legacy wire request names only a
/// reusable u16 id, so the server supplies the registry identity, generation and
/// version it currently holds and lets the engine's own preconditions decide:
/// the flag must be live and not already owned by the claimant's team, the
/// player must be active, and the claimant's authoritative hull must overlap the
/// flag box under this zone's authored claim radius. A duplicated or reordered
/// packet therefore lands on the current flag's state check instead of acting on
/// a different generation's object.
pub(crate) fn legacy_flag_claim_packets(
    state: &mut ServerState,
    player_id: u16,
    flag_id: u16,
    tick: u32,
) -> Result<Vec<Vec<u8>>, &'static str> {
    let (identity, expected_version) = state
        .objectives
        .flag(flag_id)
        .map(|flag| (flag.identity, flag.version))
        .ok_or("unknown_claim_id")?;
    let player = objective_player(state, player_id).ok_or("inactive_claim")?;
    let mutation = apply_flag_claim(state, player, identity, expected_version, tick)
        .map_err(legacy_claim_reason)?;
    sync_player_flag_counts(state);
    Ok(mutation_packets(state, &mutation, tick, 0))
}

/// Executes a legacy `0x15` drop-everything request. The client may only ask the
/// server to release what it carries; where each flag lands is the authored
/// profile's requested-drop policy, not the request.
pub(crate) fn legacy_flag_drop_packets(
    state: &mut ServerState,
    player_id: u16,
    tick: u32,
) -> Result<Vec<Vec<u8>>, &'static str> {
    // The engine skips a disabled flag profile without reporting an error, so a
    // legacy drop in a zone that never authored flags has to be reported as
    // policy closed here. Otherwise it lands on `drop_nothing_carried`, which
    // conflates a closed policy with a carrier that genuinely held nothing.
    if matches!(
        state.objectives.profile().flags,
        ServerFlagProfile::Disabled
    ) {
        return Err("drop_policy_disabled");
    }
    let player = objective_player(state, player_id).ok_or("inactive_drop")?;
    let mutation = apply_requested_flag_drop(state, player, tick).map_err(|error| {
        match error {
            // A carrier whose authored drop behaviour needs a spawn and finds the
            // reserve full is a drop failure, not a claim transition failure.
            ObjectiveMutationError::Registry(ObjectiveRegistryError::SpawnUnavailable) => {
                "drop_spawn_unavailable"
            }
            error => legacy_claim_reason(error),
        }
    })?;
    if mutation.flags.is_empty() {
        return Err("drop_nothing_carried");
    }
    sync_player_flag_counts(state);
    Ok(mutation_packets(state, &mutation, tick, 0))
}

fn legacy_claim_reason(error: ObjectiveMutationError) -> &'static str {
    match error {
        ObjectiveMutationError::Disabled => "claim_policy_disabled",
        ObjectiveMutationError::InactivePlayer | ObjectiveMutationError::StalePlayer => {
            "inactive_claim"
        }
        ObjectiveMutationError::StalePosition => "stale_claim_position",
        ObjectiveMutationError::DuplicateAction => "duplicate_claim",
        ObjectiveMutationError::UnknownObjective => "unknown_claim_id",
        ObjectiveMutationError::StaleObjective => "stale_claim",
        ObjectiveMutationError::InvalidState => "claim_invalid_state",
        ObjectiveMutationError::OwnedByTeam => "claim_owned_by_team",
        ObjectiveMutationError::CarryLimit => "claim_carry_limit",
        ObjectiveMutationError::OutOfRange => "claim_out_of_range",
        ObjectiveMutationError::BlockedLineOfSight
        | ObjectiveMutationError::PickupLocked
        | ObjectiveMutationError::UnsupportedFrequency
        | ObjectiveMutationError::Registry(_) => "claim_transition_failed",
    }
}

fn objective_player_for_witness(
    state: &ServerState,
    runtime: crate::authority_identity::PlayerRuntimeIdentity,
    witness: CanonicalPositionWitness,
) -> Option<ObjectivePlayer> {
    if !state.player_runtime_matches(runtime) || witness.life_seq != runtime.life.life_seq {
        return None;
    }
    let player_id = runtime.player_id();
    let client = state.clients.get(&player_id)?;
    Some(ObjectivePlayer {
        runtime,
        arena_stream_id: client.arena_stream_id,
        frequency: client.player.frequency,
        ship: client.player.ship,
        alive: client.alive,
        safe: crate::zone::world_position_in_safe_tile_for_map(
            state.zone_map(),
            witness.wire_x,
            witness.wire_y,
        ),
        attached: state.turret_links.contains_key(&player_id),
        position_x: witness.wire_x,
        position_y: witness.wire_y,
        velocity_x: witness.wire_velocity_x,
        velocity_y: witness.wire_velocity_y,
        direction: witness.direction,
        exact_position_x: witness.state.position_x,
        exact_position_y: witness.state.position_y,
        exact_velocity_x: witness.state.velocity_x,
        exact_velocity_y: witness.state.velocity_y,
        exact_orientation: witness.state.orientation,
        position_sequence: witness.sequence,
    })
}

fn ball_line_is_clear(
    map: &crate::zone::ZoneMap,
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    door_seeds: &[u8],
    brick_tiles: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    let delta_x = to_x - from_x;
    let delta_y = to_y - from_y;
    let max_distance = delta_x.hypot(delta_y);
    if !max_distance.is_finite() || max_distance <= 0.0 {
        return true;
    }
    let direction_x = delta_x / max_distance;
    let direction_y = delta_y / max_distance;
    let unit_step_x = (1.0 + (direction_y / direction_x).powi(2)).sqrt();
    let unit_step_y = (1.0 + (direction_x / direction_y).powi(2)).sqrt();
    let mut check_x = from_x.floor() as i32;
    let mut check_y = from_y.floor() as i32;
    let (step_x, mut travel_x) = if direction_x < 0.0 {
        (-1, (from_x - check_x as f32) * unit_step_x)
    } else {
        (1, (check_x as f32 + 1.0 - from_x) * unit_step_x)
    };
    let (step_y, mut travel_y) = if direction_y < 0.0 {
        (-1, (from_y - check_y as f32) * unit_step_y)
    } else {
        (1, (check_y as f32 + 1.0 - from_y) * unit_step_y)
    };
    let mut distance = 0.0;
    while distance < max_distance {
        if travel_x < travel_y {
            check_x += step_x;
            distance = travel_x;
            travel_x += unit_step_x;
        } else {
            check_y += step_y;
            distance = travel_y;
            travel_y += unit_step_y;
        }
        if distance <= max_distance
            && tile_blocks_ball_line(map, check_x, check_y, door_seeds, brick_tiles)
        {
            return false;
        }
    }
    true
}

fn tile_blocks_ball_line(
    map: &crate::zone::ZoneMap,
    tile_x: i32,
    tile_y: i32,
    door_seeds: &[u8],
    brick_tiles: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    let (Ok(tile_x), Ok(tile_y)) = (u16::try_from(tile_x), u16::try_from(tile_y)) else {
        return true;
    };
    if tile_x >= crate::zone::MAP_TILES as u16 || tile_y >= crate::zone::MAP_TILES as u16 {
        return true;
    }
    if brick_tiles.contains(&(tile_x, tile_y)) {
        return true;
    }
    let index = usize::from(tile_y) * crate::zone::MAP_TILES + usize::from(tile_x);
    map.tiles
        .get(index)
        .copied()
        .is_none_or(|tile| crate::zone::position_blocking_tile_for_door_seeds(tile, door_seeds))
}

fn ball_identity(request: ObjectiveActionRequest) -> PowerballIdentity {
    PowerballIdentity {
        id: request.objective_id as u8,
        generation: request.objective_generation,
    }
}

pub(crate) fn objective_player(state: &ServerState, player_id: u16) -> Option<ObjectivePlayer> {
    if let Some(runtime) = state.active_player_runtime_identity(player_id) {
        let client = state.clients.get(&player_id)?;
        if let Some(witness) = client
            .canonical_movement_witness
            .filter(|witness| witness.life_seq == runtime.life.life_seq)
        {
            return objective_player_for_witness(state, runtime, witness);
        }
        // Legacy/test fallback for a player that has a retained client-format
        // position but predates the exact canonical witness path.
        let position = crate::continuum_position::parse_client_position_snapshot(
            client.last_position.as_deref()?,
        )?;
        let position_sequence = position.client_sequence?;
        return Some(ObjectivePlayer {
            runtime,
            arena_stream_id: client.arena_stream_id,
            frequency: client.player.frequency,
            ship: client.player.ship,
            alive: client.alive,
            safe: crate::zone::world_position_in_safe_tile_for_map(
                state.zone_map(),
                position.x,
                position.y,
            ),
            attached: state.turret_links.contains_key(&player_id),
            position_x: position.x,
            position_y: position.y,
            velocity_x: position.velocity_x,
            velocity_y: position.velocity_y,
            direction: position.direction,
            exact_position_x: f32::from(position.x),
            exact_position_y: f32::from(position.y),
            exact_velocity_x: crate::continuum_position::protocol_velocity_pixels_per_second(
                position.velocity_x,
            ),
            exact_velocity_y: crate::continuum_position::protocol_velocity_pixels_per_second(
                position.velocity_y,
            ),
            exact_orientation: crate::continuum_position::protocol_direction_to_orientation(
                position.direction,
            ),
            position_sequence,
        });
    }
    let bot = state.bot(player_id)?;
    let x = bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16;
    let y = bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16;
    Some(ObjectivePlayer {
        runtime: bot.runtime_identity(),
        arena_stream_id: 1,
        frequency: bot.player.frequency,
        ship: bot.player.ship,
        alive: bot.alive(),
        safe: crate::zone::world_position_in_safe_tile_for_map(state.zone_map(), x, y),
        attached: false,
        position_x: x,
        position_y: y,
        velocity_x: (bot.velocity_x * 10.0)
            .round()
            .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
        velocity_y: (bot.velocity_y * 10.0)
            .round()
            .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
        direction: bot.direction,
        exact_position_x: bot.x,
        exact_position_y: bot.y,
        exact_velocity_x: bot.velocity_x,
        exact_velocity_y: bot.velocity_y,
        exact_orientation: crate::continuum_position::protocol_direction_to_orientation(
            bot.direction,
        ),
        position_sequence: bot.objective_position_sequence,
    })
}

pub(crate) fn objective_participants(state: &ServerState) -> Vec<ObjectiveParticipant> {
    let mut participants = state
        .clients
        .iter()
        .filter_map(|(&player_id, client)| {
            (client.joined && client.player.ship < 8).then(|| ObjectiveParticipant {
                player_id,
                frequency: client.player.frequency,
                safe: client.objective_last_safe,
            })
        })
        .collect::<Vec<_>>();
    participants.extend(state.bots.iter().filter_map(|bot| {
        (bot.alive() && bot.player.ship < 8).then(|| ObjectiveParticipant {
            player_id: bot.player.id,
            frequency: bot.player.frequency,
            safe: crate::zone::world_position_in_safe_tile_for_map(
                state.zone_map(),
                bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            ),
        })
    }));
    participants
}

/// Applies bot objective contacts through the same registry transitions and
/// score packet construction used by authenticated human actions. Shipped
/// disabled profiles leave this path inert.
pub(crate) fn step_server_bot_objectives(state: &mut ServerState, tick: u32) -> Vec<Vec<u8>> {
    if state.objectives.profile().flags.count() == 0
        && state.objectives.profile().powerballs.count() == 0
    {
        return Vec::new();
    }
    let bot_ids = state
        .bots
        .iter()
        .filter(|bot| bot.alive())
        .map(|bot| bot.player.id)
        .collect::<Vec<_>>();
    let mut packets = Vec::new();
    for bot_id in bot_ids {
        let Some(player) = objective_player(state, bot_id) else {
            continue;
        };

        for flag in state.objectives.replay().flags {
            if !matches!(flag.state, super::FlagState::OnMap { .. }) {
                continue;
            }
            let mut staged = state.objectives.clone();
            let Ok(claimed) = claim_flag(&mut staged, player, flag.identity, flag.version, tick)
            else {
                continue;
            };
            let participants = objective_participants(state);
            let mut mutation = ObjectiveMutation {
                flags: vec![claimed],
                ..ObjectiveMutation::default()
            };
            if let Ok(win) = evaluate_flag_win(&mut staged, &participants, tick) {
                merge_flag_win(&mut mutation, win);
            }
            state.objectives = staged;
            sync_player_flag_counts(state);
            packets.extend(mutation_packets(state, &mutation, tick, 0));
        }

        let carried = state.objectives.carried_powerball_ids(player.runtime);
        if let Some(ball_id) = carried.first().copied() {
            if let Some(ball) = state.objectives.powerball(ball_id) {
                let mut staged = state.objectives.clone();
                if let Ok(released) = fire_powerball(
                    &mut staged,
                    player,
                    ball.identity,
                    ball.version,
                    tick,
                    false,
                ) {
                    state.objectives = staged;
                    packets.extend(mutation_packets(
                        state,
                        &ObjectiveMutation {
                            powerballs: vec![released],
                            ..ObjectiveMutation::default()
                        },
                        tick,
                        0,
                    ));
                }
            }
            continue;
        }
        for ball in state.objectives.replay().powerballs {
            if !matches!(ball.state, PowerballState::OnMap { .. }) {
                continue;
            }
            let mut staged = state.objectives.clone();
            let Ok(claimed) =
                pickup_powerball(&mut staged, player, ball.identity, ball.version, tick, true)
            else {
                continue;
            };
            state.objectives = staged;
            packets.extend(mutation_packets(
                state,
                &ObjectiveMutation {
                    powerballs: vec![claimed],
                    ..ObjectiveMutation::default()
                },
                tick,
                0,
            ));
            break;
        }
    }
    packets
}

pub(crate) fn player_lifecycle_packets(
    state: &mut ServerState,
    player_id: u16,
    killer_id: Option<u16>,
    reason: ObjectiveLifecycleReason,
    tick: u32,
) -> Result<Vec<Vec<u8>>, ObjectiveRegistryError> {
    let Some(player) = objective_player(state, player_id) else {
        return Ok(Vec::new());
    };
    player_lifecycle_packets_for_context(state, player, killer_id, reason, tick)
}

pub(crate) fn player_lifecycle_packets_for_context(
    state: &mut ServerState,
    player: ObjectivePlayer,
    killer_id: Option<u16>,
    reason: ObjectiveLifecycleReason,
    tick: u32,
) -> Result<Vec<Vec<u8>>, ObjectiveRegistryError> {
    let map = state.zone_map();
    player_lifecycle_packets_for_context_with_tiles(
        state,
        player,
        killer_id,
        reason,
        tick,
        |x, y| {
            let tile = map
                .tiles
                .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                .copied()
                .unwrap_or(20);
            super::FlagDropTile::from_map_tile(tile, map.no_flag_drops_at(x, y))
        },
        |x, y| {
            map.tiles
                .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                .copied()
                .is_none_or(|tile| tile != 0)
        },
        |x, y| crate::zone::tile_at_world(map, x, y) == 172,
    )
}

pub(crate) fn player_lifecycle_packets_for_context_with_tiles(
    state: &mut ServerState,
    player: ObjectivePlayer,
    killer_id: Option<u16>,
    reason: ObjectiveLifecycleReason,
    tick: u32,
    mut flag_drop_tile: impl FnMut(u16, u16) -> super::FlagDropTile,
    mut powerball_tile_is_occupied: impl FnMut(u16, u16) -> bool,
    mut powerball_tile_is_goal: impl FnMut(u16, u16) -> bool,
) -> Result<Vec<Vec<u8>>, ObjectiveRegistryError> {
    player_lifecycle_packets_for_context_impl(
        state,
        player,
        killer_id,
        reason,
        tick,
        &mut flag_drop_tile,
        &mut powerball_tile_is_occupied,
        &mut powerball_tile_is_goal,
    )
}

fn player_lifecycle_packets_for_context_impl(
    state: &mut ServerState,
    player: ObjectivePlayer,
    killer_id: Option<u16>,
    reason: ObjectiveLifecycleReason,
    tick: u32,
    flag_drop_tile: &mut impl FnMut(u16, u16) -> super::FlagDropTile,
    powerball_tile_is_occupied: &mut impl FnMut(u16, u16) -> bool,
    powerball_tile_is_goal: &mut impl FnMut(u16, u16) -> bool,
) -> Result<Vec<Vec<u8>>, ObjectiveRegistryError> {
    let player_id = player.runtime.player_id();
    let reason = match reason {
        ObjectiveLifecycleReason::Death { .. } => ObjectiveLifecycleReason::Death {
            killer: killer_id.and_then(|killer_id| objective_player(state, killer_id)),
        },
        other => other,
    };
    let participants = objective_participants(state);
    let mut staged = state.objectives.clone();
    let lifecycle = apply_player_objective_lifecycle(
        &mut staged,
        player,
        reason,
        tick,
        &participants,
        flag_drop_tile,
        powerball_tile_is_occupied,
        powerball_tile_is_goal,
    );
    match lifecycle.and_then(|mut mutation| {
        let win = evaluate_flag_win(&mut staged, &participants, tick)?;
        merge_flag_win(&mut mutation, win);
        Ok(mutation)
    }) {
        Ok(mutation) => {
            state.objectives = staged;
            sync_player_flag_counts(state);
            Ok(mutation_packets(state, &mutation, tick, 0))
        }
        Err(error) => {
            tracing::warn!(
                player_id,
                ?error,
                "objective lifecycle entered generation-scoped quarantine recovery"
            );
            state
                .objectives
                .recover_hard_player_lifecycle(player.runtime, tick)?;
            sync_player_flag_counts(state);
            Ok(objective_state_replay_packets(state, tick))
        }
    }
}

fn merge_flag_win(mutation: &mut ObjectiveMutation, win: ObjectiveMutation) {
    mutation.flags.extend(win.flags);
    mutation.awards.extend(win.awards);
    if win.winning_frequency.is_some() {
        mutation.winning_frequency = win.winning_frequency;
    }
    if win.flag_victory.is_some() {
        mutation.flag_victory = win.flag_victory;
    }
}

fn blocking_brick_tiles_for_frequency(
    state: &ServerState,
    frequency: u16,
) -> std::collections::BTreeSet<(u16, u16)> {
    state
        .active_bricks
        .iter()
        .filter(|brick| {
            brick
                .packet
                .get(9..11)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u16::from_le_bytes)
                != Some(frequency)
        })
        .flat_map(|brick| brick.tiles.iter().copied())
        .collect()
}

pub(crate) fn sync_player_flag_counts(state: &mut ServerState) {
    let counts = state
        .clients
        .keys()
        .copied()
        .chain(state.bots.iter().map(|bot| bot.player.id))
        .map(|player_id| {
            let count = state
                .active_actor_runtime_identity(player_id)
                .map_or(0, |runtime| state.objectives.flag_carrier_count(runtime));
            (player_id, count)
        })
        .collect::<Vec<_>>();
    for (player_id, count) in counts {
        if let Some(client) = state.clients.get_mut(&player_id) {
            client.player.flags = count;
        } else if let Some(bot) = state.bot_mut(player_id) {
            bot.player.flags = count;
        }
    }
}

pub(crate) fn mutation_packets(
    state: &mut ServerState,
    mutation: &ObjectiveMutation,
    tick: u32,
    action_id: u32,
) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut awards = std::collections::BTreeMap::<u16, i64>::new();
    for award in &mutation.awards {
        let total = awards.entry(award.player_id).or_default();
        *total = total.saturating_add(award.points);
    }
    for (player_id, points) in awards {
        if let Some(client) = state.clients.get_mut(&player_id) {
            let adjusted = i128::from(client.player.flag_points) + i128::from(points);
            client.player.flag_points = adjusted.clamp(0, i128::from(u32::MAX)) as u32;
            packets.push(crate::roster::score_update(&client.player));
            crate::identity::persist_stats_for_client(state, player_id);
        } else if let Some(bot) = state.bot_mut(player_id) {
            let adjusted = i128::from(bot.player.flag_points) + i128::from(points);
            bot.player.flag_points = adjusted.clamp(0, i128::from(u32::MAX)) as u32;
            packets.push(crate::roster::score_update(&bot.player));
        }
    }
    for flag in &mutation.flags {
        packets.push(flag_state_packet(
            *flag,
            tick,
            action_id,
            ObjectiveStateUpdate::OUTCOME_APPLIED,
        ));
    }
    for ball in &mutation.powerballs {
        let lineage = match ball.state {
            PowerballState::Carried { carrier } => state
                .clients
                .get(&carrier.player_id())
                .map(|client| client.player.frequency)
                .or_else(|| {
                    state
                        .bot(carrier.player_id())
                        .map(|bot| bot.player.frequency)
                })
                .map(|frequency| (carrier.player_id(), frequency)),
            PowerballState::OnMap { .. } => state
                .objectives
                .powerball_motion(ball.identity.id)
                .and_then(|motion| {
                    motion
                        .last_touch
                        .zip(motion.last_touch_frequency)
                        .map(|(runtime, frequency)| (runtime.player_id(), frequency))
                }),
            PowerballState::Waiting { .. } => None,
        };
        packets.push(powerball_state_packet(
            *ball,
            lineage,
            state
                .objectives
                .powerball_killer_pickup(ball.identity.id)
                .map(|(killer, deadline)| (killer.player_id(), deadline)),
            tick,
            action_id,
            ObjectiveStateUpdate::OUTCOME_APPLIED,
        ));
    }
    for &(frequency, score) in &mutation.score_events {
        packets.push(soccer_goal_packet(frequency, score));
    }
    if let Some(version) = mutation.powerball_score_version {
        let generation = state.objectives.replay().generation;
        for &(frequency, score) in &mutation.powerball_scores {
            packets.push(objective_score_packet(
                generation,
                version,
                frequency,
                score,
                tick,
                crate::objective_protocol::ObjectiveScoreUpdate::OUTCOME_APPLIED,
            ));
        }
    }
    if let Some((frequency, points)) = mutation.flag_victory {
        packets.push(flag_victory_packet(frequency, points));
    }
    packets
}

pub(crate) fn objective_state_replay_packets(state: &ServerState, tick: u32) -> Vec<Vec<u8>> {
    let replay = state.objectives.replay();
    let generation = replay.generation;
    let mut packets = replay
        .flags
        .into_iter()
        .map(|flag| flag_state_packet(flag, tick, 0, ObjectiveStateUpdate::OUTCOME_REPLAY))
        .collect::<Vec<_>>();
    packets.extend(replay.powerballs.into_iter().map(|ball| {
        let ball = powerball_snapshot_with_current_motion(state, ball);
        let lineage = match ball.state {
            PowerballState::Carried { carrier } => state
                .clients
                .get(&carrier.player_id())
                .map(|client| client.player.frequency)
                .or_else(|| {
                    state
                        .bot(carrier.player_id())
                        .map(|bot| bot.player.frequency)
                })
                .map(|frequency| (carrier.player_id(), frequency)),
            PowerballState::OnMap { .. } => state
                .objectives
                .powerball_motion(ball.identity.id)
                .and_then(|motion| {
                    motion
                        .last_touch
                        .zip(motion.last_touch_frequency)
                        .map(|(runtime, frequency)| (runtime.player_id(), frequency))
                }),
            PowerballState::Waiting { .. } => None,
        };
        let killer_pickup = state
            .objectives
            .powerball_killer_pickup(ball.identity.id)
            .map(|(killer, deadline)| (killer.player_id(), deadline));
        powerball_state_packet(
            ball,
            lineage,
            killer_pickup,
            tick,
            0,
            ObjectiveStateUpdate::OUTCOME_REPLAY,
        )
    }));
    if state.objectives.profile().powerballs.count() > 0 {
        let version = state.objectives.powerball_score_version();
        packets.extend(state.objectives.powerball_score_snapshot().into_iter().map(
            |(frequency, score)| {
                objective_score_packet(
                    generation,
                    version,
                    frequency,
                    score,
                    tick,
                    crate::objective_protocol::ObjectiveScoreUpdate::OUTCOME_REPLAY,
                )
            },
        ));
    }
    packets
}

fn current_state_packet(
    state: &ServerState,
    request: ObjectiveActionRequest,
    tick: u32,
) -> Option<Vec<u8>> {
    current_state_packet_for_registry(&state.objectives, request, tick)
}

fn current_state_packet_for_registry(
    registry: &ObjectiveRegistry,
    request: ObjectiveActionRequest,
    tick: u32,
) -> Option<Vec<u8>> {
    match request.kind {
        ObjectiveActionKind::FlagClaim | ObjectiveActionKind::FlagDrop => {
            registry.flag(request.objective_id).map(|flag| {
                flag_state_packet(
                    flag,
                    tick,
                    request.action_id,
                    ObjectiveStateUpdate::OUTCOME_REJECTED,
                )
            })
        }
        ObjectiveActionKind::BallPickup
        | ObjectiveActionKind::BallFire
        | ObjectiveActionKind::BallGoal => {
            registry.powerball(request.objective_id as u8).map(|ball| {
                let ball = powerball_snapshot_with_current_motion_for_registry(registry, ball);
                let lineage = registry
                    .powerball_motion(ball.identity.id)
                    .and_then(|motion| {
                        motion
                            .last_touch
                            .zip(motion.last_touch_frequency)
                            .map(|(runtime, frequency)| (runtime.player_id(), frequency))
                    });
                powerball_state_packet(
                    ball,
                    lineage,
                    registry
                        .powerball_killer_pickup(ball.identity.id)
                        .map(|(killer, deadline)| (killer.player_id(), deadline)),
                    tick,
                    request.action_id,
                    ObjectiveStateUpdate::OUTCOME_REJECTED,
                )
            })
        }
    }
}

fn powerball_snapshot_with_current_motion(
    state: &ServerState,
    ball: PowerballSnapshot,
) -> PowerballSnapshot {
    powerball_snapshot_with_current_motion_for_registry(&state.objectives, ball)
}

fn powerball_snapshot_with_current_motion_for_registry(
    registry: &ObjectiveRegistry,
    mut ball: PowerballSnapshot,
) -> PowerballSnapshot {
    if let PowerballState::OnMap {
        pickup_unlock_tick, ..
    } = ball.state
        && let Some(motion) = registry.powerball_motion(ball.identity.id)
    {
        let (x, y) = motion.wire_position();
        ball.state = PowerballState::OnMap {
            x,
            y,
            velocity_x: motion.velocity_x,
            velocity_y: motion.velocity_y,
            pickup_unlock_tick,
        };
    }
    ball
}

#[cfg(test)]
mod bot_tests {
    use super::*;

    fn state_with_bot(flag_count: u16, powerball_count: u8) -> ServerState {
        let mut state = ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        };
        state.initialize_configured_bots();
        state
            .objectives
            .reset(crate::objectives::synthetic_profile(
                flag_count,
                powerball_count,
            ))
            .expect("synthetic profile resets registry");
        state
    }

    #[test]
    fn server_bot_claims_flag_through_shared_objective_registry() {
        let mut state = state_with_bot(2, 0);
        state.bots[0].player.frequency = 1;
        let bot = &state.bots[0];
        let runtime = bot.runtime_identity();
        let (x, y) = (bot.x.round() as u16, bot.y.round() as u16);
        state
            .objectives
            .create_flag(super::super::FlagState::OnMap { x, y, owner: None })
            .expect("flag fits synthetic profile");
        state
            .objectives
            .create_flag(super::super::FlagState::OnMap {
                x: x.saturating_add(1_000),
                y,
                owner: None,
            })
            .expect("second flag prevents an immediate one-flag win");

        let packets = step_server_bot_objectives(&mut state, 20);

        assert!(!packets.is_empty());
        assert!(matches!(
            state.objectives.flag(0).map(|flag| flag.state),
            Some(super::super::FlagState::Carried { carrier, .. }) if carrier == runtime
        ));
        assert_eq!(state.bots[0].player.flags, 1);
    }

    #[test]
    fn server_bot_picks_up_and_fires_powerball_through_shared_registry() {
        let mut state = state_with_bot(0, 1);
        state.bots[0].player.frequency = 1;
        let bot = &state.bots[0];
        let runtime = bot.runtime_identity();
        let (x, y) = (bot.x.round() as u16, bot.y.round() as u16);
        state
            .objectives
            .create_powerball(PowerballState::OnMap {
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .expect("powerball fits synthetic profile");

        assert!(!step_server_bot_objectives(&mut state, 30).is_empty());
        assert!(matches!(
            state.objectives.powerball(0).map(|ball| ball.state),
            Some(PowerballState::Carried { carrier }) if carrier == runtime
        ));
        assert!(!step_server_bot_objectives(&mut state, 31).is_empty());
        assert!(matches!(
            state.objectives.powerball(0).map(|ball| ball.state),
            Some(PowerballState::OnMap { .. })
        ));
        assert_eq!(
            state
                .objectives
                .powerball_motion(0)
                .and_then(|motion| motion.last_touch),
            Some(runtime)
        );
    }

    #[test]
    fn server_bot_flag_win_updates_shared_score_row() {
        let mut policy = crate::objectives::synthetic_flag_policy();
        policy.win_delay_ticks = 0;
        let profile = crate::objectives::ServerObjectiveProfile {
            flags: crate::objectives::ServerFlagProfile::validate(
                1,
                Some(crate::objectives::FlagProfileSettings {
                    spawn: crate::objectives::ObjectiveSpawnArea {
                        x: 1,
                        y: 1,
                        radius: 0,
                    },
                    carry_timeout_ticks: 100,
                    reset_delay_ticks: 10,
                    reward: 100_000,
                    team_limits: crate::objectives::ObjectiveTeamLimits::default(),
                    policy,
                }),
            )
            .expect("synthetic scoring profile"),
            powerballs: crate::objectives::ServerPowerballProfile::Disabled,
        };
        let mut state = ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        };
        state.initialize_configured_bots();
        state.bots[0].player.frequency = 1;
        let open = state
            .zone_map()
            .tiles
            .iter()
            .position(|tile| *tile == 0)
            .expect("checked-in map has open space");
        state.bots[0].x = ((open % crate::zone::MAP_TILES) * crate::zone::TILE_SIZE_PX as usize
            + crate::zone::TILE_SIZE_PX as usize / 2) as f32;
        state.bots[0].y = ((open / crate::zone::MAP_TILES) * crate::zone::TILE_SIZE_PX as usize
            + crate::zone::TILE_SIZE_PX as usize / 2) as f32;
        state
            .objectives
            .reset(profile)
            .expect("scoring profile resets registry");
        let bot = &state.bots[0];
        let before = bot.player.flag_points;
        let (x, y) = (bot.x.round() as u16, bot.y.round() as u16);
        state
            .objectives
            .create_flag(super::super::FlagState::OnMap { x, y, owner: None })
            .expect("flag fits profile");

        let packets = step_server_bot_objectives(&mut state, 40);

        assert!(!packets.is_empty());
        assert_eq!(state.bots[0].player.flag_points, before + 100);
        assert_eq!(state.objectives.flag_score(1), 100);
        assert!(matches!(
            state.objectives.flag(0).map(|flag| flag.state),
            Some(super::super::FlagState::Waiting { .. })
        ));
    }
}
