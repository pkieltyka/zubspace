use super::profile::{
    FlagDropBehavior, FlagRewardSplit, FlagWinMode, ObjectiveSpawnArea, ObjectiveTeamLimits,
    ServerFlagProfile, ServerPowerballProfile,
};
use super::registry::{
    FlagIdentity, FlagSnapshot, FlagState, ObjectiveRegistry, ObjectiveRegistryError,
    ObjectiveVersion, PowerballIdentity, PowerballMotion, PowerballSnapshot, PowerballState,
};
use crate::authority_identity::PlayerRuntimeIdentity;
use std::collections::BTreeSet;

const SPAWN_ATTEMPTS: u32 = 30;
const MAX_BALL_CATCHUP_TICKS: u32 = 6_000;
const MAP_TILE_MASK: i32 = 1023;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FlagDropTile {
    empty: bool,
    fly_through: bool,
    available: bool,
}

impl FlagDropTile {
    pub(crate) const fn from_map_tile(tile: u8, no_flag_drops: bool) -> Self {
        let fly_through = tile == 0
            || (tile >= 162 && tile <= 190)
            || tile == 170
            || tile == 171
            || tile == 172
            || tile == 250;
        Self {
            empty: tile == 0,
            fly_through,
            available: tile == 0 && !no_flag_drops,
        }
    }

    #[cfg(test)]
    pub(crate) const fn available() -> Self {
        Self {
            empty: true,
            fly_through: true,
            available: true,
        }
    }

    #[cfg(test)]
    pub(crate) const fn with_semantics(fly_through: bool, available: bool) -> Self {
        Self {
            empty: available,
            fly_through,
            available,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectiveStep {
    pub(crate) flags: Vec<FlagSnapshot>,
    pub(crate) powerballs: Vec<PowerballSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ObjectivePlayer {
    pub(crate) runtime: PlayerRuntimeIdentity,
    pub(crate) arena_stream_id: u64,
    pub(crate) frequency: u16,
    pub(crate) ship: u8,
    pub(crate) alive: bool,
    pub(crate) safe: bool,
    pub(crate) attached: bool,
    pub(crate) position_x: u16,
    pub(crate) position_y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) direction: u8,
    /// Full-precision canonical simulation geometry. Wire fields above remain
    /// authority anchors for proximity; releases quantize these exact values
    /// once, when constructing the authoritative ball state.
    pub(crate) exact_position_x: f32,
    pub(crate) exact_position_y: f32,
    pub(crate) exact_velocity_x: f32,
    pub(crate) exact_velocity_y: f32,
    pub(crate) exact_orientation: f32,
    pub(crate) position_sequence: u16,
}

impl ObjectivePlayer {
    fn active(self) -> bool {
        self.alive && self.ship < 8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveParticipant {
    pub(crate) player_id: u16,
    pub(crate) frequency: u16,
    pub(crate) safe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveAward {
    pub(crate) player_id: u16,
    pub(crate) points: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectiveMutation {
    pub(crate) flags: Vec<FlagSnapshot>,
    pub(crate) powerballs: Vec<PowerballSnapshot>,
    pub(crate) awards: Vec<ObjectiveAward>,
    pub(crate) winning_frequency: Option<u16>,
    pub(crate) flag_victory: Option<(u16, i64)>,
    pub(crate) score_events: Vec<(u16, i64)>,
    pub(crate) powerball_scores: Vec<(u16, u32)>,
    pub(crate) powerball_score_version: Option<ObjectiveVersion>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ObjectiveLifecycleReason {
    Requested,
    Death { killer: Option<ObjectivePlayer> },
    ShipOrFrequency,
    Disconnect,
    SessionOrArena,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectiveMutationError {
    Disabled,
    InactivePlayer,
    StalePlayer,
    StalePosition,
    DuplicateAction,
    UnknownObjective,
    StaleObjective,
    InvalidState,
    OwnedByTeam,
    CarryLimit,
    OutOfRange,
    BlockedLineOfSight,
    PickupLocked,
    UnsupportedFrequency,
    Registry(ObjectiveRegistryError),
}

impl From<ObjectiveRegistryError> for ObjectiveMutationError {
    fn from(value: ObjectiveRegistryError) -> Self {
        Self::Registry(value)
    }
}

pub(crate) fn authenticate_objective_action(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    request: crate::objective_protocol::ObjectiveActionRequest,
) -> Result<(), ObjectiveMutationError> {
    if request.player_id != player.runtime.player_id()
        || request.server_session_id != crate::wire::server_session_id()
        || request.life_seq != player.runtime.life.life_seq
        || request.arena_stream_id != player.arena_stream_id
    {
        return Err(ObjectiveMutationError::StalePlayer);
    }
    if request.position_sequence != player.position_sequence {
        return Err(ObjectiveMutationError::StalePosition);
    }
    if !registry.consume_action(player.runtime, request.action_id) {
        return Err(ObjectiveMutationError::DuplicateAction);
    }
    Ok(())
}

pub(crate) fn claim_flag(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    identity: FlagIdentity,
    expected_version: ObjectiveVersion,
    tick: u32,
) -> Result<FlagSnapshot, ObjectiveMutationError> {
    let ServerFlagProfile::Enabled(profile) = registry.profile().flags.clone() else {
        return Err(ObjectiveMutationError::Disabled);
    };
    if !player.active() {
        return Err(ObjectiveMutationError::InactivePlayer);
    }
    let flag = registry
        .flag(identity.id)
        .filter(|flag| flag.identity == identity)
        .ok_or(ObjectiveMutationError::UnknownObjective)?;
    if flag.version != expected_version {
        return Err(ObjectiveMutationError::StaleObjective);
    }
    let FlagState::OnMap { x, y, owner } = flag.state else {
        return Err(ObjectiveMutationError::InvalidState);
    };
    if owner == Some(player.frequency) {
        return Err(ObjectiveMutationError::OwnedByTeam);
    }
    if registry.flag_carrier_count(player.runtime) >= profile.policy.carry_limit {
        return Err(ObjectiveMutationError::CarryLimit);
    }
    let collision_radius = profile.policy.collision_radius_pixels[usize::from(player.ship)];
    if !flag_box_overlaps_ship(x, y, player.position_x, player.position_y, collision_radius) {
        return Err(ObjectiveMutationError::OutOfRange);
    }
    let claimed = registry.transition_flag(
        identity,
        expected_version,
        FlagState::Carried {
            carrier: player.runtime,
            x,
            y,
            owner: Some(player.frequency),
        },
    )?;
    registry.set_flag_carried_since(identity, tick);
    Ok(claimed)
}

pub(crate) fn pickup_powerball(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    identity: PowerballIdentity,
    expected_version: ObjectiveVersion,
    tick: u32,
    line_of_sight: bool,
) -> Result<PowerballSnapshot, ObjectiveMutationError> {
    let ServerPowerballProfile::Enabled(profile) = registry.profile().powerballs.clone() else {
        return Err(ObjectiveMutationError::Disabled);
    };
    if usize::from(player.frequency) >= super::registry::POWERBALL_SCORE_BUCKETS {
        return Err(ObjectiveMutationError::UnsupportedFrequency);
    }
    if !player.active() || player.attached {
        return Err(ObjectiveMutationError::InactivePlayer);
    }
    if !registry.carried_powerball_ids(player.runtime).is_empty() {
        return Err(ObjectiveMutationError::CarryLimit);
    }
    let ball = registry
        .powerball(identity.id)
        .filter(|ball| ball.identity == identity)
        .ok_or(ObjectiveMutationError::UnknownObjective)?;
    if ball.version != expected_version {
        return Err(ObjectiveMutationError::StaleObjective);
    }
    let PowerballState::OnMap {
        x: state_x,
        y: state_y,
        pickup_unlock_tick,
        ..
    } = ball.state
    else {
        return Err(ObjectiveMutationError::InvalidState);
    };
    let pickup_deadline = registry
        .powerball_killer_pickup(identity.id)
        .filter(|(killer, _)| *killer == player.runtime)
        .map(|(_, deadline)| deadline)
        .or(pickup_unlock_tick);
    if pickup_deadline
        .is_some_and(|deadline| !crate::continuum::continuum_tick_reached(tick, deadline))
    {
        return Err(ObjectiveMutationError::PickupLocked);
    }
    let motion = registry.powerball_motion(identity.id);
    if motion.is_some_and(|motion| {
        motion.last_touch == Some(player.runtime)
            && (motion.velocity_x != 0 || motion.velocity_y != 0)
    }) {
        return Err(ObjectiveMutationError::PickupLocked);
    }
    let in_range = motion.map_or_else(
        || {
            strict_square_overlap(
                player.position_x,
                player.position_y,
                state_x,
                state_y,
                profile.policy.pickup_radius_pixels[usize::from(player.ship)],
            )
        },
        |motion| {
            let player_x = u32::from(player.position_x) * PowerballMotion::POSITION_SCALE;
            let player_y = u32::from(player.position_y) * PowerballMotion::POSITION_SCALE;
            let radius = u32::from(profile.policy.pickup_radius_pixels[usize::from(player.ship)])
                * PowerballMotion::POSITION_SCALE;
            player_x.abs_diff(motion.position_x) < radius
                && player_y.abs_diff(motion.position_y) < radius
        },
    );
    if !in_range {
        return Err(ObjectiveMutationError::OutOfRange);
    }
    if profile.policy.require_line_of_sight && !line_of_sight {
        return Err(ObjectiveMutationError::BlockedLineOfSight);
    }
    let claimed = registry.transition_powerball(
        identity,
        expected_version,
        PowerballState::Carried {
            carrier: player.runtime,
        },
    )?;
    registry.set_powerball_carried_since(identity, tick);
    Ok(claimed)
}

pub(crate) fn fire_powerball(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    identity: PowerballIdentity,
    expected_version: ObjectiveVersion,
    tick: u32,
    timed_release: bool,
) -> Result<PowerballSnapshot, ObjectiveMutationError> {
    let ServerPowerballProfile::Enabled(profile) = registry.profile().powerballs.clone() else {
        return Err(ObjectiveMutationError::Disabled);
    };
    if !player.active() {
        return Err(ObjectiveMutationError::InactivePlayer);
    }
    let ball = registry
        .powerball(identity.id)
        .filter(|ball| ball.identity == identity)
        .ok_or(ObjectiveMutationError::UnknownObjective)?;
    if ball.version != expected_version {
        return Err(ObjectiveMutationError::StaleObjective);
    }
    if !matches!(ball.state, PowerballState::Carried { carrier } if carrier == player.runtime) {
        return Err(ObjectiveMutationError::InvalidState);
    }
    let orientation = player.exact_orientation;
    let (heading_x, heading_y) =
        crate::continuum_position::canonical_discrete_heading_components(orientation);
    let sign = if timed_release { -1.0 } else { 1.0 };
    let ship = usize::from(player.ship);
    let velocity_x = ((player.exact_velocity_x
        + sign * heading_x * (f32::from(profile.policy.throw_speed[ship]) / 10.0))
        * 10.0)
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    let velocity_y = ((player.exact_velocity_y
        + sign * heading_y * (f32::from(profile.policy.throw_speed[ship]) / 10.0))
        * 10.0)
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    let (x, y) = if timed_release {
        let (x, y) = crate::continuum_position::canonical_carried_powerball_position(
            player.exact_position_x,
            player.exact_position_y,
            orientation,
            crate::arena_settings::arena_ship_radius_pixels(player.ship),
        );
        (
            x.clamp(0.0, f32::from(u16::MAX)) as u16,
            y.clamp(0.0, f32::from(u16::MAX)) as u16,
        )
    } else {
        (
            player.exact_position_x.clamp(0.0, f32::from(u16::MAX)) as u16,
            player.exact_position_y.clamp(0.0, f32::from(u16::MAX)) as u16,
        )
    };
    let released = registry.transition_powerball(
        identity,
        expected_version,
        PowerballState::OnMap {
            x,
            y,
            velocity_x,
            velocity_y,
            pickup_unlock_tick: Some(crate::continuum::continuum_tick_add(
                tick,
                profile.policy.pass_delay_ticks,
            )),
        },
    )?;
    let mut motion = PowerballMotion::from_wire_position(
        x,
        y,
        velocity_x,
        velocity_y,
        profile.policy.friction_delta[ship],
        tick,
    );
    motion.last_touch = Some(player.runtime);
    motion.last_touch_frequency = Some(player.frequency);
    registry.set_powerball_motion(identity, motion)?;
    registry.set_powerball_killer_pickup(identity, None)?;
    Ok(released)
}

pub(crate) fn apply_player_objective_lifecycle(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    reason: ObjectiveLifecycleReason,
    tick: u32,
    participants: &[ObjectiveParticipant],
    flag_drop_tile: impl FnMut(u16, u16) -> FlagDropTile,
    powerball_tile_is_occupied: impl FnMut(u16, u16) -> bool,
    powerball_tile_is_goal: impl FnMut(u16, u16) -> bool,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let include_powerballs = !matches!(reason, ObjectiveLifecycleReason::Requested);
    apply_player_objective_lifecycle_scoped(
        registry,
        player,
        reason,
        tick,
        participants,
        flag_drop_tile,
        powerball_tile_is_occupied,
        powerball_tile_is_goal,
        include_powerballs,
    )
}

fn apply_player_objective_lifecycle_scoped(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    reason: ObjectiveLifecycleReason,
    tick: u32,
    participants: &[ObjectiveParticipant],
    mut flag_drop_tile: impl FnMut(u16, u16) -> FlagDropTile,
    mut powerball_tile_is_occupied: impl FnMut(u16, u16) -> bool,
    mut powerball_tile_is_goal: impl FnMut(u16, u16) -> bool,
    include_powerballs: bool,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let mut staged = registry.clone();
    let mut mutation = ObjectiveMutation::default();
    if let ServerFlagProfile::Enabled(profile) = staged.profile().flags.clone() {
        let mut reserved = staged
            .replay()
            .flags
            .into_iter()
            .filter_map(|flag| match flag.state {
                FlagState::OnMap { x, y, .. } => Some((x / 16, y / 16)),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let default_behavior = match reason {
            ObjectiveLifecycleReason::Requested if player.safe => profile.policy.safe_drop,
            ObjectiveLifecycleReason::Requested => profile.policy.requested_drop,
            ObjectiveLifecycleReason::Death { .. } => profile.policy.death_drop,
            ObjectiveLifecycleReason::ShipOrFrequency => profile.policy.ship_or_frequency_drop,
            ObjectiveLifecycleReason::Disconnect | ObjectiveLifecycleReason::SessionOrArena => {
                profile.policy.disconnect_drop
            }
        };
        let transfer_candidate = match reason {
            ObjectiveLifecycleReason::Death {
                killer: Some(killer),
            } if killer.active()
                && killer.runtime != player.runtime
                && ((killer.frequency == player.frequency
                    && profile.policy.transfer_on_friendly_kill)
                    || (killer.frequency != player.frequency
                        && profile.policy.transfer_on_enemy_kill)) =>
            {
                Some(killer)
            }
            _ => None,
        };
        let drop_behavior = match (reason, transfer_candidate) {
            (ObjectiveLifecycleReason::Death { .. }, Some(_)) => {
                profile.policy.transfer_overflow_drop
            }
            (
                ObjectiveLifecycleReason::Death {
                    killer: Some(killer),
                },
                None,
            ) if killer.frequency == player.frequency => profile.policy.friendly_kill_drop,
            _ => default_behavior,
        };
        let mut dropped_flags = Vec::new();
        for flag in staged.replay().flags {
            if !matches!(flag.state, FlagState::Carried { carrier, .. } if carrier == player.runtime)
            {
                continue;
            }
            if let Some(killer) = transfer_candidate.filter(|killer| {
                staged.flag_carrier_count(killer.runtime) < profile.policy.carry_limit
            }) {
                let transferred = staged.transition_flag(
                    flag.identity,
                    flag.version,
                    FlagState::Carried {
                        carrier: killer.runtime,
                        x: killer.position_x,
                        y: killer.position_y,
                        owner: Some(killer.frequency),
                    },
                )?;
                staged.set_flag_carried_since(flag.identity, tick);
                mutation.flags.push(transferred);
            } else {
                dropped_flags.push(flag);
            }
        }
        let dropped_states = dropped_flag_states(
            staged.rng_seed_mut(),
            drop_behavior,
            player,
            profile.spawn,
            profile.policy.drop_radius_tiles,
            dropped_flags.len(),
            &mut reserved,
            &mut flag_drop_tile,
        );
        for (flag, state) in dropped_flags.into_iter().zip(dropped_states) {
            let state = match state {
                Some(state) => state,
                None if matches!(reason, ObjectiveLifecycleReason::Requested) => {
                    return Err(ObjectiveRegistryError::SpawnUnavailable.into());
                }
                None => FlagState::Waiting {
                    // Hard lifecycle boundaries must retire the carrier
                    // even when neither local nor center placement is
                    // currently available. The deadline step retries the
                    // configured spawn on the next authoritative tick.
                    respawn_at_tick: Some(tick),
                },
            };
            tracing::info!(
                flag = flag.identity.id,
                player = ?player.runtime,
                frequency = player.frequency,
                reason = ?reason,
                behavior = ?drop_behavior,
                state = ?state,
                "objective flag released"
            );
            mutation
                .flags
                .push(staged.transition_flag(flag.identity, flag.version, state)?);
        }
    }
    if include_powerballs
        && let ServerPowerballProfile::Enabled(profile) = staged.profile().powerballs.clone()
    {
        let mut metadata_cleared = staged
            .clear_powerball_killer_pickups_for_player(player.runtime)
            .into_iter()
            .collect::<BTreeSet<_>>();
        if !matches!(reason, ObjectiveLifecycleReason::Requested) {
            for ball in staged.replay().powerballs {
                if !matches!(ball.state, PowerballState::OnMap { .. }) {
                    continue;
                }
                let Some(mut motion) = staged.powerball_motion(ball.identity.id) else {
                    continue;
                };
                if motion.last_touch != Some(player.runtime) {
                    continue;
                }
                let preserve_death_goal_lineage =
                    matches!(reason, ObjectiveLifecycleReason::Death { .. })
                        && profile.policy.death_scores_goal;
                if !preserve_death_goal_lineage {
                    motion.last_touch = None;
                    motion.last_touch_frequency = None;
                    staged.set_powerball_motion(ball.identity, motion)?;
                    metadata_cleared.insert(ball.identity.id);
                }
            }
        }
        for id in metadata_cleared {
            let Some(mut ball) = staged.powerball(id) else {
                continue;
            };
            let PowerballState::OnMap {
                pickup_unlock_tick, ..
            } = ball.state
            else {
                continue;
            };
            if let Some(motion) = staged.powerball_motion(id) {
                let (x, y) = motion.wire_position();
                ball.state = PowerballState::OnMap {
                    x,
                    y,
                    velocity_x: motion.velocity_x,
                    velocity_y: motion.velocity_y,
                    pickup_unlock_tick,
                };
            }
            mutation.powerballs.push(ball);
        }
        for ball in staged.replay().powerballs {
            if !matches!(ball.state, PowerballState::Carried { carrier } if carrier == player.runtime)
            {
                continue;
            }
            let global_unlock_tick =
                crate::continuum::continuum_tick_add(tick, profile.policy.pass_delay_ticks);
            let released = staged.transition_powerball(
                ball.identity,
                ball.version,
                PowerballState::OnMap {
                    x: player.position_x,
                    y: player.position_y,
                    velocity_x: 0,
                    velocity_y: 0,
                    pickup_unlock_tick: Some(global_unlock_tick),
                },
            )?;
            let mut motion = PowerballMotion::from_wire_position(
                player.position_x,
                player.position_y,
                0,
                0,
                profile.policy.friction_delta[usize::from(player.ship)],
                tick,
            );
            if matches!(reason, ObjectiveLifecycleReason::Death { .. })
                && profile.policy.death_scores_goal
            {
                motion.last_touch = Some(player.runtime);
                motion.last_touch_frequency = Some(player.frequency);
            }
            staged.set_powerball_motion(ball.identity, motion)?;
            if let ObjectiveLifecycleReason::Death {
                killer: Some(killer),
            } = reason
                && killer.runtime != player.runtime
                && profile.policy.killer_ignore_pass_delay_ticks > 0
            {
                let killer_delay = profile
                    .policy
                    .pass_delay_ticks
                    .saturating_sub(profile.policy.killer_ignore_pass_delay_ticks);
                staged.set_powerball_killer_pickup(
                    ball.identity,
                    Some((
                        killer.runtime,
                        crate::continuum::continuum_tick_add(tick, killer_delay),
                    )),
                )?;
            }
            let scores_on_death = matches!(reason, ObjectiveLifecycleReason::Death { .. })
                && profile.policy.death_scores_goal
                && powerball_tile_is_goal(player.position_x, player.position_y)
                && super::soccer::soccer_goal_counts_world(
                    player.position_x,
                    player.position_y,
                    player.frequency,
                    profile.policy.soccer_mode,
                );
            if scores_on_death {
                let goal = score_powerball_goal(
                    &mut staged,
                    released.identity,
                    released.version,
                    player.frequency,
                    player.position_x,
                    player.position_y,
                    participants,
                    tick,
                    &mut powerball_tile_is_occupied,
                )?;
                merge_powerball_goal(&mut mutation, goal);
                if mutation.winning_frequency.is_some() {
                    break;
                }
            } else {
                mutation.powerballs.push(released);
            }
        }
    }
    if !matches!(reason, ObjectiveLifecycleReason::Requested) {
        staged.clear_player_actions(player.runtime);
    }
    *registry = staged;
    Ok(mutation)
}

pub(crate) fn evaluate_flag_win(
    registry: &mut ObjectiveRegistry,
    participants: &[ObjectiveParticipant],
    tick: u32,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let ServerFlagProfile::Enabled(profile) = registry.profile().flags.clone() else {
        return Ok(ObjectiveMutation::default());
    };
    let replay = registry.replay();
    let candidate = replay.flags.first().and_then(|first| match first.state {
        FlagState::Carried { owner, .. } | FlagState::OnMap { owner, .. } => owner,
        FlagState::Waiting { .. } => None,
    });
    let wins = candidate.is_some_and(|frequency| {
        replay
            .flags
            .iter()
            .all(|flag| match (profile.policy.win_mode, flag.state) {
                (FlagWinMode::CarryAll, FlagState::Carried { owner, .. }) => {
                    owner == Some(frequency)
                }
                (FlagWinMode::OwnAllDropped, FlagState::OnMap { owner, .. }) => {
                    owner == Some(frequency)
                }
                _ => false,
            })
    });
    let Some(frequency) = candidate.filter(|_| wins) else {
        registry.set_pending_flag_win(None);
        return Ok(ObjectiveMutation::default());
    };
    let deadline = registry
        .pending_flag_win()
        .and_then(|(pending, deadline)| (pending == frequency).then_some(deadline));
    let deadline = deadline.unwrap_or_else(|| {
        crate::continuum::continuum_tick_add(tick, profile.policy.win_delay_ticks)
    });
    if profile.policy.win_delay_ticks > 0
        && !crate::continuum::continuum_tick_reached(tick, deadline)
    {
        registry.set_pending_flag_win(Some((frequency, deadline)));
        return Ok(ObjectiveMutation::default());
    }
    let eligible = participants
        .iter()
        .copied()
        .filter(|participant| participant.frequency == frequency)
        .collect::<Vec<_>>();
    let reward = if team_requirements_met(participants, frequency, profile.team_limits) {
        configured_flag_reward(profile.reward, participants.len())
    } else {
        0
    };
    let awards = split_flag_reward(reward, profile.policy.reward_split, &eligible);
    let mut staged = registry.clone();
    staged.set_pending_flag_win(None);
    staged.add_flag_score(frequency, reward);
    let mut mutation = ObjectiveMutation {
        awards,
        winning_frequency: Some(frequency),
        ..ObjectiveMutation::default()
    };
    mutation.flag_victory = Some((
        frequency,
        mutation.awards.first().map_or(0, |award| award.points),
    ));
    // The win reset is the only flag release that was invisible in the log: lifecycle drops log
    // their reason, this path logged nothing, so a `Carried -> Waiting` transition with no release
    // line could only be guessed at as a defect.
    tracing::debug!(
        frequency,
        tick,
        reward,
        participants = eligible.len(),
        reset_delay_ticks = profile.reset_delay_ticks,
        "objective flag win committed"
    );
    for flag in replay.flags {
        mutation.flags.push(staged.transition_flag(
            flag.identity,
            flag.version,
            FlagState::Waiting {
                respawn_at_tick: Some(crate::continuum::continuum_tick_add(
                    tick,
                    profile.reset_delay_ticks,
                )),
            },
        )?);
    }
    *registry = staged;
    Ok(mutation)
}

pub(crate) fn score_powerball_goal(
    registry: &mut ObjectiveRegistry,
    identity: PowerballIdentity,
    expected_version: ObjectiveVersion,
    scoring_frequency: u16,
    goal_x: u16,
    goal_y: u16,
    participants: &[ObjectiveParticipant],
    tick: u32,
    mut tile_is_occupied: impl FnMut(u16, u16) -> bool,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let ServerPowerballProfile::Enabled(profile) = registry.profile().powerballs.clone() else {
        return Err(ObjectiveMutationError::Disabled);
    };
    if usize::from(scoring_frequency) >= super::registry::POWERBALL_SCORE_BUCKETS {
        return Err(ObjectiveMutationError::UnsupportedFrequency);
    }
    let ball = registry
        .powerball(identity.id)
        .filter(|ball| ball.identity == identity)
        .ok_or(ObjectiveMutationError::UnknownObjective)?;
    if ball.version != expected_version {
        return Err(ObjectiveMutationError::StaleObjective);
    }
    if !matches!(ball.state, PowerballState::OnMap { .. }) {
        return Err(ObjectiveMutationError::InvalidState);
    }
    let (scoreable, owner_frequency) = super::soccer::soccer_goal_info_world(
        goal_x,
        goal_y,
        scoring_frequency,
        profile.policy.soccer_mode,
    );
    if !scoreable {
        return Err(ObjectiveMutationError::InvalidState);
    }
    let scoring_team = scoring_frequency;
    let mut staged = registry.clone();
    if profile.policy.capture_points >= 0 {
        if let Some(owner_frequency) = owner_frequency {
            let owner_score = staged.powerball_score(owner_frequency);
            if owner_score > 0 {
                staged.set_powerball_score(owner_frequency, owner_score - 1);
                staged.add_powerball_score(scoring_team, 1);
            }
        } else {
            staged.add_powerball_score(scoring_team, 1);
        }
    } else {
        staged.add_powerball_score(scoring_team, 1);
    }
    let winning_frequency = powerball_game_winner(&staged, &profile);
    let won = winning_frequency.is_some();
    let delay = if won {
        resolve_new_game_delay(staged.rng_seed_mut(), profile.policy.new_game_delay_ticks)
    } else {
        profile.goal_delay_ticks
    };
    let mut mutation = ObjectiveMutation {
        winning_frequency,
        ..ObjectiveMutation::default()
    };
    let goal_reward = if team_requirements_met(participants, scoring_frequency, profile.team_limits)
    {
        configured_reward(profile.reward, participants.len())
    } else {
        0
    };
    mutation.score_events.push((scoring_frequency, goal_reward));
    mutation.awards.extend(
        participants
            .iter()
            .filter(|participant| participant.frequency == scoring_frequency && !participant.safe)
            .map(|participant| ObjectiveAward {
                player_id: participant.player_id,
                points: goal_reward,
            }),
    );
    if let Some(winner) = winning_frequency {
        let game_reward = if team_requirements_met(participants, winner, profile.team_limits) {
            configured_reward(profile.reward, participants.len())
        } else {
            0
        };
        mutation.awards.extend(
            participants
                .iter()
                .filter(|participant| participant.frequency == winner && !participant.safe)
                .map(|participant| ObjectiveAward {
                    player_id: participant.player_id,
                    points: game_reward,
                }),
        );
    }
    let mut combined_awards = Vec::<ObjectiveAward>::new();
    for award in mutation.awards.drain(..) {
        if let Some(existing) = combined_awards
            .iter_mut()
            .find(|existing| existing.player_id == award.player_id)
        {
            existing.points = existing.points.saturating_add(award.points);
        } else {
            combined_awards.push(award);
        }
    }
    mutation.awards = combined_awards;
    if won {
        for ball in staged.replay().powerballs {
            mutation.powerballs.push(staged.transition_powerball(
                ball.identity,
                ball.version,
                PowerballState::Waiting {
                    respawn_at_tick: Some(crate::continuum::continuum_tick_add(tick, delay)),
                },
            )?);
        }
        staged.reset_powerball_scores();
    } else if delay == 0 {
        let area = profile.spawns[usize::from(identity.id) % profile.spawns.len()];
        let (x, y) = ball_spawn_wire_position(staged.rng_seed_mut(), area, &mut tile_is_occupied)
            .ok_or(ObjectiveRegistryError::SpawnUnavailable)?;
        let spawned = staged.transition_powerball(
            identity,
            expected_version,
            PowerballState::OnMap {
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            },
        )?;
        staged.set_powerball_motion(
            identity,
            PowerballMotion::from_wire_position(x, y, 0, 0, profile.policy.friction_delta[0], tick),
        )?;
        mutation.powerballs.push(spawned);
    } else {
        mutation.powerballs.push(staged.transition_powerball(
            identity,
            expected_version,
            PowerballState::Waiting {
                respawn_at_tick: Some(crate::continuum::continuum_tick_add(tick, delay)),
            },
        )?);
    }
    mutation.powerball_score_version = Some(staged.advance_powerball_score_version()?);
    mutation.powerball_scores = staged.powerball_score_snapshot().into_iter().collect();
    *registry = staged;
    Ok(mutation)
}

fn merge_powerball_goal(mutation: &mut ObjectiveMutation, goal: ObjectiveMutation) {
    mutation.powerballs.extend(goal.powerballs);
    mutation.awards.extend(goal.awards);
    mutation.score_events.extend(goal.score_events);
    mutation.powerball_scores = goal.powerball_scores;
    mutation.powerball_score_version = goal.powerball_score_version;
    mutation.winning_frequency = goal.winning_frequency;
}

fn resolve_new_game_delay(seed: &mut i32, configured: i32) -> u32 {
    if configured >= 0 {
        return configured as u32;
    }
    let magnitude = configured.unsigned_abs();
    let modulus = u64::from(magnitude) + 1;
    (u64::from(crate::vie::vie_rng_next(seed) as u32) % modulus) as u32
}

pub(crate) fn step_objective_deadlines(
    registry: &mut ObjectiveRegistry,
    players: &[ObjectivePlayer],
    tick: u32,
    mut tile_is_occupied: impl FnMut(u16, u16) -> bool,
    mut flag_drop_tile: impl FnMut(u16, u16) -> FlagDropTile,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    let mut staged = registry.clone();
    let mut mutation = ObjectiveMutation::default();
    if let ServerFlagProfile::Enabled(profile) = staged.profile().flags.clone() {
        let mut placed_tiles = staged
            .replay()
            .flags
            .into_iter()
            .filter_map(|flag| match flag.state {
                FlagState::OnMap { x, y, .. } => Some((x / 16, y / 16)),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for flag in staged.replay().flags {
            match flag.state {
                FlagState::Waiting {
                    respawn_at_tick: Some(deadline),
                } if crate::continuum::continuum_tick_reached(tick, deadline) => {
                    let (tile_x, tile_y) = {
                        let Some(found) = flag_spawn_tile(
                            staged.rng_seed_mut(),
                            profile.spawn,
                            &placed_tiles,
                            &mut flag_drop_tile,
                        ) else {
                            continue;
                        };
                        found
                    };
                    placed_tiles.insert((tile_x, tile_y));
                    mutation.flags.push(staged.transition_flag(
                        flag.identity,
                        flag.version,
                        FlagState::OnMap {
                            x: tile_x.saturating_mul(16).saturating_add(8),
                            y: tile_y.saturating_mul(16).saturating_add(8),
                            owner: None,
                        },
                    )?);
                }
                FlagState::Carried { carrier, .. } => {
                    if profile.carry_timeout_ticks == 0 {
                        continue;
                    }
                    let Some(started) = staged.flag_carried_since(flag.identity.id) else {
                        continue;
                    };
                    if crate::continuum::continuum_tick_diff(tick, started)
                        >= profile.carry_timeout_ticks as i32
                    {
                        let state = players
                            .iter()
                            .copied()
                            .find(|player| player.runtime == carrier)
                            .and_then(|player| {
                                dropped_flag_state(
                                    staged.rng_seed_mut(),
                                    profile.policy.timeout_drop,
                                    player,
                                    profile.spawn,
                                    profile.policy.drop_radius_tiles,
                                    &placed_tiles,
                                    &mut flag_drop_tile,
                                )
                            })
                            .unwrap_or(FlagState::Waiting {
                                respawn_at_tick: Some(tick),
                            });
                        if let FlagState::OnMap { x, y, .. } = state {
                            placed_tiles.insert((x / 16, y / 16));
                        }
                        mutation.flags.push(staged.transition_flag(
                            flag.identity,
                            flag.version,
                            state,
                        )?);
                    }
                }
                _ => {}
            }
        }
    }
    if let ServerPowerballProfile::Enabled(profile) = staged.profile().powerballs.clone() {
        for ball in staged.replay().powerballs {
            match ball.state {
                PowerballState::Waiting {
                    respawn_at_tick: Some(deadline),
                } if crate::continuum::continuum_tick_reached(tick, deadline) => {
                    let area = profile.spawns[usize::from(ball.identity.id) % profile.spawns.len()];
                    let Some((x, y)) = ball_spawn_wire_position(
                        staged.rng_seed_mut(),
                        area,
                        &mut tile_is_occupied,
                    ) else {
                        continue;
                    };
                    let spawned = staged.transition_powerball(
                        ball.identity,
                        ball.version,
                        PowerballState::OnMap {
                            x,
                            y,
                            velocity_x: 0,
                            velocity_y: 0,
                            pickup_unlock_tick: None,
                        },
                    )?;
                    staged.set_powerball_motion(
                        ball.identity,
                        PowerballMotion::from_wire_position(
                            x,
                            y,
                            0,
                            0,
                            profile.policy.friction_delta[0],
                            tick,
                        ),
                    )?;
                    mutation.powerballs.push(spawned);
                }
                PowerballState::Carried { carrier } => {
                    let Some(player) = players
                        .iter()
                        .copied()
                        .find(|player| player.runtime == carrier)
                    else {
                        continue;
                    };
                    let carry_timeout =
                        profile.policy.carry_timeout_ticks[usize::from(player.ship)];
                    if carry_timeout == 0 {
                        continue;
                    }
                    let Some(started) = staged.powerball_carried_since(ball.identity.id) else {
                        continue;
                    };
                    if crate::continuum::continuum_tick_diff(tick, started) < carry_timeout as i32 {
                        continue;
                    }
                    mutation.powerballs.push(fire_powerball(
                        &mut staged,
                        player,
                        ball.identity,
                        ball.version,
                        tick,
                        true,
                    )?);
                }
                _ => {}
            }
        }
    }
    *registry = staged;
    Ok(mutation)
}

fn dropped_flag_state(
    seed: &mut i32,
    behavior: FlagDropBehavior,
    player: ObjectivePlayer,
    spawn: ObjectiveSpawnArea,
    drop_radius_tiles: u16,
    reserved: &BTreeSet<(u16, u16)>,
    flag_drop_tile: &mut impl FnMut(u16, u16) -> FlagDropTile,
) -> Option<FlagState> {
    let owned = matches!(
        behavior,
        FlagDropBehavior::OwnedAtCarrier | FlagDropBehavior::OwnedAtCenter
    );
    let centered = matches!(
        behavior,
        FlagDropBehavior::NeutralAtCenter | FlagDropBehavior::OwnedAtCenter
    );
    let (tile_x, tile_y) = if centered {
        flag_spawn_tile(seed, spawn, reserved, flag_drop_tile)?
    } else {
        flag_drop_tile_near(
            seed,
            player.position_x / 16,
            player.position_y / 16,
            drop_radius_tiles,
            reserved,
            flag_drop_tile,
        )
        .or_else(|| flag_spawn_tile(seed, spawn, reserved, flag_drop_tile))?
    };
    Some(FlagState::OnMap {
        x: tile_x.saturating_mul(16).saturating_add(8),
        y: tile_y.saturating_mul(16).saturating_add(8),
        owner: owned.then_some(player.frequency),
    })
}

fn dropped_flag_states(
    seed: &mut i32,
    behavior: FlagDropBehavior,
    player: ObjectivePlayer,
    spawn: ObjectiveSpawnArea,
    drop_radius_tiles: u16,
    count: usize,
    reserved: &mut BTreeSet<(u16, u16)>,
    flag_drop_tile: &mut impl FnMut(u16, u16) -> FlagDropTile,
) -> Vec<Option<FlagState>> {
    let owned = matches!(
        behavior,
        FlagDropBehavior::OwnedAtCarrier | FlagDropBehavior::OwnedAtCenter
    );
    let centered = matches!(
        behavior,
        FlagDropBehavior::NeutralAtCenter | FlagDropBehavior::OwnedAtCenter
    );
    let mut local_tiles = if centered {
        Vec::new()
    } else {
        flag_drop_tiles_near(
            seed,
            player.position_x / 16,
            player.position_y / 16,
            drop_radius_tiles,
            count,
            reserved,
            flag_drop_tile,
        )
    }
    .into_iter();
    (0..count)
        .map(|_| {
            let tile = local_tiles
                .next()
                .or_else(|| flag_spawn_tile(seed, spawn, reserved, flag_drop_tile));
            tile.map(|(tile_x, tile_y)| {
                reserved.insert((tile_x, tile_y));
                FlagState::OnMap {
                    x: tile_x.saturating_mul(16).saturating_add(8),
                    y: tile_y.saturating_mul(16).saturating_add(8),
                    owner: owned.then_some(player.frequency),
                }
            })
        })
        .collect()
}

pub(crate) fn flag_drop_tile_near(
    seed: &mut i32,
    start_x: u16,
    start_y: u16,
    desired_walk_distance: u16,
    reserved: &BTreeSet<(u16, u16)>,
    flag_drop_tile: &mut impl FnMut(u16, u16) -> FlagDropTile,
) -> Option<(u16, u16)> {
    flag_drop_tiles_near(
        seed,
        start_x,
        start_y,
        desired_walk_distance,
        1,
        reserved,
        flag_drop_tile,
    )
    .into_iter()
    .next()
}

fn flag_drop_tiles_near(
    seed: &mut i32,
    start_x: u16,
    start_y: u16,
    desired_walk_distance: u16,
    needed: usize,
    reserved: &BTreeSet<(u16, u16)>,
    flag_drop_tile: &mut impl FnMut(u16, u16) -> FlagDropTile,
) -> Vec<(u16, u16)> {
    if needed == 0 {
        return Vec::new();
    }
    let start = (start_x.min(1023), start_y.min(1023));
    let mut visited = BTreeSet::from([start]);
    let mut frontier = BTreeSet::from([start]);
    let mut available = Vec::new();
    if flag_drop_tile(start.0, start.1).available && !reserved.contains(&start) {
        available.push(start);
    }
    let mut walk_distance = i32::from(desired_walk_distance);
    loop {
        if available.len() >= needed {
            if walk_distance <= 0 {
                break;
            }
            walk_distance -= 1;
        }
        let mut next = BTreeSet::new();
        for &(x, y) in &frontier {
            for neighbor in [
                x.checked_sub(1).map(|x| (x, y)),
                y.checked_sub(1).map(|y| (x, y)),
                (x < 1023).then_some((x + 1, y)),
                (y < 1023).then_some((x, y + 1)),
            ]
            .into_iter()
            .flatten()
            {
                if visited.insert(neighbor)
                    && flag_drop_tile(neighbor.0, neighbor.1).fly_through
                    && !flag_drop_tile_is_single_wide(neighbor, flag_drop_tile)
                {
                    next.insert(neighbor);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        for &candidate in &next {
            if flag_drop_tile(candidate.0, candidate.1).available && !reserved.contains(&candidate)
            {
                available.push(candidate);
            }
        }
        frontier = next;
    }
    for index in (1..available.len()).rev() {
        let random_index = crate::vie::vie_rng_next(seed) as usize % (index + 1);
        available.swap(index, random_index);
    }
    available.truncate(needed);
    available
}

fn flag_drop_tile_is_single_wide(
    (x, y): (u16, u16),
    flag_drop_tile: &mut impl FnMut(u16, u16) -> FlagDropTile,
) -> bool {
    let top = y == 0 || !flag_drop_tile(x, y - 1).fly_through;
    let bottom = y == 1023 || !flag_drop_tile(x, y + 1).fly_through;
    if top && bottom {
        return true;
    }
    let left = x == 0 || !flag_drop_tile(x - 1, y).fly_through;
    let right = x == 1023 || !flag_drop_tile(x + 1, y).fly_through;
    left && right
}

fn flag_box_overlaps_ship(
    flag_center_x: u16,
    flag_center_y: u16,
    ship_center_x: u16,
    ship_center_y: u16,
    ship_radius_pixels: u16,
) -> bool {
    const HALF_FLAG_BOX_PIXELS: i32 = 8;
    const REFERENCE_GAME_FLAG_HULL_PADDING_PIXELS: i32 = 1;
    let radius = i32::from(ship_radius_pixels) + REFERENCE_GAME_FLAG_HULL_PADDING_PIXELS;
    let flag_min_x = i32::from(flag_center_x) - HALF_FLAG_BOX_PIXELS;
    let flag_max_x = i32::from(flag_center_x) + HALF_FLAG_BOX_PIXELS;
    let flag_min_y = i32::from(flag_center_y) - HALF_FLAG_BOX_PIXELS;
    let flag_max_y = i32::from(flag_center_y) + HALF_FLAG_BOX_PIXELS;
    let ship_x = i32::from(ship_center_x);
    let ship_y = i32::from(ship_center_y);
    ship_x + radius >= flag_min_x
        && ship_x - radius <= flag_max_x
        && ship_y + radius >= flag_min_y
        && ship_y - radius <= flag_max_y
}

fn strict_square_overlap(x1: u16, y1: u16, x2: u16, y2: u16, radius: u16) -> bool {
    x1.abs_diff(x2) < radius && y1.abs_diff(y2) < radius
}

fn team_requirements_met(
    participants: &[ObjectiveParticipant],
    scoring_frequency: u16,
    limits: ObjectiveTeamLimits,
) -> bool {
    if participants.len() < usize::from(limits.min_players) {
        return false;
    }
    let mut teams = participants
        .iter()
        .map(|participant| participant.frequency % 4)
        .collect::<Vec<_>>();
    teams.sort_unstable();
    teams.dedup();
    if teams.len() < usize::from(limits.min_teams) {
        return false;
    }
    limits.max_players_per_team == 0
        || participants
            .iter()
            .filter(|participant| participant.frequency == scoring_frequency)
            .count()
            <= usize::from(limits.max_players_per_team)
}

pub(crate) fn powerball_game_winner(
    registry: &ObjectiveRegistry,
    profile: &super::profile::EnabledPowerballProfile,
) -> Option<u16> {
    if profile.policy.soccer_mode == 0 {
        return None;
    }
    let winning_team =
        (1..super::registry::POWERBALL_SCORE_BUCKETS as u16).fold(0, |highest, candidate| {
            (registry.powerball_score(candidate) > registry.powerball_score(highest))
                .then_some(candidate)
                .unwrap_or(highest)
        });
    let score = registry.powerball_score(winning_team);
    if profile.policy.capture_points >= 0 {
        return match profile.policy.soccer_mode {
            1 | 2 => (registry.powerball_score(winning_team ^ 1) == 0).then_some(winning_team),
            3..=6 => ((0..super::registry::POWERBALL_SCORE_BUCKETS as u16)
                .filter(|&team| registry.powerball_score(team) == 0)
                .count()
                == 3)
                .then_some(winning_team),
            _ => None,
        };
    }
    let threshold = i64::from(profile.policy.capture_points).saturating_abs();
    if score < threshold {
        return None;
    }
    ((0..super::registry::POWERBALL_SCORE_BUCKETS as u16)
        .filter(|&team| {
            registry
                .powerball_score(team)
                .saturating_add(i64::from(profile.win_by))
                <= score
        })
        .count()
        == super::registry::POWERBALL_SCORE_BUCKETS - 1)
        .then_some(winning_team)
}

fn configured_reward(configured: i32, participant_count: usize) -> i64 {
    if configured < 1 {
        i64::from(configured).saturating_abs()
    } else {
        let players = participant_count as i64;
        players
            .saturating_mul(players)
            .saturating_mul(i64::from(configured))
            / 1_000
    }
}

fn configured_flag_reward(configured: i32, participant_count: usize) -> i64 {
    let players = participant_count as i64;
    players
        .saturating_mul(players)
        .saturating_mul(i64::from(configured))
        / 1_000
}

fn split_flag_reward(
    reward: i64,
    split: FlagRewardSplit,
    eligible: &[ObjectiveParticipant],
) -> Vec<ObjectiveAward> {
    let mut recipients = eligible
        .iter()
        .map(|participant| participant.player_id)
        .collect::<Vec<_>>();
    recipients.sort_unstable();
    recipients.dedup();
    let points = match split {
        FlagRewardSplit::FullPerPlayer => reward,
        FlagRewardSplit::SplitAmongTeam => reward / recipients.len().max(1) as i64,
    };
    recipients
        .into_iter()
        .map(|player_id| ObjectiveAward { player_id, points })
        .collect()
}

pub(crate) fn initialize_objectives(
    registry: &mut ObjectiveRegistry,
    tick: u32,
    mut tile_is_occupied: impl FnMut(u16, u16) -> bool,
    mut flag_drop_tile: impl FnMut(u16, u16) -> FlagDropTile,
) -> Result<ObjectiveStep, ObjectiveRegistryError> {
    let mut staged = registry.clone();
    let mut result = ObjectiveStep::default();
    if let ServerFlagProfile::Enabled(profile) = staged.profile().flags.clone() {
        let mut placed_tiles = BTreeSet::new();
        for _ in 0..profile.count() {
            let (tile_x, tile_y) = {
                flag_spawn_tile(
                    staged.rng_seed_mut(),
                    profile.spawn,
                    &placed_tiles,
                    &mut flag_drop_tile,
                )
                .ok_or(ObjectiveRegistryError::SpawnUnavailable)?
            };
            placed_tiles.insert((tile_x, tile_y));
            result.flags.push(staged.create_flag(FlagState::OnMap {
                x: tile_x.saturating_mul(16).saturating_add(8),
                y: tile_y.saturating_mul(16).saturating_add(8),
                owner: None,
            })?);
        }
    }
    if let ServerPowerballProfile::Enabled(profile) = staged.profile().powerballs.clone() {
        for id in 0..profile.count() {
            let area = profile.spawns[usize::from(id) % profile.spawns.len()];
            let (x, y) =
                ball_spawn_wire_position(staged.rng_seed_mut(), area, &mut tile_is_occupied)
                    .ok_or(ObjectiveRegistryError::SpawnUnavailable)?;
            let ball = staged.create_powerball(PowerballState::OnMap {
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })?;
            staged.set_powerball_motion(
                ball.identity,
                PowerballMotion::from_wire_position(
                    x,
                    y,
                    0,
                    0,
                    profile.policy.friction_delta[0],
                    tick,
                ),
            )?;
            result.powerballs.push(ball);
        }
    }
    *registry = staged;
    Ok(result)
}

pub(crate) fn step_powerball_motion(
    registry: &mut ObjectiveRegistry,
    tick: u32,
    mut tile_is_solid: impl FnMut(Option<u16>, u16, u16) -> bool,
    mut goal_counts: impl FnMut(Option<u16>, u16, u16) -> bool,
) -> Result<Vec<PowerballSnapshot>, ObjectiveRegistryError> {
    let ServerPowerballProfile::Enabled(profile) = registry.profile().powerballs.clone() else {
        return Ok(Vec::new());
    };
    let cadence = u32::from(profile.send_cadence_ticks);
    let mut published = Vec::new();
    for snapshot in registry.replay().powerballs {
        if !matches!(snapshot.state, PowerballState::OnMap { .. }) {
            continue;
        }
        let Some(mut motion) = registry.powerball_motion(snapshot.identity.id) else {
            continue;
        };
        let elapsed = crate::continuum::continuum_tick_diff(tick, motion.last_step_tick);
        if elapsed <= 0 {
            continue;
        }
        let steps = (elapsed as u32).min(MAX_BALL_CATCHUP_TICKS);
        let frequency = motion.last_touch_frequency;
        let mut reached_goal = false;
        for _ in 0..steps {
            simulate_powerball_step(&mut motion, |x, y| tile_is_solid(frequency, x, y));
            let (x, y) = motion.wire_position();
            if goal_counts(frequency, x, y) {
                reached_goal = true;
                break;
            }
        }
        motion.last_step_tick = tick;

        if reached_goal
            || crate::continuum::continuum_tick_elapsed(tick, motion.last_publish_tick) < cadence
        {
            registry.set_powerball_motion(snapshot.identity, motion)?;
            continue;
        }
        let (x, y) = motion.wire_position();
        let pickup_unlock_tick = match snapshot.state {
            PowerballState::OnMap {
                pickup_unlock_tick, ..
            } => pickup_unlock_tick,
            _ => None,
        };
        let published_snapshot = PowerballSnapshot {
            state: PowerballState::OnMap {
                x,
                y,
                velocity_x: motion.velocity_x,
                velocity_y: motion.velocity_y,
                pickup_unlock_tick,
            },
            ..snapshot
        };
        motion.last_publish_tick = tick;
        registry.set_powerball_motion(snapshot.identity, motion)?;
        published.push(published_snapshot);
    }
    Ok(published)
}

pub(crate) fn simulate_powerball_step(
    motion: &mut PowerballMotion,
    mut tile_is_solid: impl FnMut(u16, u16) -> bool,
) {
    if motion.friction == 0 {
        return;
    }
    simulate_axis(
        &mut motion.position_x,
        &mut motion.velocity_x,
        motion.position_y,
        true,
        &mut tile_is_solid,
    );
    simulate_axis(
        &mut motion.position_y,
        &mut motion.velocity_y,
        motion.position_x,
        false,
        &mut tile_is_solid,
    );

    let friction = motion.friction / 1_000;
    motion.velocity_x = scale_velocity(motion.velocity_x, friction);
    motion.velocity_y = scale_velocity(motion.velocity_y, friction);
    motion.friction = motion
        .friction
        .wrapping_sub_signed(i32::from(motion.friction_delta));
}

fn scale_velocity(velocity: i16, friction: u32) -> i16 {
    ((i64::from(velocity) * i64::from(friction)) / 1_000)
        .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

fn simulate_axis(
    position: &mut u32,
    velocity: &mut i16,
    other_position: u32,
    x_axis: bool,
    tile_is_solid: &mut impl FnMut(u16, u16) -> bool,
) {
    let previous = *position;
    *position = add_signed(*position, *velocity);
    if position_hits_solid(*position, other_position, x_axis, tile_is_solid) {
        *position = previous;
        *velocity = velocity.wrapping_neg();
    }
}

fn add_signed(position: u32, velocity: i16) -> u32 {
    position.wrapping_add_signed(i32::from(velocity))
}

fn position_hits_solid(
    axis_position: u32,
    other_position: u32,
    x_axis: bool,
    tile_is_solid: &mut impl FnMut(u16, u16) -> bool,
) -> bool {
    let axis_tile = (axis_position / PowerballMotion::TILE_SCALE) as u16;
    let other_tile = (other_position / PowerballMotion::TILE_SCALE) as u16;
    if x_axis {
        tile_is_solid(axis_tile, other_tile)
    } else {
        tile_is_solid(other_tile, axis_tile)
    }
}

pub(crate) fn flag_spawn_tile(
    seed: &mut i32,
    area: ObjectiveSpawnArea,
    reserved: &BTreeSet<(u16, u16)>,
    flag_drop_tile: &mut impl FnMut(u16, u16) -> FlagDropTile,
) -> Option<(u16, u16)> {
    for attempt in 0..SPAWN_ATTEMPTS {
        let expanded = u32::from(area.radius).saturating_add(attempt);
        let unit_radius = random_unit(seed).sqrt();
        let angle = random_unit(seed) * std::f64::consts::TAU;
        let x = reflect_map_coordinate(
            (f64::from(area.x) + 0.5 + unit_radius * f64::from(expanded) * angle.cos()) as i32,
        );
        let y = reflect_map_coordinate(
            (f64::from(area.y) + 0.5 + unit_radius * f64::from(expanded) * angle.sin()) as i32,
        );
        let Some(found) = nearest_empty_tile(x, y, &mut |x, y| !flag_drop_tile(x, y).empty) else {
            continue;
        };
        if !reserved.contains(&found) && flag_drop_tile(found.0, found.1).available {
            return Some(found);
        }
    }
    None
}

pub(crate) fn ball_spawn_wire_position(
    seed: &mut i32,
    area: ObjectiveSpawnArea,
    tile_is_occupied: &mut impl FnMut(u16, u16) -> bool,
) -> Option<(u16, u16)> {
    let radius = random_unit(seed) * f64::from(area.radius);
    let angle = random_unit(seed) * std::f64::consts::TAU;
    let x = wrap_map_coordinate(i32::from(area.x) + (radius * angle.cos()) as i32);
    let y = wrap_map_coordinate(i32::from(area.y) + (radius * angle.sin()) as i32);
    let (tile_x, tile_y) = nearest_empty_tile(x, y, tile_is_occupied)?;
    let sub_tile = crate::vie::vie_rng_next(seed) as u8;
    Some((
        tile_x
            .saturating_mul(16)
            .saturating_add(u16::from(sub_tile >> 4)),
        tile_y
            .saturating_mul(16)
            .saturating_add(u16::from(sub_tile & 0x0f)),
    ))
}

fn nearest_empty_tile(
    x: u16,
    y: u16,
    tile_is_occupied: &mut impl FnMut(u16, u16) -> bool,
) -> Option<(u16, u16)> {
    #[derive(Clone, Copy)]
    enum Direction {
        Up,
        Right,
        Down,
        Left,
    }

    let mut direction = Direction::Left;
    let mut up_to = 0_i32;
    let mut remaining = 1_i32;
    let mut candidate_x = i32::from(x) + 1;
    let mut candidate_y = i32::from(y);
    loop {
        match direction {
            Direction::Up => candidate_y -= 1,
            Direction::Right => candidate_x += 1,
            Direction::Down => candidate_y += 1,
            Direction::Left => candidate_x -= 1,
        }
        remaining -= 1;
        if remaining == 0 {
            direction = match direction {
                Direction::Up => Direction::Right,
                Direction::Right => Direction::Down,
                Direction::Down => Direction::Left,
                Direction::Left => Direction::Up,
            };
            // The selected current primary literally checks `Up || Up` here.
            // Preserve that walk rather than inferring the likely `Up || Down` intent.
            if matches!(direction, Direction::Up) {
                up_to += 1;
            }
            remaining = up_to;
        }

        if (0..=MAP_TILE_MASK).contains(&candidate_x)
            && (0..=MAP_TILE_MASK).contains(&candidate_y)
            && !tile_is_occupied(candidate_x as u16, candidate_y as u16)
        {
            return Some((candidate_x as u16, candidate_y as u16));
        }
        if up_to >= 35 {
            return None;
        }
    }
}

fn random_unit(seed: &mut i32) -> f64 {
    f64::from(crate::vie::vie_rng_next(seed)) / f64::from(crate::vie::VIE_RNG_MODULUS)
}

fn reflect_map_coordinate(mut value: i32) -> u16 {
    while !(0..=MAP_TILE_MASK).contains(&value) {
        value = if value < 0 {
            -value
        } else {
            MAP_TILE_MASK - (value - MAP_TILE_MASK)
        };
    }
    value as u16
}

fn wrap_map_coordinate(value: i32) -> u16 {
    (value & MAP_TILE_MASK) as u16
}
