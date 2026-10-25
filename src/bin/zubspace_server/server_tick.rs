use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
use super::arena::accept_position_relay_for_runtime_at_tick;
use super::arena::{
    RuntimePositionAdmission, accept_position_relay_for_runtime_at_tick_outcome,
    advance_server_gameplay_energy, apply_movement_correction_application_at_tick,
    begin_input_history_gap_correction, client_position_sequence_advances,
    send_position_relays_batched,
};
#[cfg(test)]
use super::arena_settings::arena_ship_max_speed_pixels_per_second;
use super::arena_settings::{
    arena_mine_lifetime_ticks, arena_prize_claim_retention_ticks,
    arena_ship_weapon_fire_delay_ticks, arena_weapon_lifetime_ticks,
};
use super::hidden_prizes::step_hidden_prizes;
use super::objectives::{
    ObjectiveMutation, evaluate_flag_win, expire_bricks, mutation_packets,
    objective_action_rejection_packets, objective_participants, objective_player,
    score_powerball_goal, step_objective_deadlines, step_powerball_motion, sync_player_flag_counts,
};
use super::reliable::{
    broadcast_reliable_to_joined, retry_pending_movement_correction_delivery,
    send_movement_correction_offer_to_runtime, send_movement_correction_receipt_to_runtime,
    send_reliable_to_runtime, send_reliable_to_session, send_repel_application_receipt_to_runtime,
};
use super::roster::{
    lifecycle_baseline_accepted, repel_application_accepted, warp_relocation_accepted,
};
use super::state::{
    BrickSourceBinding, PendingReliableGameplayMutationKind, ReliableLifecycleMutationOutcome,
    ServerState,
};
use super::zone::{
    MAP_TILES, TILE_SIZE_PX, ZoneMap, position_blocking_tile_for_door_seeds, tile_at_world,
};
use tracing::{debug, info, warn};

// Per-event lines log under this module; see `log_targets`.
const _: () = assert!(crate::log_targets::is_child_of(
    crate::log_targets::SERVER_TICK_COMBAT,
    module_path!()
));

#[path = "server_tick/bots.rs"]
mod bots;
#[path = "server_tick/safety.rs"]
mod safety;
pub(crate) use safety::{
    cancel_owner_weapons_for_admitted_safe_entry, cancel_owner_weapons_for_lifecycle_change,
};

use bots::step_server_bots;

#[cfg(test)]
#[path = "server_tick/projection_measurement.rs"]
pub(crate) mod projection_measurement;

const DEFAULT_PROJECTILE_RETENTION_TICKS: u32 = 12 * 100;
const INPUT_RECOVERY_DEFER_TICKS: u32 = 12;

pub(super) fn projectile_presentation_geometry(
    clients: &std::collections::HashMap<u16, crate::state::ClientHandle>,
    projectile: &crate::authority_projectiles::ProjectileRecord,
    target: &crate::authority_projectiles::PlayerHullSnapshot,
    collision_tick: u32,
) -> crate::combat_timeline::ProjectilePresentationWitnessV1 {
    let Some(binding) = projectile.combat_timeline else {
        return crate::combat_timeline::ProjectilePresentationWitnessV1::server_native();
    };
    let Some(owner) = clients
        .get(&binding.owner.player_id())
        .filter(|owner| owner.joined && owner.session_id == binding.owner.session.session_id)
    else {
        return crate::combat_timeline::ProjectilePresentationWitnessV1::query_unavailable(
            binding,
            crate::combat_timeline::ProjectilePresentationQueryRejection::WrongRuntime,
        );
    };

    let timeline = if owner.life_seq == binding.owner.life.life_seq {
        Some(&owner.observer_combat_timeline)
    } else {
        owner
            .retired_projectile_observers
            .get(&binding.owner.life)
            .filter(|timeline| {
                timeline.observer() == Some(binding.owner)
                    && timeline.arena_stream_id() == Some(owner.arena_stream_id)
            })
    };
    let Some(timeline) = timeline else {
        return crate::combat_timeline::ProjectilePresentationWitnessV1::query_unavailable(
            binding,
            crate::combat_timeline::ProjectilePresentationQueryRejection::WrongRuntime,
        );
    };

    // A local self-bomb is drawn against the owner's local predicted ship, not
    // the remote-observer snapshot interpolation path. The accepted current
    // hull is already the production exact-motion boundary, so record it as a
    // distinct candidate instead of pretending the owner appeared in its own
    // remote snapshot history. This also covers an older own bomb against the
    // successor hull in this same authenticated session; the launch binding
    // retains its original life and never adopts successor input receipts.
    if target.player_id == binding.owner.life.player_id && target.life_seq == owner.life_seq {
        let target_x_q8 = (target.x * 256.0)
            .round()
            .clamp(i32::MIN as f32, i32::MAX as f32) as i32;
        let target_y_q8 = (target.y * 256.0)
            .round()
            .clamp(i32::MIN as f32, i32::MAX as f32) as i32;
        return crate::combat_timeline::ProjectilePresentationWitnessV1::local_self(
            binding,
            collision_tick,
            target_x_q8,
            target_y_q8,
        );
    }

    let query = match timeline.projectile_presentation_query(
        binding,
        collision_tick,
        &owner.observer_presentation_history,
    ) {
        Ok(query) => query,
        Err(rejection) => {
            return crate::combat_timeline::ProjectilePresentationWitnessV1::query_unavailable(
                binding, rejection,
            );
        }
    };
    let Some(segment) = target.motion_segment else {
        return crate::combat_timeline::ProjectilePresentationWitnessV1::projection_unavailable(
            query,
            crate::combat_timeline::PresentationProjectionUnavailable::WrongSegment,
        );
    };
    let resolve = || {
        crate::combat_timeline::PresentationGeometryResolver::resolve_from_samples(
            &owner.observer_presentation_history,
            segment,
            query.render_tick,
            query.applied_snapshot_id,
            &target.motion_samples,
        )
    };
    #[cfg(test)]
    let result = projection_measurement::resolve(
        binding.owner,
        owner.arena_stream_id,
        segment,
        query,
        resolve,
    );
    #[cfg(not(test))]
    let result = resolve();
    match result {
        Ok(anchor) => {
            crate::combat_timeline::ProjectilePresentationWitnessV1::resolved(query, anchor)
        }
        Err(rejection) => {
            crate::combat_timeline::ProjectilePresentationWitnessV1::projection_unavailable(
                query, rejection,
            )
        }
    }
}

fn position_replay_waits_for_started_door_phase(
    client: &crate::state::ClientHandle,
    current_sequence: u16,
    server_tick: u32,
) -> bool {
    let (first_sequence, steps) = match client.last_position_sequence {
        Some(previous) => {
            let steps = current_sequence.wrapping_sub(previous);
            (previous.wrapping_add(1), steps)
        }
        None => (current_sequence, 1),
    };
    if steps == 0 || steps > crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16 {
        return false;
    }
    (0..steps).any(|offset| {
        let sequence = first_sequence.wrapping_add(offset);
        client
            .recent_input_records
            .iter()
            .find(|record| record.sequence == sequence)
            .is_some_and(|record| {
                crate::continuum::continuum_tick_diff(record.door_phase_tick, server_tick) > 0
            })
    })
}

fn maximum_projectile_retention_ticks() -> u32 {
    [1_u16, 3, 7, 8]
        .into_iter()
        .filter_map(arena_weapon_lifetime_ticks)
        .chain([
            DEFAULT_PROJECTILE_RETENTION_TICKS,
            arena_mine_lifetime_ticks(),
        ])
        .max()
        .unwrap_or(DEFAULT_PROJECTILE_RETENTION_TICKS)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerTickResult {
    /// Native measurement only; never feeds load shedding or gameplay clocks.
    #[cfg(test)]
    pub(crate) snapshot_publication_work: Duration,
    pub(crate) tick: u64,
    pub(crate) brick_packets: Vec<Vec<u8>>,
    pub(crate) hidden_prize_packets: Vec<Vec<u8>>,
    pub(crate) expired_projectiles: usize,
    pub(crate) expired_prize_claims: usize,
}

pub(crate) fn server_tick_interval(tick_rate_hz: u16) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(tick_rate_hz.max(1)))
}

/// Phase 2c + Phase 2 polish (door-aware): solid-tile predicate for
/// projectile collision. Walls always block; door tiles consult
/// `door_seeds` (the server's canonical seed list for the current
/// tick) — door tiles whose seeded form maps to `OPEN_DOOR_TILE_ID`
/// let projectiles through, all others block. An empty `door_seeds`
/// fails safe (every door tile blocks); see
/// `position_blocking_tile_for_door_seeds`.
fn tile_at_world_solid(map: &ZoneMap, door_seeds: &[u8], world_x: f32, world_y: f32) -> bool {
    let arena_px = MAP_TILES as f32 * TILE_SIZE_PX;
    if world_x < 0.0 || world_y < 0.0 || world_x >= arena_px || world_y >= arena_px {
        return true;
    }
    let x = world_x.clamp(0.0, u16::MAX as f32) as u16;
    let y = world_y.clamp(0.0, u16::MAX as f32) as u16;
    position_blocking_tile_for_door_seeds(tile_at_world(map, x, y), door_seeds)
}

fn projectile_brick_teams(
    state: &ServerState,
    server_tick: u32,
) -> std::collections::HashMap<(u16, u16), Vec<u16>> {
    let mut teams = std::collections::HashMap::<_, Vec<_>>::new();
    for brick in &state.active_bricks {
        if crate::continuum::continuum_tick_reached(server_tick, brick.expires_at_tick) {
            continue;
        }
        let Some(bytes) = brick.packet.get(9..11) else {
            continue;
        };
        let frequency = u16::from_le_bytes([bytes[0], bytes[1]]);
        for &tile in &brick.tiles {
            teams.entry(tile).or_default().push(frequency);
        }
    }
    teams
}

fn projectile_tile_at_world_solid(
    map: &ZoneMap,
    door_seeds: &[u8],
    brick_teams: &std::collections::HashMap<(u16, u16), Vec<u16>>,
    frequency: u16,
    x: f32,
    y: f32,
) -> bool {
    tile_at_world_solid(map, door_seeds, x, y)
        || brick_teams
            .get(&(
                (x / TILE_SIZE_PX).floor() as u16,
                (y / TILE_SIZE_PX).floor() as u16,
            ))
            .is_some_and(|teams| teams.iter().any(|team| *team != frequency))
}

fn step_server_decoys(state: &mut ServerState, server_tick: u32, map: &ZoneMap, door_seeds: &[u8]) {
    let bricks = state
        .active_bricks
        .iter()
        .filter_map(|brick| {
            let frequency = brick
                .packet
                .get(9..11)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u16::from_le_bytes)?;
            Some((frequency, brick.tiles.clone()))
        })
        .collect::<Vec<_>>();
    for decoy in &mut state.active_decoys {
        if crate::continuum::continuum_tick_reached(server_tick, decoy.expires_at_tick) {
            continue;
        }
        let elapsed =
            crate::continuum::continuum_tick_elapsed(server_tick, decoy.last_step_tick).min(100);
        let decoy_frequency = decoy.frequency;
        for _ in 0..elapsed {
            let is_solid = |world_x: f32, world_y: f32| {
                if tile_at_world_solid(map, door_seeds, world_x, world_y) {
                    return true;
                }
                let tile_x = (world_x / TILE_SIZE_PX).floor().max(0.0) as u16;
                let tile_y = (world_y / TILE_SIZE_PX).floor().max(0.0) as u16;
                bricks.iter().any(|(frequency, tiles)| {
                    *frequency != decoy_frequency && tiles.contains(&(tile_x, tile_y))
                })
            };
            use crate::gameplay_rules::{RulesVector, weapon_motion};
            let mut position = RulesVector::new(decoy.position_x, decoy.position_y);
            let mut velocity = RulesVector::new(decoy.velocity_x, decoy.velocity_y);
            weapon_motion::step_terrain(
                &mut position,
                &mut velocity,
                weapon_motion::STEP_SECONDS,
                true,
                false,
                is_solid,
            );
            decoy.position_x = position.x;
            decoy.position_y = position.y;
            decoy.velocity_x = velocity.x;
            decoy.velocity_y = velocity.y;
            let wire_x = decoy.position_x.clamp(0.0, u16::MAX as f32) as u16;
            let wire_y = decoy.position_y.clamp(0.0, u16::MAX as f32) as u16;
            if crate::zone::tile_at_world(map, wire_x, wire_y) == 220 {
                decoy.expires_at_tick = server_tick;
                break;
            }
        }
        decoy.last_step_tick = server_tick;
    }
    state.active_decoys.retain(|decoy| {
        !crate::continuum::continuum_tick_reached(server_tick, decoy.expires_at_tick)
    });
}

fn step_objectives_for_server_tick(
    shared: &Arc<Mutex<ServerState>>,
    server_tick: u32,
) -> Vec<Vec<u8>> {
    let mut state = shared.lock().expect("server state should not be poisoned");
    state.objectives.expire_powerball_pickup_locks(server_tick);
    let door_seeds = state.server_door_seeds(server_tick);
    let mut brick_teams = std::collections::BTreeMap::<(u16, u16), Vec<u16>>::new();
    for brick in &state.active_bricks {
        let team = brick
            .packet
            .get(9..11)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u16::from_le_bytes)
            .unwrap_or(u16::MAX);
        for &tile in &brick.tiles {
            brick_teams.entry(tile).or_default().push(team);
        }
    }
    let map = state.zone_map();
    let players = state
        .clients
        .keys()
        .copied()
        .filter_map(|player_id| objective_player(&state, player_id))
        .collect::<Vec<_>>();
    let participants = objective_participants(&state);
    let mut mutation = step_objective_deadlines(
        &mut state.objectives,
        &players,
        server_tick,
        |x, y| {
            let index = usize::from(y) * MAP_TILES + usize::from(x);
            map.tiles.get(index).copied().is_none_or(|tile| tile != 0)
        },
        |x, y| {
            let tile = map
                .tiles
                .get(usize::from(y) * MAP_TILES + usize::from(x))
                .copied()
                .unwrap_or(20);
            super::objectives::FlagDropTile::from_map_tile(tile, map.no_flag_drops_at(x, y))
        },
    )
    .unwrap_or_else(|error| {
        warn!(?error, "objective deadline step failed closed");
        ObjectiveMutation::default()
    });
    let soccer_mode = match state.objectives.profile().powerballs {
        super::objectives::ServerPowerballProfile::Enabled(ref profile) => {
            Some(profile.policy.soccer_mode)
        }
        super::objectives::ServerPowerballProfile::Disabled => None,
    };
    // Timed releases above enter the motion registry at this same tick. The
    // fixed step observes their release state but has zero elapsed time, so it
    // cannot double-integrate the canonical release geometry.
    match step_powerball_motion(
        &mut state.objectives,
        server_tick,
        |frequency, x, y| {
            let index = usize::from(y) * MAP_TILES + usize::from(x);
            brick_teams.get(&(x, y)).is_some_and(|teams| {
                frequency.is_none_or(|frequency| teams.iter().any(|team| *team != frequency))
            }) || map
                .tiles
                .get(index)
                .copied()
                .is_none_or(|tile| position_blocking_tile_for_door_seeds(tile, &door_seeds))
        },
        |frequency, x, y| {
            let Some(frequency) = frequency else {
                return false;
            };
            tile_at_world(map, x, y) == 172
                && soccer_mode.is_some_and(|mode| {
                    super::objectives::soccer_goal_counts_world(x, y, frequency, mode)
                })
        },
    ) {
        Ok(powerballs) => mutation.powerballs.extend(powerballs),
        Err(error) => warn!(?error, "powerball motion step failed closed"),
    }
    if let Ok(flag_win) = evaluate_flag_win(&mut state.objectives, &participants, server_tick) {
        mutation.flags.extend(flag_win.flags);
        mutation.awards.extend(flag_win.awards);
        mutation.winning_frequency = flag_win.winning_frequency;
        mutation.flag_victory = flag_win.flag_victory;
    }
    let profile = state.objectives.profile().powerballs.clone();
    if let super::objectives::ServerPowerballProfile::Enabled(profile) = profile {
        let goal_candidates = state.objectives.replay().powerballs;
        for ball in goal_candidates {
            if !matches!(ball.state, super::objectives::PowerballState::OnMap { .. }) {
                continue;
            }
            let Some(motion) = state.objectives.powerball_motion(ball.identity.id) else {
                continue;
            };
            let (world_x, world_y) = motion.wire_position();
            if tile_at_world(map, world_x, world_y) != 172 {
                continue;
            }
            let Some(frequency) = motion.last_touch_frequency else {
                continue;
            };
            if !super::objectives::soccer_goal_counts_world(
                world_x,
                world_y,
                frequency,
                profile.policy.soccer_mode,
            ) {
                continue;
            }
            if let Ok(goal) = score_powerball_goal(
                &mut state.objectives,
                ball.identity,
                ball.version,
                frequency,
                world_x,
                world_y,
                &participants,
                server_tick,
                |tile_x, tile_y| {
                    map.tiles
                        .get(usize::from(tile_y) * MAP_TILES + usize::from(tile_x))
                        .copied()
                        .is_none_or(|tile| tile != 0)
                },
            ) {
                mutation.powerballs.extend(goal.powerballs);
                mutation.awards.extend(goal.awards);
                mutation.score_events.extend(goal.score_events);
                mutation.powerball_scores = goal.powerball_scores;
                mutation.powerball_score_version = goal.powerball_score_version;
                if goal.winning_frequency.is_some() {
                    mutation.winning_frequency = goal.winning_frequency;
                }
            }
        }
    }
    sync_player_flag_counts(&mut state);
    mutation_packets(&mut state, &mutation, server_tick, 0)
}

/// One deterministic application-work slice per 60 Hz tick. Ready runtimes
/// rotate after every round; a deferred head is attempted only once in that
/// tick while terminal prefixes may continue in later rounds. The aggregate
/// residency cap is intentionally much larger than this execution budget so
/// ingress bursts cannot drain all 49,152 entries in one frame.
pub(crate) const MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK: usize = 256;

fn process_pending_reliable_gameplay_mutations(
    shared: &Arc<Mutex<ServerState>>,
    tick: u32,
    now: Instant,
) -> usize {
    let mut terminal_count = 0;
    let mut work_count = 0;
    let mut blocked_runtimes = BTreeSet::new();
    while work_count < MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK {
        let ready_runtimes = {
            let mut state = shared.lock().expect("server state should not be poisoned");
            state.reliable_gameplay_ready_runtimes_with_budget(
                MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK - work_count,
            )
        };
        let ready_runtimes = ready_runtimes
            .into_iter()
            .filter(|identity| !blocked_runtimes.contains(identity))
            .collect::<Vec<_>>();
        if ready_runtimes.is_empty() {
            break;
        }
        for identity in ready_runtimes {
            if work_count >= MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK {
                break;
            }
            let (pending, runtime_len) = {
                let state = shared.lock().expect("server state should not be poisoned");
                (
                    state.reliable_gameplay_head_for_runtime(identity),
                    state.reliable_gameplay_runtime_len(identity),
                )
            };
            let Some(pending) = pending else {
                continue;
            };
            let hard_scope_boundary = matches!(
                &pending.kind,
                PendingReliableGameplayMutationKind::Frequency(_)
                    | PendingReliableGameplayMutationKind::Ship(_)
                    | PendingReliableGameplayMutationKind::Leave
            );
            let reserved_work = if hard_scope_boundary {
                runtime_len.max(1)
            } else {
                1
            };
            if work_count + reserved_work > MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK {
                blocked_runtimes.insert(identity);
                continue;
            }
            work_count += reserved_work;
            if matches!(
                pending.kind,
                PendingReliableGameplayMutationKind::Objective(_)
            ) {
                let outcome =
                    super::objectives::process_reliable_objective_head(shared, pending, tick);
                if outcome == ReliableLifecycleMutationOutcome::Deferred {
                    blocked_runtimes.insert(identity);
                    continue;
                }
                terminal_count += 1;
                continue;
            }
            let ingress_gate = hard_scope_boundary.then(|| {
                shared
                    .lock()
                    .expect("server state should not be poisoned")
                    .player_session_ingress_gate(identity.player_id())
            });
            let _ingress = ingress_gate
                .as_ref()
                .and_then(|gate| gate.as_ref())
                .map(|gate| {
                    gate.lock()
                        .expect("client ingress gate should not be poisoned")
                });
            let later_objective_actions = if hard_scope_boundary {
                shared
                    .lock()
                    .expect("server state should not be poisoned")
                    .objective_actions_for_runtime(identity)
            } else {
                Vec::new()
            };
            let correction_terminal = matches!(
                pending.kind,
                PendingReliableGameplayMutationKind::MovementCorrection(_)
            );
            let outcome = match &pending.kind {
                PendingReliableGameplayMutationKind::Frequency(payload) => {
                    super::arena::update_frequency_for_runtime_instant_at_tick(
                        shared,
                        Some(identity),
                        identity.player_id(),
                        &payload.packet,
                        tick,
                        now,
                    )
                }
                PendingReliableGameplayMutationKind::Ship(payload) => {
                    let payload = payload.encode();
                    super::arena::update_ship_for_runtime_at_tick(
                        shared,
                        Some(identity),
                        identity.player_id(),
                        &payload,
                        tick,
                    )
                }
                PendingReliableGameplayMutationKind::Leave => {
                    super::arena::leave_arena_for_runtime(
                        shared,
                        Some(identity),
                        identity.player_id(),
                        tick,
                    )
                }
                PendingReliableGameplayMutationKind::Attach(payload) => {
                    let payload = payload.encode();
                    super::objectives::relay_attach_request_for_runtime(
                        shared,
                        Some(identity),
                        identity.player_id(),
                        &payload,
                    );
                    ReliableLifecycleMutationOutcome::Applied
                }
                PendingReliableGameplayMutationKind::DropAttached => {
                    super::objectives::relay_drop_attached_for_runtime(
                        shared,
                        Some(identity),
                        identity.player_id(),
                    );
                    ReliableLifecycleMutationOutcome::Applied
                }
                PendingReliableGameplayMutationKind::Brick { request, source } => match source {
                    BrickSourceBinding::Pending => ReliableLifecycleMutationOutcome::Deferred,
                    BrickSourceBinding::Unavailable => {
                        ReliableLifecycleMutationOutcome::TerminalNoop
                    }
                    BrickSourceBinding::Bound(source) => {
                        let payload = request.encode();
                        super::objectives::relay_drop_brick_for_runtime_at_tick(
                            shared, identity, &payload, *source, tick,
                        )
                    }
                },
                PendingReliableGameplayMutationKind::Prize(payload) => {
                    let payload = payload.encode();
                    super::player_events::relay_prize_pickup_for_runtime_at_tick(
                        shared,
                        Some(identity),
                        identity.player_id(),
                        &payload,
                        tick,
                    )
                }
                PendingReliableGameplayMutationKind::MovementCorrection(application) => {
                    let receipt = {
                        let mut state = shared.lock().expect("server state should not be poisoned");
                        apply_movement_correction_application_at_tick(
                            &mut state,
                            identity,
                            *application,
                            tick,
                        )
                    };
                    if let Some(receipt) = receipt {
                        send_movement_correction_receipt_to_runtime(shared, identity, receipt);
                        ReliableLifecycleMutationOutcome::Applied
                    } else {
                        let cancellation = {
                            let mut state =
                                shared.lock().expect("server state should not be poisoned");
                            super::arena::cancelled_movement_correction_receipt(
                                &mut state,
                                identity,
                                *application,
                                tick,
                            )
                        };
                        if let Some(receipt) = cancellation {
                            send_movement_correction_receipt_to_runtime(shared, identity, receipt);
                        }
                        ReliableLifecycleMutationOutcome::TerminalNoop
                    }
                }
                PendingReliableGameplayMutationKind::Objective(_) => unreachable!(),
            };
            if outcome == ReliableLifecycleMutationOutcome::Deferred {
                blocked_runtimes.insert(identity);
                continue;
            }
            terminal_count += 1;
            {
                let mut state = shared.lock().expect("server state should not be poisoned");
                let _ = state.pop_reliable_gameplay_terminal(&pending);
                if correction_terminal
                    || (hard_scope_boundary
                        && outcome == ReliableLifecycleMutationOutcome::TerminalNoop)
                {
                    state.bind_pending_brick_source_for_runtime_after_correction(identity);
                }
            }
            if hard_scope_boundary && outcome == ReliableLifecycleMutationOutcome::Applied {
                let terminal_packets = {
                    let state = shared.lock().expect("server state should not be poisoned");
                    objective_action_rejection_packets(&state, &later_objective_actions, tick)
                };
                for packet in terminal_packets {
                    send_reliable_to_session(shared, identity.session, packet);
                }
                continue;
            }
        }
    }
    terminal_count
}

pub(crate) fn run_server_tick(state: &Arc<Mutex<ServerState>>, now: Instant) -> ServerTickResult {
    run_server_tick_at_tick(state, now, crate::wire::current_tick())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollisionHarmfulReadiness {
    SelfHit,
    ServerNativeVictim,
    MissingSourceRuntime,
    StaleHumanVictim,
    Recipient {
        source: crate::threat_presentation::ThreatReadinessDecision,
        exact_projectile: Option<crate::threat_presentation::ProjectileThreatReadinessDecision>,
        shot_group: Option<crate::threat_presentation::ProjectileThreatReadinessDecision>,
    },
}

impl CollisionHarmfulReadiness {
    fn harmful(self) -> bool {
        match self {
            Self::SelfHit | Self::ServerNativeVictim => true,
            Self::MissingSourceRuntime | Self::StaleHumanVictim => false,
            Self::Recipient {
                source,
                exact_projectile,
                shot_group,
            } => {
                exact_projectile.is_some_and(|decision| decision.harmful)
                    || (source.harmful && shot_group.is_some_and(|decision| decision.harmful))
                    || (exact_projectile.is_none() && shot_group.is_none() && source.harmful)
            }
        }
    }

    fn may_advance_or_terminate(self) -> bool {
        self.harmful() || matches!(self, Self::MissingSourceRuntime | Self::StaleHumanVictim)
    }

    fn reason(self) -> &'static str {
        match self {
            Self::SelfHit => "self_hit",
            Self::ServerNativeVictim => "server_native_victim",
            Self::MissingSourceRuntime => "missing_source_runtime",
            Self::StaleHumanVictim => "stale_human_victim",
            Self::Recipient {
                source,
                exact_projectile,
                shot_group,
            } => {
                if exact_projectile.is_some_and(|decision| decision.harmful) {
                    return exact_projectile
                        .expect("checked exact projectile readiness")
                        .reason
                        .as_str();
                }
                if shot_group.is_some_and(|decision| decision.harmful) {
                    return if source.harmful {
                        shot_group
                            .expect("checked shot-group readiness")
                            .reason
                            .as_str()
                    } else {
                        source.reason.as_str()
                    };
                }
                exact_projectile
                    .and_then(|decision| decision.contract.map(|_| decision.reason.as_str()))
                    .or_else(|| {
                        shot_group.and_then(|decision| {
                            decision.contract.map(|_| decision.reason.as_str())
                        })
                    })
                    .unwrap_or_else(|| source.reason.as_str())
            }
        }
    }

    fn contract(self) -> Option<crate::threat_presentation::ThreatPresentationContract> {
        match self {
            Self::Recipient { source, .. } => source.contract,
            _ => None,
        }
    }

    fn projectile_contract(
        self,
    ) -> Option<crate::threat_presentation::ProjectilePresentationContract> {
        match self {
            Self::Recipient {
                exact_projectile,
                shot_group,
                ..
            } => exact_projectile
                .and_then(|decision| decision.contract)
                .or_else(|| shot_group.and_then(|decision| decision.contract)),
            _ => None,
        }
    }
}

fn projectile_presentation_identity(
    projectile: &crate::authority_projectiles::ProjectileRecord,
) -> Option<crate::threat_presentation::ProjectilePresentationIdentity> {
    let source = projectile.presentation_source?;
    let source_runtime = crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            source.source_player_id,
            crate::authority_identity::PlayerSessionId::new(source.source_session_id),
        ),
        crate::authority_identity::LifeIdentity::new(
            source.source_player_id,
            source.source_life_seq,
        ),
    );
    crate::threat_presentation::ProjectilePresentationIdentity::projectile(
        source_runtime,
        projectile.shot_group_id,
        projectile.id,
    )
}

fn recipient_collision_readiness(
    victim: &crate::state::ClientHandle,
    source: crate::authority_identity::PlayerRuntimeIdentity,
    projectile: Option<crate::threat_presentation::ProjectilePresentationIdentity>,
    server_tick: u32,
) -> CollisionHarmfulReadiness {
    let source_decision = victim.threat_presentation.readiness(source, server_tick);
    let (exact_projectile, shot_group) = projectile.map_or((None, None), |projectile| {
        (
            Some(
                victim
                    .threat_presentation
                    .exact_projectile_readiness(projectile, server_tick),
            ),
            Some(
                victim
                    .threat_presentation
                    .shot_group_readiness(projectile, server_tick),
            ),
        )
    });
    CollisionHarmfulReadiness::Recipient {
        source: source_decision,
        exact_projectile,
        shot_group,
    }
}

fn collision_harmful_readiness(
    clients: &std::collections::HashMap<u16, crate::state::ClientHandle>,
    active_actor_runtimes: &std::collections::HashMap<
        crate::authority_identity::LifeIdentity,
        crate::authority_identity::PlayerRuntimeIdentity,
    >,
    projectile_presentations: &std::collections::HashMap<
        u32,
        crate::threat_presentation::ProjectilePresentationIdentity,
    >,
    hit: &crate::authority_projectiles::ProjectileHit,
    server_tick: u32,
) -> CollisionHarmfulReadiness {
    if hit.owner_identity() == hit.victim_identity() {
        return CollisionHarmfulReadiness::SelfHit;
    }
    let Some(victim) = clients.get(&hit.victim_id) else {
        return CollisionHarmfulReadiness::ServerNativeVictim;
    };
    if !victim.joined || victim.life_seq != hit.victim_life_seq {
        return CollisionHarmfulReadiness::StaleHumanVictim;
    }
    let projectile = projectile_presentations.get(&hit.projectile_id).copied();
    let source = projectile
        .map(|identity| identity.source)
        .or_else(|| active_actor_runtimes.get(&hit.owner_identity()).copied());
    let Some(source) = source.filter(|source| source.life == hit.owner_identity()) else {
        return CollisionHarmfulReadiness::MissingSourceRuntime;
    };
    if hit.owner_id == hit.victim_id && victim.session_id == source.session.session_id {
        // Surviving own explosives can splash the successor hull. Self flight
        // is locally presented and requires no remote source-emission barrier.
        return CollisionHarmfulReadiness::SelfHit;
    }
    recipient_collision_readiness(victim, source, projectile, server_tick)
}

fn frozen_transaction_harmful_ready(
    state: &crate::state::ServerState,
    transaction: &crate::server_damage::PreparedDamageTransaction,
    server_tick: u32,
) -> bool {
    // A lifecycle-invalid transaction must reach the ordinary application
    // validator so it can terminate and release its projectile. Readiness is
    // a bounded wait only for an otherwise-current immutable transaction.
    if !transaction.identities_are_current(state) {
        return true;
    }
    let source = transaction.shooter_runtime();
    let projectile = state
        .active_projectiles
        .iter()
        .find(|projectile| projectile.id == transaction.projectile_id())
        .and_then(projectile_presentation_identity);
    transaction.human_victim_runtimes().all(|victim_runtime| {
        if victim_runtime.player_id() == source.player_id() {
            return true;
        }
        state
            .clients
            .get(&victim_runtime.player_id())
            .is_some_and(|victim| {
                state.active_player_runtime_identity(victim_runtime.player_id())
                    == Some(victim_runtime)
                    && recipient_collision_readiness(victim, source, projectile, server_tick)
                        .harmful()
            })
    })
}

pub(crate) fn run_server_tick_at_tick(
    state: &Arc<Mutex<ServerState>>,
    now: Instant,
    server_tick: u32,
) -> ServerTickResult {
    let arena_gate = state
        .lock()
        .expect("server state should not be poisoned")
        .arena_transaction_gate
        .clone();
    let _arena_transaction = arena_gate
        .write()
        .expect("arena transaction gate should not be poisoned");
    let tick_started = Instant::now();
    // Recovery for un-acked reliable frames. `send_reliable_frame_tracked` states that the reliable channel's own
    // retransmit logic in `resend_due_reliable_to_clients` handles recovery, and drops simulated-loss frames to exercise
    // that path - but until now nothing in the live tick called it, and it was reached only from tests. Without it, one
    // lost acknowledgement is permanent: the bounded application-transfer window stays full, the zone-file transfer
    // freezes mid-file, `ZoneFilesPublished` never fires, the client never enters gameplay authority, never sends a
    // position packet, and can never be shown to a peer. Running it every tick bounds the cost of a lost ack to one
    // resend interval instead of the rest of the session.
    let _ = crate::reliable::resend_due_reliable_to_clients(state, now);
    let state_arc = state;
    let tick = state
        .lock()
        .expect("server state should not be poisoned")
        .advance_arena_tick(now);
    let step_objectives_after_positions = || {
        let objective_packets = step_objectives_for_server_tick(state, server_tick);
        for packet in objective_packets {
            broadcast_reliable_to_joined(state, packet);
        }
    };
    // Phase 4 send pacing: refill each client's outbound token
    // bucket once per server tick. Must happen before any sender
    // path that calls `try_consume`.
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let tick_rate = state.config.tick_rate_hz;
        let quality_host_healthy = state.tick_wallclock_ewma_us
            < (4_000_000.0 / f64::from(tick_rate.max(1))).max(50_000.0);
        let projectile_retention_ticks = maximum_projectile_retention_ticks();
        for client in state.clients.values_mut() {
            crate::connection_quality::poll_client(client, now, quality_host_healthy);
            client.send_pacing.refill_for_tick(tick_rate);
            if let Some(door_state) = client.door_state.as_mut() {
                crate::doors::rebase_door_state_at_tick(
                    door_state,
                    &mut client.door_seed_cache,
                    server_tick,
                );
            }
            if client.joined && client.alive && client.player.ship < 8 {
                advance_server_gameplay_energy(client, server_tick);
            }
            let ship = client.player.ship;
            let gun_cooldown_ticks = [1_u8, 2, 15]
                .into_iter()
                .map(|weapon_type| arena_ship_weapon_fire_delay_ticks(ship, weapon_type))
                .max()
                .unwrap_or(0)
                .max(crate::arena_settings::arena_ship_multifire_delay_ticks(
                    ship,
                ));
            let bomb_cooldown_ticks = (3_u8..=8)
                .map(|weapon_type| arena_ship_weapon_fire_delay_ticks(ship, weapon_type))
                .max()
                .unwrap_or(0);
            client.maintain_short_tick_state(
                server_tick,
                gun_cooldown_ticks,
                bomb_cooldown_ticks,
                arena_ship_weapon_fire_delay_ticks(ship, 5),
                projectile_retention_ticks,
            );
        }
        crate::connection_quality::adapt_presentation_in_state(
            &mut state,
            server_tick,
            now,
            quality_host_healthy,
        );
        crate::connection_quality::enforce_in_state(&mut state, server_tick, now);
    }
    let position_inputs = state
        .lock()
        .expect("server state should not be poisoned")
        .drain_position_inputs();
    let mut position_relays = Vec::new();
    let mut deferred_positions = Vec::new();
    let mut admitted_deferred_movements = std::collections::BTreeSet::new();
    for (identity, packet, was_deferred, lifecycle_baseline_delivery_id) in position_inputs {
        let life_identity = identity.life;
        let player_id = identity.player_id();
        let parsed_snapshot = crate::continuum_position::parse_client_position_snapshot(&packet);
        let movement_only = parsed_snapshot.is_some_and(|snapshot| snapshot.weapon == 0);
        if movement_only && admitted_deferred_movements.contains(&player_id) {
            // An older deferred movement was just admitted at this same
            // server tick. Keep the newer movement for the next tick so
            // position acceleration is never evaluated across zero elapsed
            // server time.
            deferred_positions.push((identity, packet, false, lifecycle_baseline_delivery_id));
            continue;
        }
        enum InputCoverage {
            Ready,
            Defer { requires_timer: bool },
            Reject,
            Correction(crate::movement_correction::MovementCorrection),
        }
        let input_coverage = match parsed_snapshot {
            Some(snapshot) => match snapshot.client_sequence {
                Some(sequence) => {
                    let mut state = state.lock().expect("server state should not be poisoned");
                    if !state.player_runtime_matches(identity) {
                        InputCoverage::Reject
                    } else {
                        let Some(client) = state.clients.get(&player_id) else {
                            continue;
                        };
                        if lifecycle_baseline_delivery_id.is_none()
                            && client.pending_movement_correction.as_ref().is_some_and(|pending| {
                                pending.proposal.reason
                                    == crate::movement_correction::MovementCorrection::REASON_INPUT_HISTORY_GAP
                            })
                        {
                            InputCoverage::Reject
                        } else {
                        let sequence_advances = client_position_sequence_advances(
                            client.last_position_sequence,
                            sequence,
                        );
                        match crate::sim_observation::validate_position_input_witness(
                            client,
                            sequence,
                            snapshot.direction,
                            server_tick,
                        ) {
                            Ok(_) => {
                                let waits_for_door_phase = sequence_advances
                                    && position_replay_waits_for_started_door_phase(
                                        client,
                                        sequence,
                                        server_tick,
                                    );
                                if waits_for_door_phase {
                                    // Input ingress resolves authenticated door phases against
                                    // the live Continuum clock. It can run just after this tick
                                    // captured `server_tick`, leaving an otherwise complete
                                    // position one 10 ms phase ahead. Retain the exact packet
                                    // until that already-authenticated phase has started instead
                                    // of misclassifying normal task scheduling as a canonical
                                    // movement mismatch.
                                    let first_tick = *state
                                        .input_recovery_deferred_at
                                        .entry(life_identity)
                                        .or_insert(server_tick);
                                    if crate::continuum::continuum_tick_elapsed(
                                        server_tick,
                                        first_tick,
                                    ) <= INPUT_RECOVERY_DEFER_TICKS
                                    {
                                        InputCoverage::Defer {
                                            requires_timer: true,
                                        }
                                    } else {
                                        state.input_recovery_deferred_at.remove(&life_identity);
                                        crate::security::log_security_reject(
                                            player_id,
                                            "position",
                                            "door_phase_future",
                                        );
                                        InputCoverage::Reject
                                    }
                                } else {
                                    if sequence_advances {
                                        state.input_recovery_deferred_at.remove(&life_identity);
                                    }
                                    InputCoverage::Ready
                                }
                            }
                            Err(crate::sim_observation::PositionInputWitnessError::Missing)
                                if sequence_advances =>
                            {
                                let missing_input_sequence = crate::sim_observation::first_missing_position_input_sequence(client, sequence);
                                let required_input_sequence = missing_input_sequence.or_else(||
                                    client.last_position_sequence.map(|last| last.wrapping_add(1)));
                                let irrecoverable = required_input_sequence
                                    .zip(client.last_input_seq)
                                    .is_some_and(|(missing, newest_input)| {
                                        let age = newest_input.wrapping_sub(missing);
                                        // Server retention is deliberately wider than one
                                        // wire bundle, but it cannot make a missing record
                                        // recoverable after the actual missing record falls out of the
                                        // browser's 32-entry rolling packet. Open the exact
                                        // correction barrier at the wire boundary instead of
                                        // waiting for the 64-entry retention cache to age out.
                                        age < (1 << 15)
                                            && age as usize
                                                >= crate::connection::CLIENT_INPUT_RECORD_MAX_INPUTS
                                    });
                                if irrecoverable {
                                    let (
                                        position_x,
                                        position_y,
                                        velocity_x,
                                        velocity_y,
                                        orientation,
                                    ) = snapshot.canonical_state.values();
                                    let detected_state = crate::state::ServerSimMovement {
                                        position_x,
                                        position_y,
                                        velocity_x,
                                        velocity_y,
                                        orientation,
                                        energy: f32::from(client.server_energy),
                                        max_energy: f32::from(client.server_max_energy),
                                    };
                                    let proposal = begin_input_history_gap_correction(
                                        &mut state,
                                        identity,
                                        sequence,
                                        detected_state,
                                        server_tick,
                                    );
                                    state.input_recovery_deferred_at.remove(&life_identity);
                                    if let Some(proposal) = proposal {
                                        warn!(
                                            player_id,
                                            life_seq = life_identity.life_seq,
                                            current_sequence = sequence,
                                            last_position_sequence = ?state.clients
                                                .get(&player_id)
                                                .and_then(|current| current.last_position_sequence),
                                            last_input_sequence = ?state.clients
                                                .get(&player_id)
                                                .and_then(|current| current.last_input_seq),
                                            missing_input_sequence = ?missing_input_sequence,
                                            required_input_sequence = ?required_input_sequence,
                                            retained_input_count = state.clients
                                                .get(&player_id)
                                                .map_or(0, |current| current.recent_input_records.len()),
                                            correction_generation = proposal.correction_generation,
                                            "position input history exhausted; opening exact correction barrier"
                                        );
                                        InputCoverage::Correction(proposal)
                                    } else {
                                        InputCoverage::Reject
                                    }
                                } else {
                                    // One unordered position stays bounded by the per-player
                                    // coalescing queue while the browser can still repeat its
                                    // missing witness in the exact 32-record rolling bundle.
                                    // Firefox may pause a content task for longer than a fixed
                                    // server-tick grace period even though RTC remains healthy;
                                    // wall time is therefore not evidence that the witness is
                                    // lost or hostile. Protocol progress above is the authority
                                    // boundary: once an actually missing sequence is outside the
                                    // wire window, `irrecoverable` opens an exact correction.
                                    // If input stops entirely, this retains only one bounded
                                    // movement until lifecycle/session cleanup.
                                    InputCoverage::Defer {
                                        requires_timer: false,
                                    }
                                }
                            }
                            Err(error) => {
                                if sequence_advances {
                                    state.input_recovery_deferred_at.remove(&life_identity);
                                }
                                if error
                                    == crate::sim_observation::PositionInputWitnessError::InvalidDirectionHistory
                                {
                                    if let Some(client) = state.clients.get(&player_id) {
                                        tracing::warn!(
                                            player_id,
                                            current_sequence = sequence,
                                            snapshot_direction = snapshot.direction,
                                            last_position_sequence = ?client.last_position_sequence,
                                            last_published_direction = ?client
                                                .last_position
                                                .as_deref()
                                                .and_then(|position| position.get(1).copied()),
                                            private_orientation = ?client
                                                .server_sim_state
                                                .map(|sim| sim.orientation),
                                            rotation_per_second = client.server_rotation_per_second,
                                            canonical_movement_witness = ?client
                                                .canonical_movement_witness,
                                            recent_inputs = ?client.recent_input_records,
                                            "queued position direction witness is impossible"
                                        );
                                    }
                                }
                                // A stale movement frame has no side effects and may
                                // have legitimately fallen out of the bounded input
                                // window. Stale weapons and all impossible direction
                                // bindings remain explicit security rejects, except a
                                // stale weapon mirror whose input record is a hole
                                // strictly inside the retained input window: that
                                // record cannot have aged out, so its absence is
                                // provable transport loss and the inert frame drops
                                // without an attack classification.
                                let lost_input_in_window = error
                                    == crate::sim_observation::PositionInputWitnessError::Missing
                                    && state
                                        .clients
                                        .get(&player_id)
                                        .is_some_and(|client| {
                                            crate::sim_observation::input_record_lost_inside_retained_window(
                                                client, sequence,
                                            )
                                        });
                                if sequence_advances
                                    || snapshot.weapon != 0
                                    || error
                                        != crate::sim_observation::PositionInputWitnessError::Missing
                                {
                                    if lost_input_in_window {
                                        debug!(
                                            player_id,
                                            server_tick,
                                            position_sequence = sequence,
                                            reported_weapon = snapshot.weapon,
                                            last_position_sequence = ?state
                                                .clients
                                                .get(&player_id)
                                                .and_then(|client| client.last_position_sequence),
                                            "dropping stale position with a lost input witness"
                                        );
                                    } else {
                                        if error
                                            == crate::sim_observation::PositionInputWitnessError::Missing
                                        {
                                            if let Some(client) = state.clients.get(&player_id) {
                                                crate::sim_observation::trace_missing_position_input_witness(
                                                    client,
                                                    player_id,
                                                    sequence,
                                                    snapshot.direction,
                                                    snapshot.weapon,
                                                    server_tick,
                                                    "tick_input_coverage",
                                                );
                                            }
                                        }
                                        crate::security::log_security_reject(
                                            player_id,
                                            "position",
                                            error.security_reason(),
                                        );
                                    }
                                }
                                InputCoverage::Reject
                            }
                        }
                        }
                    }
                }
                None => InputCoverage::Ready,
            },
            None => InputCoverage::Ready,
        };
        if let InputCoverage::Defer { requires_timer } = input_coverage {
            deferred_positions.push((
                identity,
                packet,
                requires_timer,
                lifecycle_baseline_delivery_id,
            ));
            continue;
        }
        if let InputCoverage::Correction(proposal) = input_coverage {
            send_movement_correction_offer_to_runtime(state, identity, proposal);
            continue;
        }
        if matches!(input_coverage, InputCoverage::Reject) {
            continue;
        }
        let lifecycle_baseline_waiting_for_respawn = lifecycle_baseline_delivery_id.is_some() && {
            let locked = state.lock().expect("server state should not be poisoned");
            locked.player_runtime_matches(identity)
                && locked.clients.get(&player_id).is_some_and(|client| {
                    !client.alive
                        && client.respawn_at_tick.is_some_and(|respawn_at_tick| {
                            !crate::continuum::continuum_tick_reached(server_tick, respawn_at_tick)
                        })
                })
        };
        if lifecycle_baseline_waiting_for_respawn {
            // The browser's wall-time respawn timer and the authoritative
            // Continuum clock can differ by a scheduling frame. Keep the
            // exact application-reliable neutral baseline behind its bounded
            // protected queue until the server deadline instead of consuming
            // its transport wrapper without an application receipt.
            deferred_positions.push((identity, packet, false, lifecycle_baseline_delivery_id));
            continue;
        }
        // Server-tick scheduling fixtures deliberately use synthetic legacy
        // positions while testing queue ordering, recovery, recharge, and
        // receipt routing. Give each such packet an explicit test-only exact
        // boundary here; production and the focused authority tests enter
        // `accept_position_relay_for_runtime_at_tick` without this adapter.
        // Authenticated reliable lifecycle baselines must also keep their
        // real admission path, including dead-life neutral respawn checks.
        #[cfg(test)]
        if let Some(snapshot) = parsed_snapshot.filter(|_| lifecycle_baseline_delivery_id.is_none())
        {
            let mut locked = state.lock().expect("server state should not be poisoned");
            if let Some(client) = locked.clients.get_mut(&player_id) {
                client.pending_frequency_baseline = Some(snapshot);
            }
        }
        let admission = accept_position_relay_for_runtime_at_tick_outcome(
            state,
            identity,
            &packet,
            server_tick,
        );
        if matches!(
            admission,
            RuntimePositionAdmission::DeferredWeapon | RuntimePositionAdmission::DeferredSpawn
        ) {
            deferred_positions.push((
                identity,
                packet,
                // Spawn geometry, like event capacity, is retried by the
                // next server tick. This packet has no missing-input gap
                // and must not depend on an input-recovery timer entry.
                false,
                lifecycle_baseline_delivery_id,
            ));
            continue;
        }
        if let RuntimePositionAdmission::Accepted((accepted_identity, relay)) = admission {
            if let Some(reliable_delivery_id) = lifecycle_baseline_delivery_id {
                let sequence = parsed_snapshot
                    .and_then(|snapshot| snapshot.client_sequence)
                    .expect("admitted lifecycle baseline must carry a sequence");
                send_reliable_to_runtime(
                    state,
                    accepted_identity,
                    lifecycle_baseline_accepted(
                        reliable_delivery_id,
                        accepted_identity.life.life_seq,
                        sequence,
                        server_tick,
                        crate::arena::large_position_coordinates(&relay.position)
                            .expect("admitted lifecycle baseline has a public position"),
                    ),
                );
            }
            if let Some((collectible_id, life_seq, sequence)) = relay.warp_relocation_receipt {
                send_reliable_to_runtime(
                    state,
                    accepted_identity,
                    warp_relocation_accepted(collectible_id, life_seq, sequence),
                );
            }
            if let Some((
                server_session_id,
                life_seq,
                generation,
                snapshot_id,
                sequence,
                accepted_end_sequence,
                active_steps,
            )) = relay.repel_application_receipt
            {
                send_repel_application_receipt_to_runtime(
                    state,
                    accepted_identity,
                    generation,
                    repel_application_accepted(
                        server_session_id,
                        life_seq,
                        generation,
                        snapshot_id,
                        sequence,
                        accepted_end_sequence,
                        active_steps,
                    ),
                );
            }
            if let Some(correction) = relay.movement_correction {
                send_movement_correction_offer_to_runtime(state, accepted_identity, correction);
            }
            position_relays.push(relay);
        } else if let Some(reliable_delivery_id) = lifecycle_baseline_delivery_id {
            let locked = state.lock().expect("server state should not be poisoned");
            let client = locked.clients.get(&player_id);
            warn!(
                player_id,
                life_seq = identity.life.life_seq,
                reliable_delivery_id,
                client_sequence = ?parsed_snapshot.and_then(|snapshot| snapshot.client_sequence),
                active_runtime = ?locked.active_player_runtime_identity(player_id),
                life_position_sequence_base = ?client.and_then(|client| client.life_position_sequence_base),
                last_position_sequence = ?client.and_then(|client| client.last_position_sequence),
                last_input_sequence = ?client.and_then(|client| client.last_input_seq),
                pending_frequency_baseline = client.is_some_and(|client| client.pending_frequency_baseline.is_some()),
                joined = client.is_some_and(|client| client.joined),
                alive = client.is_some_and(|client| client.alive),
                "authoritative lifecycle baseline rejected before application receipt"
            );
        }
        if was_deferred && movement_only {
            admitted_deferred_movements.insert(player_id);
        }
    }
    if !deferred_positions.is_empty() {
        let mut state = state.lock().expect("server state should not be poisoned");
        for (identity, packet, requires_recovery_timer, lifecycle_baseline_delivery_id) in
            deferred_positions
        {
            if !requires_recovery_timer
                || state
                    .input_recovery_deferred_at
                    .contains_key(&identity.life)
            {
                if let Some(reliable_delivery_id) = lifecycle_baseline_delivery_id {
                    state
                        .pending_position_inputs
                        .requeue_deferred_lifecycle_baseline(
                            identity,
                            packet,
                            reliable_delivery_id,
                        );
                } else {
                    state
                        .pending_position_inputs
                        .requeue_deferred(identity, packet);
                }
            }
        }
    }
    crate::spawn_idle::enforce(
        &mut state.lock().expect("server state lock"),
        server_tick,
        now,
    );
    let bots_started_us = tick_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    let bot_tick = step_server_bots(state, server_tick);
    let bots_finished_us = tick_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    position_relays.extend(bot_tick.positions);
    send_position_relays_batched(state, position_relays);
    for packet in bot_tick.reliable_packets {
        broadcast_reliable_to_joined(state, packet);
    }
    state
        .lock()
        .expect("server state should not be poisoned")
        .bind_pending_brick_sources_after_positions();
    // Drain each runtime's exact reliable gameplay prefix after this tick's
    // positions have established source geometry. Correction application,
    // Brick, prize, objective, and lifecycle mutations therefore observe one
    // FIFO while independent runtimes continue to make progress.
    let _ = process_pending_reliable_gameplay_mutations(state, server_tick, now);
    // The reference game evaluates player-relative flag/ball timers from the live ship
    // state for this frame. Admit every current canonical position first, but
    // keep timed releases inside the objective phase before ball motion.
    step_objectives_after_positions();
    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        crate::objectives::queue_enemy_brick_overlap_corrections(&mut guard, server_tick);
    }
    retry_pending_movement_correction_delivery(state);

    {
        // Keep drain, admission, and deferred requeue atomic. Otherwise fresh
        // ingress could fill the temporarily empty global queue between locks
        // and strand an already-admitted request without a terminal.
        let mut state = state.lock().expect("server state should not be poisoned");
        let portal_placements = state.drain_portal_placements();
        for pending in portal_placements {
            let outcome =
                crate::arena::apply_portal_placement_at_tick(&mut state, pending, server_tick);
            if outcome == crate::arena::PortalPlacementOutcome::Deferred {
                let retained = state.requeue_deferred_portal_placement(pending);
                debug_assert!(
                    retained,
                    "drained Portal request must retain its queue slot"
                );
            }
        }
        let portal_warps = state.drain_portal_warps();
        for pending in portal_warps {
            let outcome = crate::arena::apply_portal_warp_at_tick(&mut state, pending, server_tick);
            if outcome == crate::arena::PortalWarpOutcome::Deferred {
                let retained = state.requeue_deferred_portal_warp(pending);
                debug_assert!(retained, "drained Portal Warp must retain its queue slot");
            }
        }
    }

    let brick_packets = expire_bricks(state, server_tick);
    let hidden_prize_packets = step_hidden_prizes(state, server_tick);
    let (hits, expired_projectiles, expired_prize_claims) = {
        let mut state = state.lock().expect("server state should not be poisoned");
        let contact_diagnostics_enabled = state.config.client_debug_trace;
        state
            .active_projectiles
            .set_contact_diagnostics_enabled(contact_diagnostics_enabled);
        let projectile_count = state.active_projectiles.len();
        // Phase 2b/2c/2d: advance every projectile by the elapsed
        // Continuum ticks, run axis-swept tile collision, and detect
        // hits on alive player hulls. Hits are observation-only for
        // now (logged via stderr at high deviation); Phase 2f turns
        // them into authoritative damage events.
        // `zone_map()` returns `&'static ZoneMap`, so the implicit
        // copy of the reference is already cheap; no `.clone()`
        // needed (and `ZoneMap` doesn't impl Clone anyway). B4 fix.
        let map = state.zone_map();
        // Phase 2 polish (door-aware projectile collision): compute
        // the server's canonical door seeds for this tick BEFORE the
        // split-borrow destructure below. `server_door_seeds()` takes
        // `&mut self` (to refresh the cache) and returns an owned
        // `Vec<u8>`, so the borrow on `state` is released by the time
        // we hit the destructure pattern.
        let door_seeds = state.server_door_seeds(server_tick);
        safety::cancel_safe_owner_weapons(&mut state, server_tick, map);
        step_server_decoys(&mut state, server_tick, map, &door_seeds);
        let projectile_bricks = projectile_brick_teams(&state, server_tick);
        let active_actor_runtimes = state
            .clients
            .iter()
            .filter_map(|(&player_id, client)| {
                client.joined.then_some((
                    crate::authority_identity::LifeIdentity::new(player_id, client.life_seq),
                    crate::authority_identity::PlayerRuntimeIdentity::new(
                        crate::authority_identity::PlayerSessionIdentity::new(
                            player_id,
                            client.session_id,
                        ),
                        crate::authority_identity::LifeIdentity::new(player_id, client.life_seq),
                    ),
                ))
            })
            .chain(
                state
                    .bots
                    .iter()
                    .filter(|bot| bot.alive())
                    .map(|bot| (bot.life_identity(), bot.runtime_identity())),
            )
            .collect::<std::collections::HashMap<_, _>>();
        let projectile_presentations = state
            .active_projectiles
            .iter()
            .filter_map(|projectile| {
                projectile_presentation_identity(projectile)
                    .map(|identity| (projectile.id, identity))
            })
            .collect::<std::collections::HashMap<_, _>>();
        // P1: reuse `state.hull_snapshots_scratch` across ticks so the
        // per-hull history Vecs and the outer Vec keep their allocations.
        // Destructure to split-borrow `clients` (read),
        // `hull_snapshots_scratch` (write), and `active_projectiles`
        // (write) as disjoint fields — bare `state.field` access can't
        // cross the iter borrow, but pattern bindings can.
        let hits = {
            let crate::state::ServerState {
                clients,
                bots,
                hull_snapshots_scratch,
                active_projectiles,
                ..
            } = &mut *state;
            let mut idx = 0usize;
            for (&player_id, client) in clients.iter() {
                if !client.joined || !client.alive || client.player.ship >= 8 {
                    continue;
                }
                let Some(last_position) = client.last_position.as_deref() else {
                    continue;
                };
                let Some((px, py)) = crate::arena::large_position_coordinates(last_position) else {
                    continue;
                };
                if crate::zone::world_position_in_safe_tile_for_map(map, px, py) {
                    continue;
                }
                let radius_px = crate::arena_settings::arena_ship_radius_pixels(client.player.ship);
                if idx < hull_snapshots_scratch.len() {
                    let hull = &mut hull_snapshots_scratch[idx];
                    hull.player_id = player_id;
                    hull.life_seq = client.life_seq;
                    hull.x = px as f32;
                    hull.y = py as f32;
                    hull.frequency = client.player.frequency;
                    hull.radius_px = radius_px;
                    hull.motion_segment = client.actor_motion_history.current_segment();
                    hull.motion_samples.clear();
                    hull.motion_samples
                        .extend(client.actor_motion_history.current_samples());
                    hull.motion_source_kind =
                        Some(crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness);
                    hull.lag_compensation_ticks = client.lag_compensation_ticks;
                    hull.life_started_tick = client.life_started_tick;
                } else {
                    hull_snapshots_scratch.push(crate::authority_projectiles::PlayerHullSnapshot {
                        player_id,
                        life_seq: client.life_seq,
                        x: px as f32,
                        y: py as f32,
                        frequency: client.player.frequency,
                        radius_px,
                        motion_segment: client.actor_motion_history.current_segment(),
                        motion_samples: client.actor_motion_history.current_samples().collect(),
                        motion_source_kind: Some(
                            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
                        ),
                        lag_compensation_ticks: client.lag_compensation_ticks,
                        life_started_tick: client.life_started_tick,
                    });
                }
                idx += 1;
            }
            for bot in bots.iter().filter(|bot| bot.alive()) {
                let bot_x = bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16;
                let bot_y = bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16;
                if crate::zone::world_position_in_safe_tile_for_map(map, bot_x, bot_y) {
                    continue;
                }
                let radius_px = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship);
                if idx < hull_snapshots_scratch.len() {
                    let hull = &mut hull_snapshots_scratch[idx];
                    hull.player_id = bot.player.id;
                    hull.life_seq = bot.life_seq;
                    hull.x = bot.x;
                    hull.y = bot.y;
                    hull.frequency = bot.player.frequency;
                    hull.radius_px = radius_px;
                    hull.motion_segment = bot.actor_motion_history.current_segment();
                    hull.motion_samples.clear();
                    hull.motion_samples
                        .extend(bot.actor_motion_history.current_samples());
                    hull.motion_source_kind =
                        Some(crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation);
                    hull.lag_compensation_ticks = 0;
                    hull.life_started_tick = None;
                } else {
                    hull_snapshots_scratch.push(crate::authority_projectiles::PlayerHullSnapshot {
                        player_id: bot.player.id,
                        life_seq: bot.life_seq,
                        x: bot.x,
                        y: bot.y,
                        frequency: bot.player.frequency,
                        radius_px,
                        motion_segment: bot.actor_motion_history.current_segment(),
                        motion_samples: bot.actor_motion_history.current_samples().collect(),
                        motion_source_kind: Some(
                            crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation,
                        ),
                        lag_compensation_ticks: 0,
                        life_started_tick: None,
                    });
                }
                idx += 1;
            }
            hull_snapshots_scratch.truncate(idx);
            #[cfg(test)]
            projection_measurement::begin_step();
            active_projectiles.step_with_environment_transactional(
                server_tick,
                |projectile, position, dt| {
                    use crate::shared_player_movement::{
                        MovementVector, WormholeTileGrid, sample_wormhole_gravity_in_grid,
                    };
                    let Some(source) = projectile.presentation_source.filter(|_| {
                        crate::arena_settings::arena_gravity_bombs()
                            && matches!(
                                crate::continuum_weapon::continuum_weapon_type(
                                    projectile.weapon_data
                                ),
                                3 | 4
                            )
                    }) else {
                        return crate::gameplay_rules::RulesVector::new(0.0, 0.0);
                    };
                    let sample = sample_wormhole_gravity_in_grid(
                        MovementVector::new(position.x, position.y),
                        crate::arena_settings::arena_ship_wormhole_gravity(source.ship),
                        0.0,
                        dt,
                        TILE_SIZE_PX,
                        WormholeTileGrid {
                            tiles: &map.tiles,
                            width: MAP_TILES,
                            height: MAP_TILES,
                            wormhole_tile: 220,
                        },
                    );
                    crate::gameplay_rules::RulesVector::new(
                        sample.velocity_delta.x,
                        sample.velocity_delta.y,
                    )
                },
                |x, y| {
                    let tx = (x / TILE_SIZE_PX).floor() as i32;
                    let ty = (y / TILE_SIZE_PX).floor() as i32;
                    tx >= 0
                        && ty >= 0
                        && tx < MAP_TILES as i32
                        && ty < MAP_TILES as i32
                        && map.tiles[ty as usize * MAP_TILES + tx as usize] == 220
                },
                |frequency, x, y| {
                    projectile_tile_at_world_solid(
                        map,
                        &door_seeds,
                        &projectile_bricks,
                        frequency,
                        x,
                        y,
                    )
                },
                &*hull_snapshots_scratch,
                crate::arena_settings::arena_proximity_distance(),
                crate::arena_settings::arena_bomb_explode_delay_ticks(),
                crate::arena_settings::arena_bomb_explode_pixels(),
                |projectile, target, collision_tick| {
                    projectile_presentation_geometry(clients, projectile, target, collision_tick)
                },
                |hit| {
                    collision_harmful_readiness(
                        clients,
                        &active_actor_runtimes,
                        &projectile_presentations,
                        hit,
                        server_tick,
                    )
                    .may_advance_or_terminate()
                },
            )
        };
        if let Some((samples, suppressed)) = state.active_projectiles.drain_contact_diagnostics() {
            for (sample_index, sample) in samples.into_iter().enumerate() {
                let projectile = sample.projectile;
                let source = projectile.captured_source();
                let event = serde_json::json!({
                    "type": "server_projectile_near_hull",
                    "diagnostic_only": true,
                    "sample_policy": "first_pair_per_100_ticks_max_64",
                    "suppressed_attempts_since_last_batch": if sample_index == 0 { suppressed } else { 0 },
                    "server_session_id": crate::wire::server_session_id().to_string(),
                    "server_tick": sample.tick,
                    "projectile_id": projectile.id,
                    "shot_group_id": projectile.shot_group_id.to_string(),
                    "projectile_index": projectile.shot_index,
                    "owner_id": projectile.owner_id,
                    "owner_session_id": source.map(|source| source.source_session_id.to_string()),
                    "owner_life_seq": projectile.owner_life_seq,
                    "source_frequency": sample.source_frequency,
                    "source_ship": source.map(|source| source.ship),
                    "weapon_data": projectile.weapon_data,
                    "source_server_tick": projectile.source_server_tick,
                    "created_tick": projectile.created_tick,
                    "expires_at_tick": projectile.expiry_tick,
                    "projectile_x": projectile.pos_x,
                    "projectile_y": projectile.pos_y,
                    "velocity_x": projectile.motion_velocity().x,
                    "velocity_y": projectile.motion_velocity().y,
                    "target_id": sample.target.player_id,
                    "target_session_id": sample.target_session_id.to_string(),
                    "target_life_seq": sample.target.life_seq,
                    "target_frequency": sample.target_frequency,
                    "target_current_x": sample.target_x,
                    "target_current_y": sample.target_y,
                    "target_radius": sample.target_radius,
                    "collision_anchor_x": sample.selected.map(|anchor| anchor.x),
                    "collision_anchor_y": sample.selected.map(|anchor| anchor.y),
                    "requested_target_tick": sample.selected.map(|anchor| anchor.requested_tick),
                    "selected_target_tick": sample.selected.and_then(|anchor| anchor.selected_tick),
                    "history_source": sample.selected.map(|anchor| anchor.source.as_str()),
                    "collision_available": sample.selected.map(|anchor| anchor.collision_available),
                    "presentation_status": sample.presentation.map(|witness| witness.status.as_str()),
                    "render_tick": sample.presentation.map(|witness| witness.render_tick),
                    "reason": sample.reason.as_str(),
                });
                info!("zubspace_event {event}");
                state
                    .qa_traces
                    .record_for_player(sample.target.player_id, event);
            }
        }
        // Freeze every newly detected collision into a complete damage/splash
        // transaction before releasing the state lock. Preparation consumes
        // the registry's bounded FIFO prefix even when harmful readiness is
        // still closed, so no later motion can change splash membership or
        // damage anchors while application waits.
        crate::server_explosions::freeze_wall_explosions(&mut state);
        let collision_hits = state.active_projectiles.pending_hits_for_preparation();
        let mut invalid_projectile_ids = Vec::new();
        for pending_hit in &collision_hits {
            if state
                .pending_damage_transactions
                .contains_key(&pending_hit.projectile_id)
            {
                continue;
            }
            let mut geometry_metrics =
                crate::authority_projectiles::PresentationGeometryMetrics::default();
            let prepared =
                crate::server_damage::prepare_server_detected_damage_transaction_from_collision(
                    &state,
                    pending_hit,
                    &state.hull_snapshots_scratch,
                    &mut geometry_metrics,
                );
            // Count attempted splash lookups even when preparation fails closed.
            state
                .active_projectiles
                .record_presentation_geometry_metrics(geometry_metrics);
            match prepared {
                Some(transaction) => {
                    for witness in transaction.collision_damage_witnesses() {
                        let victim_hit = crate::authority_projectiles::ProjectileHit {
                            projectile_id: pending_hit.projectile_id,
                            owner_id: pending_hit.owner_id,
                            owner_life_seq: pending_hit.owner_life_seq,
                            victim_id: witness.target.player_id,
                            victim_life_seq: witness.target.life_seq,
                            weapon_data: pending_hit.weapon_data,
                            x: pending_hit.x,
                            y: pending_hit.y,
                            tick: pending_hit.tick,
                        };
                        let harmful_readiness = collision_harmful_readiness(
                            &state.clients,
                            &active_actor_runtimes,
                            &projectile_presentations,
                            &victim_hit,
                            pending_hit.tick,
                        );
                        let threat_contract = harmful_readiness.contract();
                        let projectile_contract = harmful_readiness.projectile_contract();
                        state.collision_witnesses_total =
                            state.collision_witnesses_total.saturating_add(1);
                        let (
                            target_current_x,
                            target_current_y,
                            collision_anchor_x,
                            collision_anchor_y,
                            collision_center_distance,
                            history_source,
                            contact_reason,
                        ) = if witness.primary {
                            (
                                witness.target_current_x,
                                witness.target_current_y,
                                witness.collision.target_anchor_x,
                                witness.collision.target_anchor_y,
                                witness.collision.center_distance,
                                witness.history_source.as_str(),
                                witness.collision.contact_reason.as_str(),
                            )
                        } else {
                            (
                                witness.target_current_x,
                                witness.target_current_y,
                                witness.damage_anchor_x,
                                witness.damage_anchor_y,
                                witness.damage_center_distance,
                                witness.history_source.as_str(),
                                "splash",
                            )
                        };
                        let presentation_geometry_event = {
                            let presentation_geometry = witness.target_presentation_geometry;
                            let presentation_geometry_anchor =
                                presentation_geometry.target_anchor_q8();
                            serde_json::json!({
                                "status": presentation_geometry.status.as_str(),
                                "accepted_timeline_generation": presentation_geometry.accepted_timeline_generation,
                                "effective_timeline_generation": presentation_geometry.effective_timeline_generation,
                                "render_tick": presentation_geometry.render_tick,
                                "applied_snapshot_id": presentation_geometry.applied_snapshot_id,
                                "lower_snapshot_id": presentation_geometry.lower_snapshot_id,
                                "upper_snapshot_id": presentation_geometry.upper_snapshot_id,
                                "target_x": presentation_geometry_anchor.map(|(x, _)| x as f32 / 256.0),
                                "target_y": presentation_geometry_anchor.map(|(_, y)| y as f32 / 256.0),
                                "authority_x": presentation_geometry.authority_x_q8 as f32 / 256.0,
                                "authority_y": presentation_geometry.authority_y_q8 as f32 / 256.0,
                                "authority_error": presentation_geometry.authority_error_q8 as f32 / 256.0,
                                "server_advanced_watermark": presentation_geometry.server_advanced_watermark,
                            })
                        };
                        let target_motion_event = witness.target_motion_segment.map(|segment| {
                            serde_json::json!({
                                "arena_stream_id": segment.arena_stream_id.to_string(),
                                "discontinuity_generation": segment.discontinuity_generation,
                            })
                        });
                        let harmful_eligibility_event = serde_json::json!({
                            "harmful": harmful_readiness.harmful(),
                            "reason": harmful_readiness.reason(),
                            "source_first_snapshot_id": threat_contract.map(|contract| contract.first_snapshot_id),
                            "source_first_emitted_tick": threat_contract.map(|contract| contract.first_emitted_tick),
                            "conservative_open_tick": threat_contract.map(|contract| contract.conservative_open_tick),
                            "hard_open_tick": threat_contract.map(|contract| contract.hard_open_tick),
                            "application_snapshot_id": threat_contract.and_then(|contract| contract.application_snapshot_id),
                            "application_observed_tick": threat_contract.and_then(|contract| contract.application_observed_tick),
                            "projectile_identity_kind": projectile_contract.map(|contract| if contract.identity.projectile_id == 0 { "shot_group" } else { "exact_projectile" }),
                            "projectile_queued_event_id": projectile_contract.map(|contract| contract.queued_event_id),
                            "projectile_queued_tick": projectile_contract.map(|contract| contract.queued_tick),
                            "projectile_first_emitted_event_id": projectile_contract.and_then(|contract| contract.first_emitted_tick.map(|_| contract.queued_event_id)),
                            "projectile_first_emitted_tick": projectile_contract.and_then(|contract| contract.first_emitted_tick),
                            "projectile_harmful_open_tick": projectile_contract.and_then(|contract| contract.harmful_open_tick),
                        });
                        let witness_event = serde_json::json!({
                            "type": "server_collision_witness",
                            "server_session_id": crate::wire::server_session_id().to_string(),
                            "projectile_id": witness.collision.projectile_id,
                            "shot_group_id": witness.collision.shot_group_id.to_string(),
                            "owner_id": witness.collision.owner.player_id,
                            "owner_life_seq": witness.collision.owner.life_seq,
                            "target_id": witness.target.player_id,
                            "target_life_seq": witness.target.life_seq,
                            "projectile_created_tick": witness.collision.projectile_created_tick,
                            "collision_tick": witness.collision.collision_tick,
                            "requested_target_tick": witness.requested_target_tick,
                            "selected_target_tick": witness.selected_target_tick,
                            "target_motion": target_motion_event,
                            "rewind_ticks": witness.collision.rewind_ticks,
                            "projectile_x": witness.collision.projectile_x,
                            "projectile_y": witness.collision.projectile_y,
                            "target_current_x": target_current_x,
                            "target_current_y": target_current_y,
                            "collision_anchor_x": collision_anchor_x,
                            "collision_anchor_y": collision_anchor_y,
                            "damage_anchor_x": witness.damage_anchor_x,
                            "damage_anchor_y": witness.damage_anchor_y,
                            "collision_center_distance": collision_center_distance,
                            "damage_center_distance": witness.damage_center_distance,
                            "target_radius": witness.collision.target_radius,
                            "proximity_trigger_radius": witness.collision.proximity_trigger_radius,
                            "explosion_radius": witness.collision.explosion_radius,
                            "unprotected_damage": witness.unprotected_damage,
                            "adjusted_damage": witness.adjusted_damage,
                            "history_source": history_source,
                            "contact_reason": contact_reason,
                            "presentation_geometry": presentation_geometry_event,
                            "harmful_eligibility": harmful_eligibility_event,
                        });
                        debug!("zubspace_event {witness_event}");
                        state.qa_traces.record_for_player(
                            witness.collision.owner.player_id,
                            witness_event.clone(),
                        );
                        if witness.target.player_id != witness.collision.owner.player_id {
                            state
                                .qa_traces
                                .record_for_player(witness.target.player_id, witness_event);
                        }
                    }
                    state
                        .pending_damage_transactions
                        .insert(pending_hit.projectile_id, transaction);
                }
                None => invalid_projectile_ids.push(pending_hit.projectile_id),
            }
        }
        for projectile_id in &invalid_projectile_ids {
            state.active_projectiles.resolve_hit(*projectile_id);
            state.pending_damage_transactions.remove(projectile_id);
        }
        let hits = hits
            .into_iter()
            .filter(|hit| !invalid_projectile_ids.contains(&hit.projectile_id))
            .filter(|hit| {
                state
                    .pending_damage_transactions
                    .get(&hit.projectile_id)
                    .is_some_and(|transaction| {
                        frozen_transaction_harmful_ready(&state, transaction, server_tick)
                    })
            })
            .collect::<Vec<_>>();
        for hit in &hits {
            info!(
                target: crate::log_targets::SERVER_TICK_COMBAT,
                projectile_id = hit.projectile_id,
                owner_id = hit.owner_id,
                victim_id = hit.victim_id,
                owner_life_seq = hit.owner_life_seq,
                victim_life_seq = hit.victim_life_seq,
                server_tick = hit.tick,
                weapon = hit.weapon_data,
                x = hit.x,
                y = hit.y,
                "server projectile hit"
            );
            debug!(
                "zubspace_event {{\"type\":\"server_projectile_hit\",\"projectile_id\":{},\"owner_id\":{},\"victim_id\":{},\"owner_life_seq\":{},\"victim_life_seq\":{},\"server_tick\":{},\"weapon\":{},\"x\":{},\"y\":{}}}",
                hit.projectile_id,
                hit.owner_id,
                hit.victim_id,
                hit.owner_life_seq,
                hit.victim_life_seq,
                hit.tick,
                hit.weapon_data,
                hit.x,
                hit.y
            );
            if state.qa_traces.is_recording() {
                state.qa_traces.record_for_all(serde_json::json!({
                    "type": "server_projectile_hit",
                    "projectileId": hit.projectile_id,
                    "ownerId": hit.owner_id,
                    "victimId": hit.victim_id,
                    "ownerLifeSeq": hit.owner_life_seq,
                    "victimLifeSeq": hit.victim_life_seq,
                    "serverTick": hit.tick,
                    "weapon": hit.weapon_data,
                    "x": hit.x,
                    "y": hit.y,
                }));
            }
        }
        state.expire_projectiles_older_than_by_weapon(
            server_tick,
            DEFAULT_PROJECTILE_RETENTION_TICKS,
            arena_mine_lifetime_ticks(),
        );
        let expired_projectiles = projectile_count.saturating_sub(state.active_projectiles.len());
        let expired_prize_claims =
            state.expire_prize_claims_older_than(server_tick, arena_prize_claim_retention_ticks());
        let projectile_retention_ticks = maximum_projectile_retention_ticks();
        for client in state.clients.values_mut() {
            client.clear_expired_life_start_gate(server_tick, projectile_retention_ticks);
        }
        (hits, expired_projectiles, expired_prize_claims)
    };
    // Drop the state lock before the damage/event transaction re-acquires it.
    // A collision remains frozen in the registry while recipient capacity is
    // full. Only a committed or permanently invalid transaction resolves the
    // projectile, so pressure cannot cause a phantom despawn.
    for hit in &hits {
        match crate::server_damage::try_apply_server_detected_hit_with_broadcast(state_arc, hit) {
            crate::server_damage::DetectedHitApplication::Applied
            | crate::server_damage::DetectedHitApplication::Invalid => {
                let mut state = state_arc
                    .lock()
                    .expect("server state should not be poisoned");
                state.active_projectiles.resolve_hit(hit.projectile_id);
                state.prune_retired_projectile_owners();
            }
            crate::server_damage::DetectedHitApplication::DeferredCapacity => {
                // Damage transactions are a single FIFO authority ledger.
                // A later collision must not derive an outcome from state
                // that assumes this predecessor committed. Leave this hit and
                // every later projectile frozen for the next bounded retry.
                break;
            }
        }
    }

    crate::server_explosions::commit_wall_explosions(state_arc, server_tick);

    // Phase 3e-1: emit per-recipient snapshot frames at SNAPSHOT_HZ
    // when the flag is on. Empty bodies for now — the wire shape is
    // fixed and 3e-2 onwards will fill in entity / event payloads
    // without changing the header.
    #[cfg(test)]
    let snapshot_publication_started = Instant::now();
    let snapshots_started_us = tick_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    crate::snapshot::emit_snapshots_for_tick_at_tick(state_arc, server_tick);
    let snapshots_finished_us = tick_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    #[cfg(test)]
    let snapshot_publication_work = snapshot_publication_started.elapsed();

    // Phase 4 anti-cheat instrumentation: every
    // `SECURITY_REJECT_SUMMARY_INTERVAL_TICKS` ticks, drain the
    // aggregated (category, reason) reject counter and emit one
    // summary line. Per-event log lines are still useful for
    // forensics; the summary lets operators alert on totals over
    // time without grepping a high-volume log. Cadence picked so
    // the summary at 40 Hz lands every 15 s — small enough to
    // catch a sudden spike, large enough not to clutter.
    if tick % SECURITY_REJECT_SUMMARY_INTERVAL_TICKS == 0 {
        let _emitted = crate::security::dump_security_reject_summary();
        // Phase 4 send pacing: emit aggregated drop counts on the
        // same cadence so operators see throttle pressure alongside
        // anti-cheat numbers. Sums across all clients; per-client
        // breakdown stays on the ClientHandle for forensic queries.
        let mut state = state.lock().expect("server state should not be poisoned");
        let mut total_packets = 0u64;
        let mut total_bytes = 0u64;
        for client in state.clients.values_mut() {
            if client.send_pacing.packets_dropped > 0 {
                total_packets += client.send_pacing.packets_dropped;
                total_bytes += client.send_pacing.bytes_dropped;
                client.send_pacing.packets_dropped = 0;
                client.send_pacing.bytes_dropped = 0;
            }
        }
        if total_packets > 0 {
            warn!(
                packets_dropped = total_packets,
                bytes_dropped = total_bytes,
                "server send pacing summary"
            );
        }
        // Phase 4 load-shedding: aggregated shed counts on the same
        // cadence. Zero in a healthy run; sustained non-zero means
        // the server is consistently spending > 60 % of its tick
        // budget — time to scale horizontally or trim AOI radii.
        let mid_skipped = state.load_shed_mid_skipped;
        let frame_skipped = state.load_shed_frame_skipped;
        state.load_shed_mid_skipped = 0;
        state.load_shed_frame_skipped = 0;
        if mid_skipped > 0 || frame_skipped > 0 {
            warn!(
                mid_skipped,
                frame_skipped,
                pressure = state.load_pressure_ratio(),
                "server load shed summary"
            );
        }
        // Bounded production heartbeat on the existing summary cadence.
        tracing::info!(
            event = "server_tick_health",
            sessions = state.clients.len(),
            tick_overruns_total = state.telemetry.tick_overruns,
            interval_jitter_us = state.tick_interval_jitter_ewma_us as u64,
            tick_wallclock_us = state.tick_wallclock_ewma_us as u64,
            movement_snapbacks = state.movement_snapbacks_since_summary,
            movement_snapbacks_total = state.movement_snapbacks_total,
            "server tick pacing summary"
        );
        state.movement_snapbacks_since_summary = 0;
        // Phase 5 simulated WebRTC loss summary. Drain the
        // per-window counters in `simulated_loss` and emit one line
        // when non-zero so the realised drop rate shows up in the
        // operator log alongside the other periodic summaries.
        let (loss_packets, loss_bytes) = state.simulated_tx_loss.drain_counts();
        if loss_packets > 0 {
            warn!(
                packets_dropped = loss_packets,
                bytes_dropped = loss_bytes,
                "server simulated tx-loss summary"
            );
        }
    }

    // Phase 4 load-shedding: feed this tick's wallclock into the
    // EWMA. Done as the very last step so we capture the full path,
    // including snapshot emission, position fan-out, etc.
    {
        let elapsed_us = tick_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        let mut state = state.lock().expect("server state should not be poisoned");
        state.record_tick_wallclock(elapsed_us);
        let phases = [
            bots_started_us,
            bots_finished_us.saturating_sub(bots_started_us),
            snapshots_started_us.saturating_sub(bots_finished_us),
            snapshots_finished_us.saturating_sub(snapshots_started_us),
            elapsed_us.saturating_sub(snapshots_finished_us),
        ];
        let budget_us = 1_000_000.0 / state.config.tick_rate_hz.max(1) as f64;
        if state
            .telemetry
            .record_tick_phases(phases, budget_us, Instant::now())
        {
            tracing::warn!(
                event = "server_tick_overrun_phases",
                server_tick,
                work_us = elapsed_us,
                budget_us,
                admission_us = phases[0],
                bots_us = phases[1],
                world_and_publish_us = phases[2],
                snapshots_us = phases[3],
                finish_us = phases[4],
                "exclusive tick wall phases include scheduling and lock waits"
            );
        }
    }

    ServerTickResult {
        #[cfg(test)]
        snapshot_publication_work,
        tick,
        brick_packets,
        hidden_prize_packets,
        expired_projectiles,
        expired_prize_claims,
    }
}

/// Anti-cheat instrumentation: tick interval between summary-log
/// emissions. 600 ticks = 10 s at the default 60 Hz tick rate; the
/// counter map drains and resets at each emission,
/// so a per-window spike is a single log line, not thousands.
const SECURITY_REJECT_SUMMARY_INTERVAL_TICKS: u64 = 600;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{
        disconnect_player, join_arena, large_position_from_client, register_connection,
    };
    use crate::config::ServerConfig;
    use crate::state::{
        SERVER_BOT_ID_BASE, ServerBrick, ServerHiddenPrize, ServerPrizeClaim, ServerPrizeReceipt,
        ServerProjectile,
    };
    use std::cell::Cell;
    use std::sync::mpsc;

    thread_local! {
        // Independent test runtimes do not share an input stream. Allocating
        // globally fabricates gaps when the native suite runs in parallel.
        static NEXT_TEST_CLIENT_POSITION_SEQUENCE: Cell<u16> = const { Cell::new(1) };
    }

    #[test]
    fn decoy_retained_after_death_steps_until_its_own_wrapping_deadline() {
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&shared, tx);
        join_arena(&shared, player, 0);
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(player).unwrap();
        let map = state.zone_map();
        let index = map.tiles.iter().position(|tile| *tile == 0).unwrap();
        let x = (index % MAP_TILES) as f32 * TILE_SIZE_PX + 8.0;
        let y = (index / MAP_TILES) as f32 * TILE_SIZE_PX + 8.0;
        let created = u32::MAX - 5;
        state.active_decoys.push(crate::state::ServerDecoy {
            presentation_id: 0,
            presentation_source: None,
            source_input_sequence: None,
            owner,
            source_weapon_data: 6,
            frequency: 0,
            ship: 0,
            position_x: x,
            position_y: y,
            velocity_x: 1.0,
            velocity_y: 0.0,
            orientation: 0.25,
            created_tick: created,
            last_step_tick: created,
            expires_at_tick: 5,
        });
        state
            .advance_client_life_after_damage_death(player, u32::MAX - 4)
            .unwrap();
        let doors = state.server_door_seeds(4);
        step_server_decoys(&mut state, 4, map, &doors);
        assert_eq!(state.active_decoys.len(), 1);
        let decoy = state.active_decoys[0];
        assert_eq!(decoy.owner, owner);
        assert_eq!(decoy.created_tick, created);
        assert_eq!(decoy.expires_at_tick, 5);
        assert!(decoy.position_x > x);
        assert_eq!(decoy.last_step_tick, 4);
        step_server_decoys(&mut state, 5, map, &doors);
        assert!(state.active_decoys.is_empty());
    }

    #[test]
    fn projectile_bricks_block_hostile_frequency_and_expire_at_the_deadline() {
        let mut state = ServerState::default();
        let map = state.zone_map();
        let index = map.tiles.iter().position(|tile| *tile == 0).unwrap();
        let tile = ((index % MAP_TILES) as u16, (index / MAP_TILES) as u16);
        let x = f32::from(tile.0) * TILE_SIZE_PX + 8.0;
        let y = f32::from(tile.1) * TILE_SIZE_PX + 8.0;
        let mut packet = vec![0; 16];
        packet[9..11].copy_from_slice(&7_u16.to_le_bytes());
        state.active_bricks.push(ServerBrick {
            tiles: vec![tile],
            packet,
            expires_at_tick: 100,
        });
        let bricks = projectile_brick_teams(&state, 99);
        assert!(!projectile_tile_at_world_solid(map, &[], &bricks, 7, x, y));
        assert!(projectile_tile_at_world_solid(map, &[], &bricks, 8, x, y));
        let expired = projectile_brick_teams(&state, 100);
        assert!(expired.is_empty());
        assert!(!projectile_tile_at_world_solid(map, &[], &expired, 8, x, y));
    }

    fn safe_tile_center(state: &ServerState) -> (u16, u16) {
        let index = state
            .zone_map()
            .tiles
            .iter()
            .position(|&tile| tile == crate::zone::SAFE_TILE_ID)
            .expect("checked-in arena has a safe tile");
        let tile_x = index % MAP_TILES;
        let tile_y = index / MAP_TILES;
        (
            ((tile_x as f32 + 0.5) * TILE_SIZE_PX) as u16,
            ((tile_y as f32 + 0.5) * TILE_SIZE_PX) as u16,
        )
    }

    fn prize_receipt(
        collectible_id: u32,
        x: u16,
        y: u16,
        prize_id: i16,
        claimed_at_tick: u32,
    ) -> ServerPrizeReceipt {
        ServerPrizeReceipt {
            claim: ServerPrizeClaim {
                collectible_id,
                x,
                y,
                prize_id,
            },
            winner: crate::authority_identity::PlayerRuntimeIdentity::new(
                crate::authority_identity::PlayerSessionIdentity::new(
                    0,
                    crate::authority_identity::PlayerSessionId::new(1),
                ),
                crate::authority_identity::LifeIdentity::new(0, 1),
            ),
            claimed_at_tick,
            authoritative_energy: 1234,
            prize_state_revision: 1,
            engine_shutdown_until_tick: None,
            engine_shutdown_application_required: false,
        }
    }

    fn open_bot_life_threat_window(
        state: &mut ServerState,
        recipient_id: u16,
        bot_id: u16,
        bot_life_seq: u32,
        server_tick: u32,
    ) {
        let source = state
            .active_actor_runtime_identity(bot_id)
            .filter(|runtime| runtime.life.life_seq == bot_life_seq)
            .expect("exact bot runtime");
        let client = state.clients.get_mut(&recipient_id).expect("joined client");
        client.threat_presentation.observe_emitted(
            source,
            0xb071,
            crate::continuum::continuum_tick_sub(
                server_tick,
                crate::threat_presentation::THREAT_HARD_OPEN_TICKS,
            ),
        );
    }

    fn open_active_projectile_threat_windows(
        state: &mut ServerState,
        recipient_id: u16,
        owner_id: u16,
        server_tick: u32,
    ) {
        let identities = state
            .active_projectiles
            .iter()
            .filter(|projectile| projectile.owner_id == owner_id)
            .filter_map(|projectile| {
                projectile_presentation_identity(projectile)
                    .map(|identity| (projectile.id, identity))
            })
            .collect::<Vec<_>>();
        let emitted_tick = crate::continuum::continuum_tick_sub(
            server_tick,
            crate::threat_presentation::PROJECTILE_PRESENTATION_LEAD_TICKS,
        );
        let client = state.clients.get_mut(&recipient_id).expect("joined client");
        for (event_id, identity) in identities {
            assert!(client.threat_presentation.observe_projectile_queued(
                identity,
                event_id,
                emitted_tick,
            ));
            assert!(client.threat_presentation.observe_projectile_emitted(
                identity,
                event_id,
                emitted_tick,
            ));
        }
    }

    #[test]
    fn selected_target_readiness_cannot_authorize_an_unready_human_bystander() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (source_tx, _source_rx) = mpsc::channel();
        let (selected_tx, _selected_rx) = mpsc::channel();
        let (bystander_tx, _bystander_rx) = mpsc::channel();
        let source_id = register_connection(&state, source_tx);
        let selected_target_id = register_connection(&state, selected_tx);
        let actual_bystander_id = register_connection(&state, bystander_tx);
        for player_id in [source_id, selected_target_id, actual_bystander_id] {
            join_arena(&state, player_id, 0);
        }

        let mut state = state.lock().expect("state");
        let source = state
            .active_player_runtime_identity(source_id)
            .expect("source runtime");
        let selected_life = state.clients[&selected_target_id].life_seq;
        let bystander_life = state.clients[&actual_bystander_id].life_seq;
        let emitted_tick = 10_000;
        let selected_contract = state
            .clients
            .get_mut(&selected_target_id)
            .expect("selected target")
            .threat_presentation
            .observe_emitted(source, 71, emitted_tick);
        let active_actor_runtimes = state
            .clients
            .iter()
            .map(|(&player_id, client)| {
                let life = crate::authority_identity::LifeIdentity::new(player_id, client.life_seq);
                (
                    life,
                    crate::authority_identity::PlayerRuntimeIdentity::new(
                        crate::authority_identity::PlayerSessionIdentity::new(
                            player_id,
                            client.session_id,
                        ),
                        life,
                    ),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        let projectile_presentations = std::collections::HashMap::new();
        let selected_hit = crate::authority_projectiles::ProjectileHit {
            projectile_id: 41,
            owner_id: source_id,
            owner_life_seq: source.life.life_seq,
            victim_id: selected_target_id,
            victim_life_seq: selected_life,
            weapon_data: 2,
            x: 100,
            y: 100,
            tick: selected_contract.hard_open_tick,
        };
        assert!(
            collision_harmful_readiness(
                &state.clients,
                &active_actor_runtimes,
                &projectile_presentations,
                &selected_hit,
                selected_contract.hard_open_tick,
            )
            .harmful(),
            "the originally selected recipient has reached its own server deadline"
        );

        let bystander_hit = crate::authority_projectiles::ProjectileHit {
            projectile_id: 41,
            victim_id: actual_bystander_id,
            victim_life_seq: bystander_life,
            ..selected_hit
        };
        let bystander_decision = collision_harmful_readiness(
            &state.clients,
            &active_actor_runtimes,
            &projectile_presentations,
            &bystander_hit,
            selected_contract.hard_open_tick,
        );
        assert!(!bystander_decision.harmful());
        assert_eq!(
            bystander_decision.reason(),
            "source_life_not_emitted",
            "one recipient's readiness must never authorize the actual bystander victim"
        );

        let bystander_contract = state
            .clients
            .get_mut(&actual_bystander_id)
            .expect("bystander")
            .threat_presentation
            .observe_emitted(source, 72, emitted_tick + 10);
        assert!(
            collision_harmful_readiness(
                &state.clients,
                &active_actor_runtimes,
                &projectile_presentations,
                &bystander_hit,
                bystander_contract.hard_open_tick,
            )
            .harmful(),
            "withholding all receipts still opens the actual victim at its own bounded deadline"
        );
    }

    #[test]
    fn exact_projectile_state_can_replace_missing_source_sprite_but_group_cannot() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (source_tx, _source_rx) = mpsc::channel();
        let (victim_tx, _victim_rx) = mpsc::channel();
        let source_id = register_connection(&state, source_tx);
        let victim_id = register_connection(&state, victim_tx);
        join_arena(&state, source_id, 0);
        join_arena(&state, victim_id, 0);

        let mut state = state.lock().expect("state");
        let source = state
            .active_player_runtime_identity(source_id)
            .expect("source runtime");
        let victim_life = state.clients[&victim_id].life_seq;
        let exact =
            crate::threat_presentation::ProjectilePresentationIdentity::projectile(source, 41, 99)
                .expect("exact projectile");
        let mut projectile_presentations = std::collections::HashMap::new();
        projectile_presentations.insert(99, exact);
        let active_actor_runtimes = [(source.life, source)].into_iter().collect();
        let hit = crate::authority_projectiles::ProjectileHit {
            projectile_id: 99,
            owner_id: source_id,
            owner_life_seq: source.life.life_seq,
            victim_id,
            victim_life_seq: victim_life,
            weapon_data: 2,
            x: 100,
            y: 100,
            tick: 128,
        };

        let victim = state.clients.get_mut(&victim_id).expect("victim");
        assert!(
            victim
                .threat_presentation
                .observe_projectile_queued(exact, 70, 99)
        );
        assert!(
            victim
                .threat_presentation
                .observe_projectile_emitted(exact, 70, 100)
        );
        let exact_decision = collision_harmful_readiness(
            &state.clients,
            &active_actor_runtimes,
            &projectile_presentations,
            &hit,
            128,
        );
        assert!(exact_decision.harmful());
        assert_eq!(exact_decision.reason(), "projectile_lead_elapsed");

        let victim = state.clients.get_mut(&victim_id).expect("victim");
        victim.threat_presentation.clear();
        let group = exact.group();
        assert!(
            victim
                .threat_presentation
                .observe_projectile_queued(group, 71, 99)
        );
        assert!(
            victim
                .threat_presentation
                .observe_projectile_emitted(group, 71, 100)
        );
        let group_without_source = collision_harmful_readiness(
            &state.clients,
            &active_actor_runtimes,
            &projectile_presentations,
            &hit,
            150,
        );
        assert!(!group_without_source.harmful());
        assert_eq!(group_without_source.reason(), "source_life_not_emitted");

        state
            .clients
            .get_mut(&victim_id)
            .expect("victim")
            .threat_presentation
            .observe_emitted(source, 72, 100);
        assert!(
            collision_harmful_readiness(
                &state.clients,
                &active_actor_runtimes,
                &projectile_presentations,
                &hit,
                150,
            )
            .harmful()
        );
    }

    #[test]
    fn server_tick_interval_uses_configured_tick_rate() {
        assert_eq!(server_tick_interval(20), Duration::from_millis(50));
        assert_eq!(server_tick_interval(0), Duration::from_secs(1));
    }

    fn retired_human_projectile_fixture() -> (
        Arc<Mutex<ServerState>>,
        ServerProjectile,
        crate::authority_projectiles::PlayerHullSnapshot,
    ) {
        use crate::combat_timeline::*;
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let owner_id = register_connection(&shared, tx.clone());
        let target_id = register_connection(&shared, tx);
        join_arena(&shared, owner_id, 0);
        join_arena(&shared, target_id, 0);
        let mut state = shared.lock().unwrap();
        configure_damage_test_client(&mut state, owner_id, 100, 1000, 700, 1, 1700);
        configure_damage_test_client(&mut state, target_id, 100, 500, 700, 2, 1700);
        // This fixture uses a deterministic logical clock rather than the
        // wall-clock join tick; the target must already exist at collision.
        state.clients.get_mut(&target_id).unwrap().life_started_tick = Some(90);
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        let target = state.active_player_runtime_identity(target_id).unwrap();
        let stream = state.clients[&owner_id].arena_stream_id;
        let segment = state.clients[&target_id]
            .actor_motion_history
            .current_segment()
            .unwrap();
        let frame = |observer, id, tick, x| ObserverPresentationFrameV1 {
            observer,
            arena_stream_id: stream,
            snapshot_id: id,
            server_tick: tick,
            actors: vec![ObserverPresentationSampleV1 {
                sample_time: PresentationSampleTime::from_tick(tick),
                actor: target,
                motion_segment: segment,
                x_q8: x * 256,
                y_q8: 700 * 256,
                velocity_x_q8: 0,
                velocity_y_q8: 0,
                orientation_u16: 0,
                snap_discontinuity: false,
            }],
        };
        let client = state.clients.get_mut(&owner_id).unwrap();
        client
            .observer_combat_timeline
            .begin_scope(owner, stream, ObserverNetworkProfile::CleanLan, 700, 100, 1)
            .unwrap();
        client
            .observer_presentation_history
            .begin_scope(owner, stream);
        assert_eq!(
            client
                .observer_presentation_history
                .observe_emitted(frame(owner, 1, 100, 500)),
            ObserverPresentationAppendDisposition::Applied
        );
        let receipt = CombatViewReceiptV1 {
            observer_life_seq: owner.life.life_seq,
            input_sequence: 1,
            timeline_generation: 700,
            latest_game_applied_snapshot_id: 1,
            flags: 0,
        };
        client
            .observer_combat_timeline
            .verify_receipt(owner, 107, receipt, &client.observer_presentation_history)
            .unwrap();
        let binding = client
            .observer_combat_timeline
            .bind_projectile(owner, 1, 107)
            .unwrap();
        let mut projectile =
            damage_test_projectile(owner_id, owner.life.life_seq, 91, 107, 2, 600, 700);
        projectile.timestamp = 107;
        projectile.expiry_tick = Some(500);
        projectile.combat_timeline = Some(binding);
        projectile.presentation_source = Some(crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: owner_id,
            source_session_id: owner.session.session_id.get(),
            source_life_seq: owner.life.life_seq,
            shot_group_id: projectile.shot_group_id,
            x: 600.0,
            y: 700.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            frequency: 1,
            ship: 0,
            weapon_data: 2,
        });
        state.record_projectile(projectile);
        // Match production human death: life advances before old-source
        // retirement is registered. The observer context must be captured
        // before the advance clears the old verified receipt.
        state.advance_client_life_at_tick(owner_id, 110).unwrap();
        state.retire_projectile_owner_after_damage_death(owner, 110);
        let successor = state.active_player_runtime_identity(owner_id).unwrap();
        assert_eq!(
            state.clients[&owner_id].retired_projectile_observers.len(),
            1
        );
        assert_eq!(
            state.clients[&owner_id].observer_presentation_history.len(),
            1
        );
        let client = state.clients.get_mut(&owner_id).unwrap();
        assert_eq!(
            client.observer_combat_timeline.verify_receipt(
                successor,
                112,
                CombatViewReceiptV1 {
                    observer_life_seq: successor.life.life_seq,
                    ..receipt
                },
                &client.observer_presentation_history
            ),
            Err(ObserverTimelineReceiptRejection::AppliedSnapshotFromOtherRuntime)
        );
        assert_eq!(
            client
                .observer_presentation_history
                .observe_emitted(frame(successor, 2, 120, 600)),
            ObserverPresentationAppendDisposition::Applied
        );
        let target_client = state.clients.get_mut(&target_id).unwrap();
        append_stationary_test_motion(target_client, 120, 600, 700);
        let hull = crate::authority_projectiles::PlayerHullSnapshot {
            player_id: target_id,
            life_seq: target.life.life_seq,
            x: 600.0,
            y: 700.0,
            frequency: 2,
            radius_px: 14.0,
            motion_segment: Some(segment),
            motion_samples: target_client
                .actor_motion_history
                .current_samples()
                .collect(),
            motion_source_kind: Some(ActorMotionSourceKind::AcceptedHumanWitness),
            lag_compensation_ticks: 0,
            life_started_tick: None,
        };
        drop(state);
        (shared, projectile, hull)
    }

    #[test]
    fn retired_human_projectile_observer_survives_respawn_without_successor_receipts() {
        use crate::combat_timeline::*;
        let (shared, projectile, hull) = retired_human_projectile_fixture();
        let mut state = shared.lock().unwrap();
        let owner_id = projectile.owner_id;
        let stream = state.clients[&owner_id].arena_stream_id;
        let witness = projectile_presentation_geometry(&state.clients, &projectile, &hull, 127);
        assert!(
            matches!(witness.status, ProjectilePresentationStatusV1::Resolved(_)),
            "{witness:?}"
        );
        assert_eq!(witness.target_anchor_q8(), Some((600 * 256, 700 * 256)));
        assert!(witness.server_advanced_watermark);
        assert_eq!(witness.applied_snapshot_id, 2);
        assert_eq!(
            state.clients[&owner_id]
                .observer_combat_timeline
                .decision_for_sequence(1),
            None
        );
        for tick in 128..132 {
            state.advance_client_life_at_tick(owner_id, tick).unwrap();
        }
        assert_eq!(
            state.clients[&owner_id].retired_projectile_observers.len(),
            1
        );
        assert!(matches!(
            projectile_presentation_geometry(&state.clients, &projectile, &hull, 127).status,
            ProjectilePresentationStatusV1::Resolved(_)
        ));
        let client = state.clients.get_mut(&owner_id).unwrap();
        client.arena_stream_id = stream + 1;
        assert!(matches!(
            projectile_presentation_geometry(&state.clients, &projectile, &hull, 127).status,
            ProjectilePresentationStatusV1::QueryUnavailable(
                ProjectilePresentationQueryRejection::WrongRuntime
            )
        ));
        state.clients.get_mut(&owner_id).unwrap().arena_stream_id = stream;
        state.active_projectiles.step(500, |_, _| false, &[]);
        state.prune_retired_projectile_owners();
        assert!(
            state.clients[&owner_id]
                .retired_projectile_observers
                .is_empty()
        );
        assert!(matches!(
            projectile_presentation_geometry(&state.clients, &projectile, &hull, 127).status,
            ProjectilePresentationStatusV1::QueryUnavailable(
                ProjectilePresentationQueryRejection::WrongRuntime
            )
        ));
    }

    #[test]
    fn retired_own_explosive_uses_successor_local_hull_without_rebinding_source_life() {
        use crate::combat_timeline::*;
        let (shared, mut projectile, mut hull) = retired_human_projectile_fixture();
        let mut state = shared.lock().unwrap();
        let binding = projectile.combat_timeline.unwrap();
        let owner_id = projectile.owner_id;
        hull.player_id = owner_id;
        hull.life_seq = state.clients[&owner_id].life_seq;
        hull.life_started_tick = Some(110);
        hull.x = 1000.0;
        projectile.weapon_data = 3;
        let witness = projectile_presentation_geometry(&state.clients, &projectile, &hull, 127);
        assert!(matches!(
            witness.status,
            ProjectilePresentationStatusV1::LocalSelf
        ));
        assert_eq!(witness.target_anchor_q8(), Some((1000 * 256, 700 * 256)));
        assert_eq!(
            witness.accepted_timeline_generation,
            binding.accepted_timeline_generation
        );
        assert_ne!(hull.life_seq, binding.owner.life.life_seq);
        assert_eq!(projectile.combat_timeline, Some(binding));

        let hit = crate::authority_projectiles::ProjectileHit {
            projectile_id: projectile.id,
            owner_id,
            owner_life_seq: projectile.owner_life_seq,
            victim_id: owner_id,
            victim_life_seq: hull.life_seq,
            weapon_data: 3,
            x: 1000,
            y: 700,
            tick: 127,
        };
        let identities = [(
            projectile.id,
            projectile_presentation_identity(&projectile).unwrap(),
        )]
        .into_iter()
        .collect();
        let active = std::collections::HashMap::new();
        assert_eq!(
            collision_harmful_readiness(&state.clients, &active, &identities, &hit, 127).reason(),
            "self_hit"
        );
        state.clients.get_mut(&owner_id).unwrap().session_id =
            crate::authority_identity::PlayerSessionId::new(
                binding.owner.session.session_id.get() + 1,
            );
        assert!(matches!(
            projectile_presentation_geometry(&state.clients, &projectile, &hull, 127).status,
            ProjectilePresentationStatusV1::QueryUnavailable(
                ProjectilePresentationQueryRejection::WrongRuntime
            )
        ));
        assert!(
            !collision_harmful_readiness(&state.clients, &active, &identities, &hit, 127).harmful()
        );
    }

    #[test]
    fn retired_human_projectile_observer_is_revoked_on_disconnect_and_session_reuse() {
        use crate::combat_timeline::*;
        let (shared, projectile, hull) = retired_human_projectile_fixture();
        let mut state = shared.lock().unwrap();
        let owner_id = projectile.owner_id;
        let original_session = state.clients[&owner_id].session_id;
        state.clients.get_mut(&owner_id).unwrap().session_id =
            crate::authority_identity::PlayerSessionId::new(original_session.get() + 1);
        assert!(matches!(
            projectile_presentation_geometry(&state.clients, &projectile, &hull, 127).status,
            ProjectilePresentationStatusV1::QueryUnavailable(
                ProjectilePresentationQueryRejection::WrongRuntime
            )
        ));
        state.clients.get_mut(&owner_id).unwrap().session_id = original_session;
        assert!(state.remove_client(owner_id).is_some());
        assert!(state.active_projectiles.is_empty());
        assert!(state.retired_projectile_owners.is_empty());
        assert!(matches!(
            projectile_presentation_geometry(&state.clients, &projectile, &hull, 127).status,
            ProjectilePresentationStatusV1::QueryUnavailable(
                ProjectilePresentationQueryRejection::WrongRuntime
            )
        ));
    }

    #[test]
    fn retired_human_projectile_deals_damage_through_the_production_tick_after_respawn() {
        let (state, projectile, hull) = retired_human_projectile_fixture();
        let before = {
            let mut guard = state.lock().unwrap();
            let position =
                large_position_from_client(hull.player_id, &position_packet_at(120, 600, 700, 0))
                    .unwrap();
            guard.set_client_position(hull.player_id, position);
            let identity = projectile_presentation_identity(&projectile).unwrap();
            let ledger = &mut guard
                .clients
                .get_mut(&hull.player_id)
                .unwrap()
                .threat_presentation;
            assert!(ledger.observe_projectile_queued(identity, 700, 107));
            assert!(ledger.observe_projectile_emitted(identity, 700, 107));
            guard.clients[&hull.player_id].server_energy
        };
        run_server_tick_at_tick(&state, Instant::now(), 134);
        run_server_tick_at_tick(&state, Instant::now(), 135);
        let mut guard = state.lock().unwrap();
        assert!(
            guard.clients[&hull.player_id].server_energy < before,
            "projectile={:?} witness={:?} metrics={:?}",
            guard.active_projectiles.projectile(projectile.id),
            guard.active_projectiles.collision_witness(projectile.id),
            guard.active_projectiles.presentation_geometry_metrics()
        );
        assert!(guard.active_projectiles.projectile(projectile.id).is_none());
        guard.prune_retired_projectile_owners();
        assert!(
            guard.clients[&projectile.owner_id]
                .retired_projectile_observers
                .is_empty()
        );
    }

    #[test]
    fn retired_human_projectile_kill_credits_session_totals_without_changing_successor_bounty() {
        let (shared, projectile, hull) = retired_human_projectile_fixture();
        let successor_bounty = {
            let mut state = shared.lock().unwrap();
            let position =
                large_position_from_client(hull.player_id, &position_packet_at(120, 600, 700, 0))
                    .unwrap();
            state.set_client_position(hull.player_id, position);
            let victim = state.clients.get_mut(&hull.player_id).unwrap();
            victim.server_energy = 1;
            victim.server_bounty = 47;
            let identity = projectile_presentation_identity(&projectile).unwrap();
            assert!(
                victim
                    .threat_presentation
                    .observe_projectile_queued(identity, 700, 107)
            );
            assert!(
                victim
                    .threat_presentation
                    .observe_projectile_emitted(identity, 700, 107)
            );
            assert_ne!(
                state.clients[&projectile.owner_id].life_seq,
                projectile.owner_life_seq
            );
            state.clients[&projectile.owner_id].server_bounty
        };
        run_server_tick_at_tick(&shared, Instant::now(), 134);
        run_server_tick_at_tick(&shared, Instant::now(), 135);
        let state = shared.lock().unwrap();
        assert_eq!(
            state.clients[&hull.player_id].player.losses, 1,
            "the retired shot must kill"
        );
        let owner = &state.clients[&projectile.owner_id];
        assert_eq!(
            (owner.player.wins, owner.player.kill_points),
            (1, 47),
            "the same session's earlier-life kill belongs in cumulative totals"
        );
        assert_eq!(
            owner.server_bounty, successor_bounty,
            "old flight cannot transfer bounty to a different life"
        );
    }

    #[test]
    fn retired_human_projectile_frozen_hit_survives_pressure_and_expiry_without_retargeting() {
        for commit_tick in [140, 600] {
            let (shared, projectile, hull) = retired_human_projectile_fixture();
            let (energy_before, sequence_before) = {
                let mut state = shared.lock().unwrap();
                let position = large_position_from_client(
                    hull.player_id,
                    &position_packet_at(120, 600, 700, 0),
                )
                .unwrap();
                state.set_client_position(hull.player_id, position);
                let identity = projectile_presentation_identity(&projectile).unwrap();
                let target = state.clients.get_mut(&hull.player_id).unwrap();
                assert!(
                    target
                        .threat_presentation
                        .observe_projectile_queued(identity, 700, 107)
                );
                assert!(
                    target
                        .threat_presentation
                        .observe_projectile_emitted(identity, 700, 107)
                );
                let template = crate::snapshot::PendingRealtimeEventEnvelope {
                    flight_transition: false,
                    recipient: state
                        .active_player_runtime_identity(hull.player_id)
                        .unwrap(),
                    event_id: 701,
                    server_tick: 120,
                    payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                        projectile.presentation_source.unwrap(),
                    ),
                };
                state
                    .clients
                    .get_mut(&hull.player_id)
                    .unwrap()
                    .pending_realtime_events
                    .queue
                    .extend(std::iter::repeat_n(
                        template,
                        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                    ));
                (
                    state.clients[&hull.player_id].server_energy,
                    state.next_damage_seq,
                )
            };
            run_server_tick_at_tick(&shared, Instant::now(), 134);
            run_server_tick_at_tick(&shared, Instant::now(), 135);
            {
                let mut state = shared.lock().unwrap();
                assert_eq!(state.clients[&hull.player_id].server_energy, energy_before);
                assert_eq!(state.next_damage_seq, sequence_before);
                assert_eq!(state.active_projectiles.pending_hit_count(), 1);
                assert_eq!(state.pending_damage_transactions.len(), 1);
                let frozen = state
                    .active_projectiles
                    .collision_witness(projectile.id)
                    .unwrap();
                assert_eq!(frozen.target, hull.identity());
                // More source lives and entirely different target motion must
                // not rebind or recompute a previously frozen contact.
                for tick in 136..139 {
                    state
                        .advance_client_life_after_damage_death(projectile.owner_id, tick)
                        .unwrap();
                }
                assert_eq!(
                    state.clients[&projectile.owner_id]
                        .retired_projectile_observers
                        .len(),
                    1
                );
                let moved = large_position_from_client(
                    hull.player_id,
                    &position_packet_at(139, 15000, 15000, 0),
                )
                .unwrap();
                state.set_client_position(hull.player_id, moved);
                let target = state.clients.get_mut(&hull.player_id).unwrap();
                target.position_history.clear();
                target.pending_realtime_events.queue.clear();
                assert_eq!(
                    state.active_projectiles.collision_witness(projectile.id),
                    Some(frozen)
                );
            }
            run_server_tick_at_tick(&shared, Instant::now(), commit_tick);
            {
                let state = shared.lock().unwrap();
                assert!(state.clients[&hull.player_id].server_energy < energy_before);
                assert_eq!(state.next_damage_seq, sequence_before.wrapping_add(1));
                assert!(state.active_projectiles.is_empty());
                assert!(state.pending_damage_transactions.is_empty());
                assert!(
                    state.clients[&projectile.owner_id]
                        .retired_projectile_observers
                        .is_empty()
                );
            }
            run_server_tick_at_tick(&shared, Instant::now(), commit_tick + 1);
            assert_eq!(
                shared.lock().unwrap().next_damage_seq,
                sequence_before.wrapping_add(1)
            );
        }
        // Without a frozen contact, the same source's expired flight must
        // disappear normally rather than gain a new hit during queue recovery.
        let (shared, projectile, hull) = retired_human_projectile_fixture();
        let before = shared.lock().unwrap().clients[&hull.player_id].server_energy;
        run_server_tick_at_tick(&shared, Instant::now(), 134);
        run_server_tick_at_tick(&shared, Instant::now(), 500);
        let state = shared.lock().unwrap();
        assert_eq!(state.clients[&hull.player_id].server_energy, before);
        assert!(state.active_projectiles.is_empty());
        assert!(state.pending_damage_transactions.is_empty());
        assert!(
            state.clients[&projectile.owner_id]
                .retired_projectile_observers
                .is_empty()
        );
    }

    #[test]
    fn surviving_human_projectile_damages_a_successor_victim_through_current_life_projection() {
        use crate::combat_timeline::*;
        let (shared, projectile, hull) = retired_human_projectile_fixture();
        let (target_life, before) = {
            let mut state = shared.lock().unwrap();
            let target_id = hull.player_id;
            let target_life = state
                .advance_client_life_after_damage_death(target_id, 121)
                .unwrap();
            configure_damage_test_client(&mut state, target_id, 140, 600, 700, 2, 1700);
            let target = state.active_player_runtime_identity(target_id).unwrap();
            let segment = state.clients[&target_id]
                .actor_motion_history
                .current_segment()
                .unwrap();
            let observer = state
                .active_player_runtime_identity(projectile.owner_id)
                .unwrap();
            let stream = state.clients[&projectile.owner_id].arena_stream_id;
            assert_eq!(
                state
                    .clients
                    .get_mut(&projectile.owner_id)
                    .unwrap()
                    .observer_presentation_history
                    .observe_emitted(ObserverPresentationFrameV1 {
                        observer,
                        arena_stream_id: stream,
                        snapshot_id: 3,
                        server_tick: 140,
                        actors: vec![ObserverPresentationSampleV1 {
                            sample_time: PresentationSampleTime::from_tick(140),
                            actor: target,
                            motion_segment: segment,
                            x_q8: 600 * 256,
                            y_q8: 700 * 256,
                            velocity_x_q8: 0,
                            velocity_y_q8: 0,
                            orientation_u16: 0,
                            snap_discontinuity: false,
                        }],
                    }),
                ObserverPresentationAppendDisposition::Applied
            );
            // Preserve the full required projectile lead for this new recipient life.
            open_active_projectile_threat_windows(&mut state, target_id, projectile.owner_id, 147);
            assert_ne!(target_life.life_seq, hull.life_seq);
            (target_life, state.clients[&target_id].server_energy)
        };
        run_server_tick_at_tick(&shared, Instant::now(), 147);
        run_server_tick_at_tick(&shared, Instant::now(), 148);
        let state = shared.lock().unwrap();
        assert_eq!(
            state.clients[&hull.player_id].life_seq,
            target_life.life_seq
        );
        assert!(
            state.clients[&hull.player_id].server_energy < before,
            "old shot must damage the newly presented current-life target; record={:?} witness={:?}",
            state.active_projectiles.projectile(projectile.id),
            state.active_projectiles.collision_witness(projectile.id)
        );
        assert!(state.active_projectiles.projectile(projectile.id).is_none());
    }

    #[test]
    fn server_default_door_state_anchors_at_timestamp_zero() {
        // Phase 2 polish (door-aware projectile collision). The
        // server's canonical door state must seed at SERVER_DOOR_RNG_SEED
        // anchored at timestamp 0, not the all-zeros that a naive
        // derive(Default) would produce. Verifies the explicit Default
        // impl on `ClientDoorState`.
        let state = ServerState::default();
        assert_eq!(
            state.server_door_state.rng_seed,
            crate::doors::SERVER_DOOR_RNG_SEED as i32
        );
        assert_eq!(state.server_door_state.timestamp, 0);
        assert_eq!(
            state.server_door_challenge_state, state.server_door_state,
            "the client challenge and authoritative simulation must begin from one arena anchor"
        );
        assert!(state.server_door_seed_cache.is_none());
    }

    #[test]
    fn server_door_seeds_returns_nonempty_after_first_call() {
        // The seed list should always include at least the current
        // update-count's seed; with the default anchor (timestamp=0)
        // and any current_tick, that means at least one entry.
        let mut state = ServerState::default();
        let seeds = state.server_door_seeds(10_000);
        assert_eq!(seeds.len(), 1, "world collision must use one exact phase");
        let seeds2 = state.server_door_seeds(10_000);
        assert_eq!(seeds2, seeds);
    }

    #[test]
    fn projectile_door_seed_is_identical_before_and_after_rebase_including_wrap() {
        const DOOR_STATE_REBASE_TICKS: u32 = 24 * 60 * 60 * 100;

        for anchor_tick in [0, 0x7fff_ff00] {
            let rebase_tick =
                crate::continuum::continuum_tick_add(anchor_tick, DOOR_STATE_REBASE_TICKS);
            let mut state = ServerState::default();
            state.server_door_state = crate::doors::ClientDoorState::for_challenge(anchor_tick);
            state.server_door_seed_cache = None;

            // Projectile collision obtains this exact phase, then the helper
            // rebases the internal state. A second query at the same tick must
            // still see precisely the same phase.
            let before_rebase = state.server_door_seeds(rebase_tick);
            let after_rebase = state.server_door_seeds(rebase_tick);
            assert_eq!(after_rebase, before_rebase);
            assert_eq!(before_rebase.len(), 1);

            let next_tick = crate::continuum::continuum_tick_add(rebase_tick, 1);
            let expected_next = crate::doors::client_door_simulation_seed_at_tick(
                Some(crate::doors::ClientDoorState::for_challenge(anchor_tick)),
                next_tick,
            )
            .into_iter()
            .collect::<Vec<_>>();
            assert_eq!(state.server_door_seeds(next_tick), expected_next);
        }
    }

    #[test]
    fn projectile_collision_never_unions_adjacent_door_grace_phases() {
        use crate::zone::position_blocking_tile_for_door_seeds;

        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let mut validation_cache = None;
        for tick in 1..100_000 {
            let candidates = crate::doors::client_door_validation_seeds_at_tick(
                Some(door_state),
                &mut validation_cache,
                tick,
            );
            let exact = crate::doors::client_door_simulation_seed_at_tick(Some(door_state), tick)
                .expect("canonical challenge has an exact phase");
            for tile_id in 162..=169 {
                if position_blocking_tile_for_door_seeds(tile_id, &[exact])
                    && !position_blocking_tile_for_door_seeds(tile_id, &candidates)
                {
                    let mut state = ServerState::default();
                    assert_eq!(state.server_door_seeds(tick), vec![exact]);
                    assert!(position_blocking_tile_for_door_seeds(
                        tile_id,
                        &state.server_door_seeds(tick)
                    ));
                    return;
                }
            }
        }
        panic!("fixture did not find a closed current door with an open grace neighbor");
    }

    #[test]
    fn tile_at_world_solid_classifies_open_door_as_passable_with_seeded_state() {
        // The predicate the projectile step consumes. A door tile id
        // with a seed that maps it to OPEN_DOOR_TILE_ID via
        // `position_blocking_tile_for_door_seeds` must return false
        // (passable); with an empty seed slice (fail-safe) it must
        // return true (blocking).
        use crate::zone::position_blocking_tile_for_door_seeds;
        // tile_id 162 is DOOR_FIRST_TILE_ID; bit 0 of seed controls
        // whether it's open. seed=0xFE has bit 0 clear ⇒ tile 162 maps
        // to OPEN. seed=0xFF has bit 0 set ⇒ tile 162 stays closed.
        assert!(
            !position_blocking_tile_for_door_seeds(162, &[0xFE]),
            "door tile 162 must be passable when seed bit 0 is clear"
        );
        assert!(
            position_blocking_tile_for_door_seeds(162, &[0xFF]),
            "door tile 162 must block when seed bit 0 is set"
        );
        assert!(
            position_blocking_tile_for_door_seeds(162, &[]),
            "empty seed slice must fail safe to blocking"
        );
    }

    #[test]
    fn server_tick_expires_projectiles_and_prize_claims() {
        let now = Instant::now();
        let server_tick = 10_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        {
            let mut state = state.lock().expect("state");
            state.record_projectile(projectile(7, server_tick - 2_000));
            state.record_projectile(projectile(7, server_tick - 100));
            state
                .claimed_prizes
                .insert(1, prize_receipt(1, 500, 700, 15, server_tick - 9_000));
            state
                .claimed_prizes
                .insert(2, prize_receipt(2, 520, 700, 16, server_tick - 100));
        }

        let result = run_server_tick_at_tick(&state, now, server_tick);

        assert_eq!(result.tick, 1);
        assert_eq!(result.expired_projectiles, 1);
        assert_eq!(result.expired_prize_claims, 1);
        let state = state.lock().expect("state");
        assert_eq!(state.arena_tick, 1);
        assert_eq!(state.last_tick_at, Some(now));
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(state.claimed_prizes.len(), 1);
    }

    #[test]
    fn server_tick_retains_mines_for_arena_mine_lifetime() {
        let now = Instant::now();
        let server_tick = 10_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        {
            let mut state = state.lock().expect("state");
            state.record_projectile(projectile(7, server_tick - 2_000));
            let mut old_mine = projectile(8, server_tick - 2_000);
            old_mine.weapon_data = 3 | (1 << 15);
            state.record_projectile(old_mine);
            let mut expired_mine = projectile(
                9,
                crate::continuum::continuum_tick_sub(
                    server_tick,
                    arena_mine_lifetime_ticks() + 100,
                ),
            );
            expired_mine.weapon_data = 3 | (1 << 15);
            state.record_projectile(expired_mine);
        }

        let result = run_server_tick_at_tick(&state, now, server_tick);

        assert_eq!(result.expired_projectiles, 2);
        let state = state.lock().expect("state");
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(
            state
                .active_projectiles
                .last()
                .expect("recent mine should remain")
                .weapon_data,
            3 | (1 << 15)
        );
    }

    #[test]
    fn production_projectile_hit_waits_for_capacity_then_applies_exactly_once() {
        for safe_after_contact in [false, true] {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (shooter_tx, _shooter_rx) = mpsc::channel();
            let (victim_tx, _victim_rx) = mpsc::channel();
            let shooter = register_connection(&state, shooter_tx);
            let victim = register_connection(&state, victim_tx);
            join_arena(&state, shooter, 0);
            join_arena(&state, victim, 0);
            let created_tick = 10_000;
            {
                let mut locked = state.lock().expect("state");
                for (player_id, frequency) in [(shooter, 1), (victim, 2)] {
                    configure_damage_test_client(
                        &mut locked,
                        player_id,
                        created_tick,
                        500,
                        700,
                        frequency,
                        1_000,
                    );
                }
                let mut projectile = projectile(shooter, created_tick);
                projectile.owner_life_seq = locked.clients[&shooter].life_seq;
                projectile.timestamp = created_tick;
                if safe_after_contact {
                    let owner = locked.active_player_runtime_identity(shooter).unwrap();
                    projectile.presentation_source =
                        Some(crate::realtime_event::RealtimeWeaponEvent {
                            source_player_id: shooter,
                            source_session_id: owner.session.session_id.get(),
                            source_life_seq: owner.life.life_seq,
                            shot_group_id: projectile.shot_group_id,
                            x: f32::from(projectile.x),
                            y: f32::from(projectile.y),
                            velocity_x: 0.0,
                            velocity_y: 0.0,
                            orientation: 0.0,
                            frequency: 1,
                            ship: 0,
                            weapon_data: projectile.weapon_data,
                        });
                }
                locked.record_projectile(projectile);
                if safe_after_contact {
                    open_active_projectile_threat_windows(
                        &mut locked,
                        victim,
                        shooter,
                        created_tick + 1,
                    );
                }
            }

            // Establish the registry's integration anchor; the first step never
            // guesses an elapsed duration or collides.
            run_server_tick_at_tick(&state, Instant::now(), created_tick);
            let (energy_before, damage_seq_before) = {
                let mut locked = state.lock().expect("state");
                let victim_runtime = locked
                    .active_player_runtime_identity(victim)
                    .expect("victim runtime");
                let shooter_runtime = locked
                    .active_player_runtime_identity(shooter)
                    .expect("shooter runtime");
                let template = crate::snapshot::PendingRealtimeEventEnvelope {
                    flight_transition: false,
                    recipient: victim_runtime,
                    event_id: 1,
                    server_tick: created_tick,
                    payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                        crate::realtime_event::RealtimeWeaponEvent {
                            source_player_id: shooter,
                            source_session_id: shooter_runtime.session.session_id.get(),
                            source_life_seq: shooter_runtime.life.life_seq,
                            shot_group_id: 1,
                            x: 500.0,
                            y: 700.0,
                            velocity_x: 0.0,
                            velocity_y: 0.0,
                            orientation: 0.0,
                            frequency: 1,
                            ship: 0,
                            weapon_data: 2,
                        },
                    ),
                };
                locked
                    .clients
                    .get_mut(&victim)
                    .expect("victim")
                    .pending_realtime_events
                    .queue
                    .extend(std::iter::repeat_n(
                        template,
                        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                    ));
                (
                    locked.clients[&victim].server_energy,
                    locked.next_damage_seq,
                )
            };

            run_server_tick_at_tick(&state, Instant::now(), created_tick + 1);
            {
                let locked = state.lock().expect("state");
                assert_eq!(locked.clients[&victim].server_energy, energy_before);
                assert_eq!(locked.next_damage_seq, damage_seq_before);
                assert_eq!(locked.active_projectiles.len(), 1);
                assert_eq!(locked.active_projectiles.pending_hit_count(), 1);
                assert_eq!(locked.pending_damage_transactions.len(), 1);
                assert_eq!(locked.realtime_event_actions_rejected, 0);
                assert_eq!(locked.realtime_event_actions_deferred, 1);
            }
            {
                let mut locked = state.lock().expect("state");
                let moved = large_position_from_client(
                    victim,
                    &position_packet_at(created_tick + 2, 15_000, 15_000, 0),
                )
                .expect("moved victim position");
                locked.set_client_position(victim, moved);
                let victim_client = locked.clients.get_mut(&victim).expect("victim");
                victim_client.position_history.clear();
                victim_client.pending_realtime_events.queue.clear();
            }

            if safe_after_contact {
                let mut locked = state.lock().unwrap();
                let (x, y) = safe_tile_center(&locked);
                let position = large_position_from_client(
                    shooter,
                    &position_packet_at(created_tick + 2, x, y, 0),
                )
                .unwrap();
                locked.set_client_position(shooter, position);
            }
            run_server_tick_at_tick(&state, Instant::now(), created_tick + 2);
            let energy_after = {
                let locked = state.lock().expect("state");
                assert!(locked.clients[&victim].server_energy < energy_before);
                assert_eq!(locked.next_damage_seq, damage_seq_before.wrapping_add(1));
                assert!(locked.active_projectiles.is_empty());
                assert_eq!(locked.active_projectiles.pending_hit_count(), 0);
                assert!(locked.pending_damage_transactions.is_empty());
                if safe_after_contact {
                    let source = locked.active_player_session_identity(shooter).unwrap();
                    assert!(
                        locked
                            .owner_weapon_cancellation_watermarks
                            .contains_key(&source)
                    );
                }
                locked.clients[&victim].server_energy
            };

            run_server_tick_at_tick(&state, Instant::now(), created_tick + 3);
            let locked = state.lock().expect("state");
            assert_eq!(locked.clients[&victim].server_energy, energy_after);
            assert_eq!(locked.next_damage_seq, damage_seq_before.wrapping_add(1));
        }
    }

    fn configure_damage_test_client(
        state: &mut ServerState,
        player_id: u16,
        tick: u32,
        x: u16,
        y: u16,
        frequency: u16,
        energy: u16,
    ) {
        let position = large_position_from_client(player_id, &position_packet_at(tick, x, y, 0))
            .expect("fixture position");
        state.set_client_position(player_id, position);
        let client = state.clients.get_mut(&player_id).expect("client");
        client.alive = true;
        client.player.frequency = frequency;
        client.server_energy = energy;
        client.server_max_energy = energy.max(1_700);
        client.server_energy_remainder = 0.0;
        client.server_recharge_per_second = 0.0;
        client.last_energy_recharge_tick = Some(tick);
        // Most server-tick tests exercise FIFO damage independently of the
        // damage-prize stream. Dedicated prize-loss fixtures opt back in with
        // a positive bounty and an explicit deterministic seed.
        client.server_bounty = 0;
        let rewind_tick =
            crate::continuum::continuum_tick_sub(tick, u32::from(client.lag_compensation_ticks));
        append_stationary_test_motion(client, rewind_tick, x, y);
        append_stationary_test_motion(client, tick, x, y);

        // These fixtures isolate collision/damage FIFO behavior from the
        // source-presentation policy. Model a source life that was emitted
        // more than one hard deadline ago to every joined recipient. Focused
        // threat tests deliberately do not use this adapter.
        let emitted_tick = crate::continuum::continuum_tick_sub(
            tick,
            crate::threat_presentation::THREAT_HARD_OPEN_TICKS,
        );
        let runtimes = state
            .clients
            .iter()
            .filter_map(|(&id, client)| {
                client
                    .joined
                    .then_some(crate::authority_identity::PlayerRuntimeIdentity::new(
                        crate::authority_identity::PlayerSessionIdentity::new(
                            id,
                            client.session_id,
                        ),
                        crate::authority_identity::LifeIdentity::new(id, client.life_seq),
                    ))
            })
            .collect::<Vec<_>>();
        for recipient in state.clients.values_mut().filter(|client| client.joined) {
            for source in runtimes.iter().copied() {
                recipient
                    .threat_presentation
                    .observe_emitted(source, 0xdafe, emitted_tick);
            }
        }
    }

    fn append_stationary_test_motion(
        client: &mut crate::state::ClientHandle,
        tick: u32,
        x: u16,
        y: u16,
    ) {
        let witness = crate::state::CanonicalPositionWitness {
            sequence: 0,
            life_seq: client.life_seq,
            state: crate::state::ServerSimMovement {
                position_x: f32::from(x),
                position_y: f32::from(y),
                orientation: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                energy: client.server_energy as f32,
                max_energy: client.server_max_energy as f32,
            },
            wire_x: x,
            wire_y: y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 0,
            door_phase_tick: tick,
        };
        client.append_actor_motion_witness(tick, witness).unwrap();
    }

    fn damage_test_projectile(
        owner_id: u16,
        owner_life_seq: u32,
        id: u32,
        tick: u32,
        weapon_data: u16,
        x: u16,
        y: u16,
    ) -> ServerProjectile {
        let mut projectile = projectile(owner_id, tick);
        projectile.id = id;
        projectile.shot_group_id = u64::from(id);
        projectile.owner_life_seq = owner_life_seq;
        projectile.weapon_data = weapon_data;
        projectile.x = x;
        projectile.y = y;
        projectile.pos_x = f32::from(x);
        projectile.pos_y = f32::from(y);
        projectile.timestamp = tick;
        projectile
    }

    fn fill_damage_event_capacity(
        state: &mut ServerState,
        recipient_id: u16,
        source_id: u16,
        count: usize,
        tick: u32,
    ) {
        let recipient = state
            .active_player_runtime_identity(recipient_id)
            .expect("recipient runtime");
        let source = state
            .active_player_runtime_identity(source_id)
            .expect("source runtime");
        let template = crate::snapshot::PendingRealtimeEventEnvelope {
            flight_transition: false,
            recipient,
            event_id: 1,
            server_tick: tick,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                crate::realtime_event::RealtimeWeaponEvent {
                    source_player_id: source_id,
                    source_session_id: source.session.session_id.get(),
                    source_life_seq: source.life.life_seq,
                    shot_group_id: 1,
                    x: 500.0,
                    y: 700.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    orientation: 0.0,
                    frequency: 1,
                    ship: 0,
                    weapon_data: 2,
                },
            ),
        };
        state
            .clients
            .get_mut(&recipient_id)
            .expect("recipient")
            .pending_realtime_events
            .queue
            .extend(std::iter::repeat_n(template, count));
    }

    fn two_projectile_damage_fixture(
        victim_energy: u16,
        first_weapon: u16,
        second_weapon: u16,
    ) -> (Arc<Mutex<ServerState>>, u16, u16, u32) {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (shooter_tx, _shooter_rx) = mpsc::channel();
        let (victim_tx, _victim_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let victim = register_connection(&state, victim_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, victim, 0);
        let tick = 20_000;
        {
            let mut locked = state.lock().expect("state");
            configure_damage_test_client(&mut locked, shooter, tick, 400, 700, 1, 1_700);
            configure_damage_test_client(&mut locked, victim, tick, 500, 700, 2, victim_energy);
            let shooter_life = locked.clients[&shooter].life_seq;
            locked.record_projectile(damage_test_projectile(
                shooter,
                shooter_life,
                101,
                tick,
                first_weapon,
                500,
                700,
            ));
            locked.record_projectile(damage_test_projectile(
                shooter,
                shooter_life,
                102,
                tick,
                second_weapon,
                500,
                700,
            ));
        }
        (state, shooter, victim, tick)
    }

    #[test]
    fn production_tick_sequences_equal_and_unequal_same_tick_damage_without_energy_rollback() {
        let equal_damage = crate::authority_damage::authoritative_unprotected_direct_damage(
            1,
            crate::arena_settings::arena_weapon_damage_settings(),
            crate::arena_settings::arena_exact_damage(),
        )
        .expect("exact bullet damage");
        let upgraded_weapon = 1 | (3 << 5);
        let upgraded_damage = crate::authority_damage::authoritative_unprotected_direct_damage(
            upgraded_weapon,
            crate::arena_settings::arena_weapon_damage_settings(),
            crate::arena_settings::arena_exact_damage(),
        )
        .expect("exact upgraded bullet damage");
        assert!(upgraded_damage > equal_damage);

        let (state, _shooter, victim, tick) = two_projectile_damage_fixture(1_700, 1, 1);
        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        {
            let locked = state.lock().expect("state");
            assert_eq!(
                locked.clients[&victim].server_energy,
                1_700_u16.saturating_sub(equal_damage.saturating_mul(2))
            );
            assert_eq!(locked.next_damage_seq, 2);
            assert!(locked.pending_damage_transactions.is_empty());
        }

        let (state, _shooter, victim, tick) =
            two_projectile_damage_fixture(1_700, 1, upgraded_weapon);
        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.clients[&victim].server_energy,
            1_700_u16
                .saturating_sub(equal_damage)
                .saturating_sub(upgraded_damage)
        );
        assert!(locked.clients[&victim].server_energy < 1_700 - equal_damage);
        assert_eq!(locked.next_damage_seq, 2);
    }

    #[test]
    fn production_tick_combined_lethality_is_fifo_and_emitted_once_for_two_sources() {
        let damage = crate::authority_damage::authoritative_unprotected_direct_damage(
            1,
            crate::arena_settings::arena_weapon_damage_settings(),
            crate::arena_settings::arena_exact_damage(),
        )
        .expect("exact bullet damage");
        let initial_energy = damage.saturating_mul(2).saturating_sub(1);
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (first_tx, _first_rx) = mpsc::channel();
        let (second_tx, _second_rx) = mpsc::channel();
        let (victim_tx, _victim_rx) = mpsc::channel();
        let first = register_connection(&state, first_tx);
        let second = register_connection(&state, second_tx);
        let victim = register_connection(&state, victim_tx);
        for player_id in [first, second, victim] {
            join_arena(&state, player_id, 0);
        }
        let tick = 30_000;
        {
            let mut locked = state.lock().expect("state");
            configure_damage_test_client(&mut locked, first, tick, 400, 690, 1, 1_700);
            configure_damage_test_client(&mut locked, second, tick, 400, 710, 3, 1_700);
            configure_damage_test_client(&mut locked, victim, tick, 500, 700, 2, initial_energy);
            for (id, owner) in [(201, first), (202, second)] {
                let life = locked.clients[&owner].life_seq;
                locked
                    .record_projectile(damage_test_projectile(owner, life, id, tick, 1, 500, 700));
            }
        }

        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        {
            let locked = state.lock().expect("state");
            let victim_client = &locked.clients[&victim];
            assert_eq!(victim_client.server_energy, 0);
            assert!(!victim_client.alive);
            assert_eq!(locked.next_damage_seq, 2);
            // Public damage facts carry victim/shooter runtime identities in
            // the payload. The dying victim and a surviving AOI observer both
            // retain the two facts in FIFO order across the victim cutover.
            let damage_facts = locked.clients[&first]
                .pending_realtime_events
                .queue
                .iter()
                .filter_map(|event| match event.payload {
                    crate::realtime_event::RealtimeEventPayload::WatchDamage(damage) => {
                        Some(damage)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(damage_facts.len(), 2);
            assert_eq!(damage_facts[0].shooter_player_id, first);
            assert_eq!(damage_facts[0].energy_before, initial_energy);
            assert_eq!(damage_facts[0].energy_after, initial_energy - damage);
            assert!(!damage_facts[0].lethal);
            assert_eq!(damage_facts[1].shooter_player_id, second);
            assert_eq!(damage_facts[1].energy_before, initial_energy - damage);
            assert_eq!(damage_facts[1].energy_after, 0);
            assert!(damage_facts[1].lethal);
            let victim_damage_facts = locked.clients[&victim]
                .pending_realtime_events
                .queue
                .iter()
                .filter_map(|event| match event.payload {
                    crate::realtime_event::RealtimeEventPayload::WatchDamage(damage) => {
                        Some(damage)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(victim_damage_facts, damage_facts);
            assert!(locked.pending_damage_transactions.is_empty());
            assert!(locked.active_projectiles.is_empty());
        }

        run_server_tick_at_tick(&state, Instant::now(), tick + 2);
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.next_damage_seq, 2,
            "retry cannot duplicate damage/death"
        );
        assert_eq!(locked.clients[&victim].server_energy, 0);
    }

    #[test]
    fn production_tick_capacity_pressure_preserves_first_and_second_hit_fifo_ledgers() {
        let damage = crate::authority_damage::authoritative_unprotected_direct_damage(
            1,
            crate::arena_settings::arena_weapon_damage_settings(),
            crate::arena_settings::arena_exact_damage(),
        )
        .expect("exact bullet damage");
        for initially_free_slots in [0, 1] {
            let (state, shooter, victim, tick) = two_projectile_damage_fixture(1_700, 1, 1);
            {
                let mut locked = state.lock().expect("state");
                fill_damage_event_capacity(
                    &mut locked,
                    victim,
                    shooter,
                    crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY - initially_free_slots,
                    tick,
                );
            }
            run_server_tick_at_tick(&state, Instant::now(), tick);
            run_server_tick_at_tick(&state, Instant::now(), tick + 1);
            {
                let mut locked = state.lock().expect("state");
                assert_eq!(
                    locked.clients[&victim].server_energy,
                    1_700 - damage.saturating_mul(initially_free_slots as u16)
                );
                assert_eq!(locked.next_damage_seq, initially_free_slots as u32);
                assert_eq!(
                    locked.pending_damage_transactions.len(),
                    2 - initially_free_slots
                );
                assert_eq!(
                    locked.active_projectiles.pending_hit_count(),
                    2 - initially_free_slots
                );
                locked
                    .clients
                    .get_mut(&victim)
                    .expect("victim")
                    .pending_realtime_events
                    .queue
                    .clear();
            }

            run_server_tick_at_tick(&state, Instant::now(), tick + 2);
            {
                let locked = state.lock().expect("state");
                assert_eq!(
                    locked.clients[&victim].server_energy,
                    1_700 - damage.saturating_mul(2)
                );
                assert_eq!(locked.next_damage_seq, 2);
                assert!(locked.pending_damage_transactions.is_empty());
                assert!(locked.active_projectiles.is_empty());
            }
            run_server_tick_at_tick(&state, Instant::now(), tick + 3);
            let locked = state.lock().expect("state");
            assert_eq!(
                locked.next_damage_seq, 2,
                "resolved retries stay idempotent"
            );
        }
    }

    #[test]
    fn production_tick_damage_loss_rng_and_mutable_state_follow_the_exact_fifo_predecessor() {
        let upgraded_weapon = 1 | (3 << 5);
        let damage = crate::authority_damage::authoritative_unprotected_direct_damage(
            upgraded_weapon,
            crate::arena_settings::arena_weapon_damage_settings(),
            crate::arena_settings::arena_exact_damage(),
        )
        .expect("exact upgraded bullet damage");
        let damage_factor = crate::arena_settings::arena_ship_damage_factor(2);
        let mut expected_seed = 3;
        let expected_loss = crate::authority_prizes::damage_prize_loss_id(
            &mut expected_seed,
            damage_factor,
            damage,
        )
        .expect("fixture seed selects the second hit's damage loss");

        for initially_free_slots in [0, 1] {
            let (state, shooter, victim, tick) =
                two_projectile_damage_fixture(1_700, upgraded_weapon, upgraded_weapon);
            let revision_before = {
                let mut locked = state.lock().expect("state");
                let client = locked.clients.get_mut(&victim).expect("victim");
                client.server_bounty = 1;
                let revision = client.server_prize_state_revision;
                locked.damage_prize_rng_seed = 3;
                fill_damage_event_capacity(
                    &mut locked,
                    victim,
                    shooter,
                    crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY - initially_free_slots,
                    tick,
                );
                revision
            };

            run_server_tick_at_tick(&state, Instant::now(), tick);
            run_server_tick_at_tick(&state, Instant::now(), tick + 1);
            {
                let mut locked = state.lock().expect("state");
                assert_eq!(
                    locked.clients[&victim].server_energy,
                    1_700 - damage.saturating_mul(initially_free_slots as u16)
                );
                assert_eq!(
                    locked.damage_prize_rng_seed, 3,
                    "a full-energy first hit is ineligible and a deferred second hit cannot commit RNG"
                );
                assert_eq!(locked.clients[&victim].last_damage_prize_loss_seq, None);
                locked
                    .clients
                    .get_mut(&victim)
                    .expect("victim")
                    .pending_realtime_events
                    .queue
                    .clear();
            }

            run_server_tick_at_tick(&state, Instant::now(), tick + 2);
            let locked = state.lock().expect("state");
            let client = &locked.clients[&victim];
            assert_eq!(client.server_energy, 1_700 - damage.saturating_mul(2));
            assert_eq!(client.last_damage_prize_loss_seq, Some(2));
            assert_eq!(client.last_damage_prize_loss_id, Some(expected_loss));
            assert_eq!(
                client.server_prize_state_revision,
                revision_before.wrapping_add(1).max(1)
            );
            assert_eq!(locked.damage_prize_rng_seed, expected_seed);
            assert_eq!(locked.next_damage_seq, 2);
            assert!(locked.pending_damage_transactions.is_empty());
            assert!(locked.active_projectiles.is_empty());
        }
    }

    #[test]
    fn production_tick_two_eligible_damage_losses_advance_one_fifo_rng_ledger() {
        let upgraded_weapon = 1 | (3 << 5);
        let damage = crate::authority_damage::authoritative_unprotected_direct_damage(
            upgraded_weapon,
            crate::arena_settings::arena_weapon_damage_settings(),
            crate::arena_settings::arena_exact_damage(),
        )
        .expect("exact upgraded bullet damage");
        let damage_factor = crate::arena_settings::arena_ship_damage_factor(2);
        let mut expected_seed = 120;
        let first_loss = crate::authority_prizes::damage_prize_loss_id(
            &mut expected_seed,
            damage_factor,
            damage,
        )
        .expect("fixture seed selects the first eligible damage loss");
        let second_loss = crate::authority_prizes::damage_prize_loss_id(
            &mut expected_seed,
            damage_factor,
            damage,
        )
        .expect("fixture seed selects the second eligible damage loss");
        assert_eq!((first_loss, second_loss), (23, 15));

        let (state, _shooter, victim, tick) =
            two_projectile_damage_fixture(1_600, upgraded_weapon, upgraded_weapon);
        let revision_before = {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&victim).expect("victim");
            client.server_bounty = 10;
            let revision = client.server_prize_state_revision;
            locked.damage_prize_rng_seed = 120;
            revision
        };

        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        let locked = state.lock().expect("state");
        let client = &locked.clients[&victim];
        assert_eq!(client.server_energy, 1_600 - damage.saturating_mul(2));
        assert_eq!(client.server_bounty, 10);
        assert_eq!(client.last_damage_prize_loss_seq, Some(2));
        assert_eq!(client.last_damage_prize_loss_id, Some(second_loss));
        assert_eq!(
            client.server_prize_state_revision,
            revision_before.wrapping_add(2).max(1)
        );
        assert_eq!(locked.damage_prize_rng_seed, expected_seed);
        assert_eq!(locked.next_damage_seq, 2);
        assert!(locked.pending_damage_transactions.is_empty());
        assert!(locked.active_projectiles.is_empty());
    }

    #[test]
    fn production_tick_cancels_all_same_life_fifo_damage_after_lifecycle_change() {
        let (state, shooter, victim, tick) = two_projectile_damage_fixture(1_700, 1, 1);
        {
            let mut locked = state.lock().expect("state");
            fill_damage_event_capacity(
                &mut locked,
                victim,
                shooter,
                crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                tick,
            );
        }
        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        {
            let mut locked = state.lock().expect("state");
            assert_eq!(locked.pending_damage_transactions.len(), 2);
            let victim_client = locked.clients.get_mut(&victim).expect("victim");
            victim_client.life_seq = victim_client.life_seq.wrapping_add(1).max(1);
            victim_client.pending_realtime_events.queue.clear();
            victim_client.server_energy = 1_111;
        }
        run_server_tick_at_tick(&state, Instant::now(), tick + 2);
        let locked = state.lock().expect("state");
        assert_eq!(locked.clients[&victim].server_energy, 1_111);
        assert_eq!(locked.next_damage_seq, 0);
        assert!(locked.pending_damage_transactions.is_empty());
        assert!(locked.active_projectiles.is_empty());
    }

    #[test]
    fn production_tick_overlapping_bomb_splash_accumulates_for_every_frozen_victim() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut registered = Vec::new();
        for _ in 0..4 {
            let (tx, _rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);
            registered.push(player_id);
        }
        let [shooter, primary, splash, far] = registered.as_slice() else {
            panic!("four clients")
        };
        let tick = 40_000;
        {
            let mut locked = state.lock().expect("state");
            configure_damage_test_client(&mut locked, *shooter, tick, 200, 700, 1, 1_700);
            configure_damage_test_client(&mut locked, *primary, tick, 500, 700, 2, 1_700);
            configure_damage_test_client(&mut locked, *splash, tick, 520, 700, 3, 1_700);
            configure_damage_test_client(&mut locked, *far, tick, 15_000, 15_000, 4, 1_700);
            let life = locked.clients[shooter].life_seq;
            for id in [301, 302] {
                let projectile = damage_test_projectile(*shooter, life, id, tick, 3, 500, 700);
                locked.record_projectile(projectile);
            }
        }

        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        let locked = state.lock().expect("state");
        assert!(locked.clients[primary].server_energy < 1_700);
        assert!(locked.clients[splash].server_energy < 1_700);
        assert_eq!(locked.clients[far].server_energy, 1_700);
        let primary_events = locked.clients[primary]
            .pending_realtime_events
            .queue
            .iter()
            .filter(|event| {
                matches!(
                    event.payload,
                    crate::realtime_event::RealtimeEventPayload::WatchDamage(damage)
                        if damage.victim_player_id == *primary
                )
            })
            .count();
        let splash_events = locked.clients[splash]
            .pending_realtime_events
            .queue
            .iter()
            .filter(|event| {
                matches!(
                    event.payload,
                    crate::realtime_event::RealtimeEventPayload::WatchDamage(damage)
                        if damage.victim_player_id == *splash
                )
            })
            .count();
        assert_eq!(primary_events, 2);
        assert_eq!(splash_events, 2);
        assert_eq!(locked.next_damage_seq, 4);
        assert!(locked.pending_damage_transactions.is_empty());
    }

    #[test]
    fn unready_splash_victim_delays_one_frozen_transaction_without_recomputing_geometry() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut registered = Vec::new();
        for _ in 0..3 {
            let (tx, _rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);
            registered.push(player_id);
        }
        let [shooter, primary, splash] = registered.as_slice() else {
            panic!("three clients")
        };
        let tick = 44_000;
        {
            let mut locked = state.lock().expect("state");
            configure_damage_test_client(&mut locked, *shooter, tick, 200, 700, 1, 1_700);
            configure_damage_test_client(&mut locked, *primary, tick, 500, 700, 2, 1_700);
            configure_damage_test_client(&mut locked, *splash, tick, 520, 700, 3, 1_700);
            locked
                .clients
                .get_mut(splash)
                .expect("splash")
                .threat_presentation
                .clear();
            let life = locked.clients[shooter].life_seq;
            let projectile = damage_test_projectile(*shooter, life, 350, tick, 3, 500, 700);
            locked.record_projectile(projectile);
        }

        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        {
            let mut locked = state.lock().expect("state");
            assert_eq!(locked.clients[primary].server_energy, 1_700);
            assert_eq!(locked.clients[splash].server_energy, 1_700);
            assert_eq!(locked.pending_damage_transactions.len(), 1);
            assert_eq!(locked.active_projectiles.pending_hit_count(), 1);

            // Move the splash victim far outside the explosion after the
            // collision. The eventual damage must retain the collision-time
            // membership and anchor rather than query this later pose.
            let moved = large_position_from_client(
                *splash,
                &position_packet_at(tick + 2, 15_000, 15_000, 0),
            )
            .expect("moved splash position");
            locked.set_client_position(*splash, moved);
            locked
                .clients
                .get_mut(splash)
                .expect("splash")
                .position_history
                .clear();

            let source = locked
                .active_player_runtime_identity(*shooter)
                .expect("source runtime");
            locked
                .clients
                .get_mut(splash)
                .expect("splash")
                .threat_presentation
                .observe_emitted(source, 81, tick + 2);
        }

        let hard_open_tick = tick + 2 + crate::threat_presentation::THREAT_HARD_OPEN_TICKS;
        for server_tick in tick + 2..hard_open_tick {
            run_server_tick_at_tick(&state, Instant::now(), server_tick);
        }
        {
            let locked = state.lock().expect("state");
            assert_eq!(locked.clients[primary].server_energy, 1_700);
            assert_eq!(locked.clients[splash].server_energy, 1_700);
            assert_eq!(locked.pending_damage_transactions.len(), 1);
        }

        run_server_tick_at_tick(&state, Instant::now(), hard_open_tick);
        let locked = state.lock().expect("state");
        assert!(locked.clients[primary].server_energy < 1_700);
        assert!(
            locked.clients[splash].server_energy < 1_700,
            "splash damage uses the immutable collision-time anchor"
        );
        assert!(locked.pending_damage_transactions.is_empty());
        assert!(locked.active_projectiles.is_empty());
    }

    #[test]
    fn production_tick_never_overtakes_more_than_128_old_pending_hits_with_new_lethal() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut registered = Vec::new();
        for _ in 0..4 {
            let (tx, _rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);
            registered.push(player_id);
        }
        let [old_owner, live_owner, old_victim, lethal_victim] = registered.as_slice() else {
            panic!("four clients")
        };
        let tick = 50_000;
        {
            let mut locked = state.lock().expect("state");
            configure_damage_test_client(&mut locked, *old_owner, tick, 400, 700, 1, 1_700);
            configure_damage_test_client(&mut locked, *live_owner, tick, 8_100, 8_200, 3, 1_700);
            configure_damage_test_client(&mut locked, *old_victim, tick, 500, 700, 2, u16::MAX);
            configure_damage_test_client(&mut locked, *lethal_victim, tick, 8_200, 8_200, 4, 1);
            let old_life = locked.clients[old_owner].life_seq;
            for ordinal in 0..crate::authority_projectiles::MAX_PENDING_HITS_PER_OWNER {
                locked.record_projectile(damage_test_projectile(
                    *old_owner,
                    old_life,
                    1_000 + ordinal as u32,
                    tick,
                    1,
                    500,
                    700,
                ));
            }
            let live_life = locked.clients[live_owner].life_seq;
            locked.record_projectile(damage_test_projectile(
                *live_owner,
                live_life,
                2_000,
                tick,
                1,
                500,
                700,
            ));
            fill_damage_event_capacity(
                &mut locked,
                *old_victim,
                *live_owner,
                crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                tick,
            );
        }

        run_server_tick_at_tick(&state, Instant::now(), tick);
        run_server_tick_at_tick(&state, Instant::now(), tick + 1);
        {
            let mut locked = state.lock().expect("state");
            assert_eq!(locked.active_projectiles.pending_hit_count(), 129);
            assert_eq!(locked.pending_damage_transactions.len(), 129);
            // Make the first 128 transactions permanently invalid without
            // removing them from the registry. The 129th remains valid but
            // capacity-deferred and therefore owns the FIFO head after those
            // cancellations.
            let client = locked.clients.get_mut(old_owner).expect("old owner");
            client.life_seq = client.life_seq.wrapping_add(1).max(1);
            let live_life = locked.clients[live_owner].life_seq;
            locked.record_projectile(damage_test_projectile(
                *live_owner,
                live_life,
                3_000,
                tick + 1,
                1,
                8_200,
                8_200,
            ));
        }

        run_server_tick_at_tick(&state, Instant::now(), tick + 2);
        {
            let mut locked = state.lock().expect("state");
            assert_eq!(
                locked.clients[lethal_victim].server_energy, 1,
                "new lethal collision cannot overtake omitted old authority"
            );
            assert!(locked.clients[lethal_victim].alive);
            assert_eq!(locked.active_projectiles.pending_hit_count(), 2);
            assert!(locked.active_projectiles.contains_pending_hit(2_000));
            assert!(locked.active_projectiles.contains_pending_hit(3_000));
            assert_eq!(locked.pending_damage_transactions.len(), 2);
            locked
                .clients
                .get_mut(old_victim)
                .expect("old victim")
                .pending_realtime_events
                .queue
                .clear();
        }

        run_server_tick_at_tick(&state, Instant::now(), tick + 3);
        let locked = state.lock().expect("state");
        assert_eq!(locked.next_damage_seq, 2);
        assert!(locked.clients[old_victim].server_energy < u16::MAX);
        assert_eq!(locked.clients[lethal_victim].server_energy, 0);
        assert!(!locked.clients[lethal_victim].alive);
        assert!(locked.pending_damage_transactions.is_empty());
        assert!(locked.active_projectiles.is_empty());
    }

    #[test]
    fn server_tick_advances_monotonic_server_tick_counter() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let first = Instant::now();
        let second = first + Duration::from_millis(50);

        let first_result = run_server_tick(&state, first);
        let second_result = run_server_tick(&state, second);

        assert_eq!(first_result.tick, 1);
        assert_eq!(second_result.tick, 2);
        let state = state.lock().expect("state");
        assert_eq!(state.arena_tick, 2);
        assert_eq!(state.last_tick_at, Some(second));
    }

    #[test]
    fn server_tick_returns_brick_refresh_packets_when_bricks_expire() {
        let now = Instant::now();
        let server_tick = 10_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        {
            let mut state = state.lock().expect("state");
            state.active_bricks.push(ServerBrick {
                tiles: vec![(10, 11)],
                packet: vec![0x1c, 10, 11],
                expires_at_tick: server_tick,
            });
            state.active_bricks.push(ServerBrick {
                tiles: vec![(12, 13)],
                packet: vec![0x1c, 12, 13],
                expires_at_tick: server_tick + 100,
            });
        }

        let result = run_server_tick_at_tick(&state, now, server_tick);

        assert_eq!(result.brick_packets.len(), 2);
        assert_eq!(result.brick_packets[1], vec![0x1c, 12, 13]);
        assert_eq!(
            state
                .lock()
                .expect("state")
                .active_bricks
                .iter()
                .map(|brick| brick.tiles.clone())
                .collect::<Vec<_>>(),
            vec![vec![(12, 13)]]
        );
    }

    #[test]
    fn server_tick_coalesces_movement_inputs_but_preserves_weapon_inputs() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        {
            let mut state = state.lock().expect("state");
            for timestamp in 0_u32..10 {
                state.queue_position_input(player_id, position_packet(timestamp));
            }
            assert_eq!(
                state
                    .pending_position_inputs
                    .queued_count_for_player(player_id),
                1
            );
            state.queue_position_input(player_id, position_packet_with_weapon(10, 2));
            state.queue_position_input(player_id, position_packet(11));
            state.queue_position_input(player_id, position_packet(12));
            let queue = state
                .pending_position_inputs
                .queued_for_player(player_id)
                .expect("player input queue should exist");
            assert_eq!(queue.len(), 2);
            assert_eq!(u32::from_le_bytes(queue[0][2..6].try_into().unwrap()), 10);
            assert_eq!(u16::from_le_bytes(queue[0][20..22].try_into().unwrap()), 2);
            assert_eq!(u32::from_le_bytes(queue[1][2..6].try_into().unwrap()), 12);
            assert_eq!(u16::from_le_bytes(queue[1][20..22].try_into().unwrap()), 0);
        }

        let result = run_server_tick(&state, Instant::now());

        assert!(result.brick_packets.is_empty());
        assert!(
            state
                .lock()
                .expect("state")
                .pending_position_inputs
                .is_empty()
        );
    }

    #[test]
    fn production_one_send_weapon_is_durably_retried_after_event_pressure_drains() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (shooter_tx, shooter_rx) = mpsc::channel();
        let (observer_tx, observer_rx) = mpsc::channel();
        let shooter = register_connection(&state, shooter_tx);
        let observer = register_connection(&state, observer_tx);
        join_arena(&state, shooter, 0);
        join_arena(&state, observer, 0);
        let (spawn_x, spawn_y) = {
            let locked = state.lock().expect("state");
            crate::zone::server_spawn_coordinates(locked.zone_map())
        };
        let mut weapon_packet = position_packet_at(0, spawn_x, spawn_y, 2);
        weapon_packet[18..20].copy_from_slice(
            &state.lock().expect("state").clients[&shooter]
                .server_energy
                .to_le_bytes(),
        );
        let weapon_sequence =
            crate::continuum_position::parse_client_position_snapshot(&weapon_packet)
                .and_then(|snapshot| snapshot.client_sequence)
                .expect("current-protocol weapon sequence");
        let (energy_before, runtime) = {
            let mut locked = state.lock().expect("state");
            let base_sequence = weapon_sequence.wrapping_sub(1);
            let base_state = crate::state::ServerSimMovement {
                position_x: spawn_x as f32,
                position_y: spawn_y as f32,
                orientation: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                energy: locked.clients[&shooter].server_energy as f32,
                max_energy: locked.clients[&shooter].server_max_energy as f32,
            };
            let mut base_packet = position_packet_at(0, spawn_x, spawn_y, 0);
            base_packet[18..20]
                .copy_from_slice(&locked.clients[&shooter].server_energy.to_le_bytes());
            let base_position = large_position_from_client(shooter, &base_packet)
                .expect("established position fixture");
            locked.set_client_position(shooter, base_position);
            {
                let client = locked.clients.get_mut(&shooter).expect("shooter");
                client.alive = true;
                client.respawn_at_tick = None;
                client.last_position_sequence = Some(base_sequence);
                client.last_input_seq = Some(base_sequence);
                client.last_input_record = Some(crate::state::ClientInputObservation {
                    bits: 0,
                    direction: 0,
                });
                client
                    .recent_input_records
                    .push_back(crate::state::SequencedInputObservation {
                        sequence: base_sequence,
                        input: crate::state::ClientInputObservation {
                            bits: 0,
                            direction: 0,
                        },
                        door_seed: 0,
                        door_phase_tick: 0,
                    });
                let witness = crate::state::CanonicalPositionWitness {
                    sequence: base_sequence,
                    life_seq: client.life_seq,
                    state: base_state,
                    wire_x: spawn_x,
                    wire_y: spawn_y,
                    wire_velocity_x: 0,
                    wire_velocity_y: 0,
                    direction: 0,
                    door_seed: 0,
                    door_phase_tick: 0,
                };
                client.server_sim_state = Some(base_state);
                client.server_sim_anchor_tick = Some(9_999);
                client.canonical_movement_witness = Some(witness);
                client.canonical_position_witnesses.push_back(witness);
                client.server_sim_input_bits = Some(0);
                client.last_position_server_tick = Some(9_999);
                client.last_energy_recharge_tick = Some(9_999);
                client
                    .observer_combat_timeline
                    .seed_input_decision_for_test(weapon_sequence, 10_000)
                    .expect("weapon retry fixture owns an exact combat-view decision");
            }
            let runtime = locked
                .active_player_runtime_identity(shooter)
                .expect("shooter runtime");
            let observer_runtime = locked
                .active_player_runtime_identity(observer)
                .expect("observer runtime");
            let template = crate::snapshot::PendingRealtimeEventEnvelope {
                flight_transition: false,
                recipient: observer_runtime,
                event_id: 1,
                server_tick: 1,
                payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                    crate::realtime_event::RealtimeWeaponEvent {
                        source_player_id: shooter,
                        source_session_id: runtime.session.session_id.get(),
                        source_life_seq: runtime.life.life_seq,
                        shot_group_id: 1,
                        x: 500.0,
                        y: 700.0,
                        velocity_x: 0.0,
                        velocity_y: 0.0,
                        orientation: 0.0,
                        frequency: 0,
                        ship: 0,
                        weapon_data: 2,
                    },
                ),
            };
            locked
                .clients
                .get_mut(&observer)
                .expect("observer")
                .pending_realtime_events
                .queue
                .extend(std::iter::repeat_n(
                    template,
                    crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                ));
            let energy = locked.clients[&shooter].server_energy;
            locked.queue_position_input(shooter, weapon_packet.clone());
            (energy, runtime)
        };

        // This is the sole browser send. The full observer queue defers it
        // without spending energy, consuming sequence state, or spawning.
        run_server_tick_at_tick(&state, Instant::now(), 10_000);
        {
            let locked = state.lock().expect("state");
            assert_eq!(locked.active_projectiles.len(), 0);
            assert_eq!(locked.clients[&shooter].server_energy, energy_before);
            assert_eq!(
                locked
                    .pending_position_inputs
                    .queued_count_for_player(shooter),
                1,
                "the server owns the exact deferred packet; deferred={} overflow={} last_seq={:?} weapon_seen={} position={}",
                locked.realtime_event_actions_deferred,
                locked.realtime_event_queue_overflows,
                locked.clients[&shooter].last_position_sequence,
                locked.clients[&shooter].weapon_sequence_seen(weapon_sequence),
                locked.clients[&shooter].last_position.is_some(),
                // Included in the failure trace to make this production-path
                // fixture diagnose an authority reject rather than masking it.
            );
            assert_eq!(locked.realtime_event_actions_deferred, 1);
        }
        {
            let mut locked = state.lock().expect("state");
            let later_input_count =
                crate::authority_inputs::DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER as u16 + 4;
            for offset in 1..=later_input_count {
                let mut later = weapon_packet.clone();
                let sequence = weapon_sequence.wrapping_add(offset);
                later[28..32].copy_from_slice(
                    &crate::continuum_position::zubspace_position_sequence_items(sequence)
                        .to_le_bytes(),
                );
                locked.pending_position_inputs.queue(runtime, later);
            }
            assert_eq!(
                locked
                    .pending_position_inputs
                    .queued_count_for_player(shooter),
                crate::authority_inputs::DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER + 1,
                "one protected recovery edge plus the complete bounded authority-witness window"
            );
            locked
                .clients
                .get_mut(&observer)
                .expect("observer")
                .pending_realtime_events
                .queue
                .clear();
        }

        // Once outbound delivery drains the pressure queue, the next
        // production tick admits the retained packet automatically; no
        // second queue/send.
        run_server_tick_at_tick(&state, Instant::now(), 10_001);
        {
            let locked = state.lock().expect("state");
            assert_eq!(locked.active_projectiles.len(), 1);
            assert!(locked.clients[&shooter].server_energy < energy_before);
            assert!(locked.clients[&shooter].weapon_sequence_seen(weapon_sequence));
            assert!(
                locked
                    .pending_position_inputs
                    .queued_count_for_player(shooter)
                    <= crate::authority_inputs::DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER,
                "later unordered weapon inputs remain bounded after the protected edge admits"
            );
        }
        let projectile_count = state.lock().expect("state").active_projectiles.len();
        {
            let mut locked = state.lock().expect("state");
            locked.clear_pending_position_inputs_for_player(shooter);
        }
        run_server_tick_at_tick(&state, Instant::now(), 10_002);
        assert_eq!(
            state.lock().expect("state").active_projectiles.len(),
            projectile_count,
            "automatic retry is consumed exactly once and lifecycle cleanup drops only later work",
        );

        let locked = state.lock().expect("state");
        assert_eq!(
            locked.realtime_event_queue_current(),
            2,
            "the exact accepted fact is queued once for shooter and observer",
        );
        assert!(locked.player_runtime_matches(runtime));
        drop((shooter_rx, observer_rx));
    }

    #[test]
    fn server_tick_defers_position_until_late_redundant_input_arrives() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let sequence = 0xfffe;
        let mut position = position_packet_at(0, 500, 700, 0);
        position[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
        );
        assert!(
            state
                .lock()
                .expect("state")
                .queue_position_input_for_runtime(runtime, position)
        );

        run_server_tick_at_tick(&state, Instant::now(), 100);
        {
            let state = state.lock().expect("state");
            assert!(state.clients[&player_id].last_position.is_none());
            assert_eq!(
                state
                    .pending_position_inputs
                    .queued_count_for_player(player_id),
                1
            );
            assert!(state.input_recovery_deferred_at.is_empty());
        }

        let mut input_packet = vec![0x62];
        input_packet.extend_from_slice(&sequence.to_le_bytes());
        input_packet.push(1);
        input_packet.extend_from_slice(&0_u16.to_le_bytes());
        input_packet.extend_from_slice(&[0, 0]);
        input_packet.extend_from_slice(
            &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
        );
        let mut joined = true;
        crate::connection::handle_client_packet(
            &state,
            player_id,
            input_packet,
            "test-peer",
            &mut joined,
        );
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .recent_input_records
            .iter_mut()
            .find(|record| record.sequence == sequence)
            .expect("late input record")
            .door_phase_tick = 101;
        run_server_tick_at_tick(&state, Instant::now(), 101);

        let state = state.lock().expect("state");
        assert_eq!(
            state.clients[&player_id].last_position_sequence,
            Some(sequence)
        );
        assert!(state.pending_position_inputs.is_empty());
        assert!(state.input_recovery_deferred_at.is_empty());
    }

    #[test]
    fn server_tick_defers_position_until_authenticated_door_phase_starts() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let sequence = 321_u16;
        let mut input_packet = vec![0x62];
        input_packet.extend_from_slice(&sequence.to_le_bytes());
        input_packet.push(1);
        input_packet.extend_from_slice(&0_u16.to_le_bytes());
        input_packet.extend_from_slice(&[0, 0]);
        input_packet.extend_from_slice(
            &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
        );
        let mut joined = true;
        crate::connection::handle_client_packet(
            &state,
            player_id,
            input_packet,
            "test-peer",
            &mut joined,
        );
        let mut position = position_packet_at(0, 500, 700, 0);
        position[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
        );
        {
            let mut locked = state.lock().expect("state");
            locked
                .clients
                .get_mut(&player_id)
                .expect("client")
                .recent_input_records
                .iter_mut()
                .find(|record| record.sequence == sequence)
                .expect("input record")
                .door_phase_tick = 101;
            locked.pending_position_inputs.queue(runtime, position);
        }

        run_server_tick_at_tick(&state, Instant::now(), 100);
        {
            let locked = state.lock().expect("state");
            assert!(locked.clients[&player_id].last_position.is_none());
            assert_eq!(
                locked
                    .pending_position_inputs
                    .queued_count_for_player(player_id),
                1
            );
            assert_eq!(
                locked.input_recovery_deferred_at.get(&runtime.life),
                Some(&100)
            );
        }

        run_server_tick_at_tick(&state, Instant::now(), 101);
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.clients[&player_id].last_position_sequence,
            Some(sequence)
        );
        assert!(locked.pending_position_inputs.is_empty());
        assert!(locked.input_recovery_deferred_at.is_empty());
    }

    #[test]
    fn admitted_deferred_movement_staggers_a_newer_interleaved_movement_to_next_tick() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let packet = |sequence: u16, x: u16| {
            let mut packet = position_packet_at(0, x, 700, 0);
            packet[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(sequence)
                    .to_le_bytes(),
            );
            packet
        };
        let old = packet(100, 500);
        let newer = packet(101, 501);
        {
            let mut locked = state.lock().expect("state");
            // Seed the exact retained input witnesses as though their
            // redundant records arrived while the old position was waiting.
            locked.queue_position_input(player_id, old.clone());
            let _ = locked.drain_position_inputs();
            locked.queue_position_input(player_id, newer.clone());
            let _ = locked.drain_position_inputs();
            // Deterministic lock-gap interleaving: old deferred retry first,
            // then a newer movement that arrived while processing.
            locked
                .pending_position_inputs
                .requeue_deferred(runtime, old);
            locked.pending_position_inputs.queue(runtime, newer);
        }

        run_server_tick_at_tick(&state, Instant::now(), 100);
        {
            let locked = state.lock().expect("state");
            assert_eq!(locked.clients[&player_id].last_position_sequence, Some(100));
            let queued = locked
                .pending_position_inputs
                .queued_for_player(player_id)
                .expect("newer movement retained");
            assert_eq!(queued.len(), 1);
            assert_eq!(
                crate::continuum_position::parse_client_position_snapshot(&queued[0])
                    .and_then(|snapshot| snapshot.client_sequence),
                Some(101)
            );
        }

        run_server_tick_at_tick(&state, Instant::now(), 101);
        let locked = state.lock().expect("state");
        assert_eq!(locked.clients[&player_id].last_position_sequence, Some(101));
        assert!(locked.pending_position_inputs.is_empty());
        assert!(locked.input_recovery_deferred_at.is_empty());
    }

    #[test]
    fn server_tick_does_not_defer_reordered_position_for_input_recovery() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        {
            let mut state = state.lock().expect("state");
            state
                .clients
                .get_mut(&player_id)
                .expect("client")
                .last_position_sequence = Some(100);
        }
        let mut reordered = position_packet_at(0, 500, 700, 0);
        reordered[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(99).to_le_bytes(),
        );
        assert!(
            state
                .lock()
                .expect("state")
                .queue_position_input_for_runtime(runtime, reordered)
        );

        run_server_tick_at_tick(&state, Instant::now(), 100);

        let state = state.lock().expect("state");
        assert_eq!(state.clients[&player_id].last_position_sequence, Some(100));
        assert!(state.pending_position_inputs.is_empty());
        assert!(state.input_recovery_deferred_at.is_empty());
    }

    #[test]
    fn reordered_packet_does_not_discard_an_advancing_packets_recovery_bridge() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let mut advancing = position_packet_at(0, 500, 700, 0);
        advancing[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(101).to_le_bytes(),
        );
        let mut reordered_weapon = position_packet_at(0, 500, 700, 2);
        reordered_weapon[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(99).to_le_bytes(),
        );
        {
            let mut state = state.lock().expect("state");
            state
                .clients
                .get_mut(&player_id)
                .expect("client")
                .last_position_sequence = Some(100);
            state.pending_position_inputs.queue(runtime, advancing);
            state
                .pending_position_inputs
                .queue(runtime, reordered_weapon);
        }

        run_server_tick_at_tick(&state, Instant::now(), 100);

        let state = state.lock().expect("state");
        assert_eq!(state.clients[&player_id].last_position_sequence, Some(100));
        assert_eq!(
            state
                .pending_position_inputs
                .queued_count_for_player(player_id),
            1
        );
        assert!(state.input_recovery_deferred_at.is_empty());
    }

    #[test]
    fn server_tick_bounds_omitted_input_until_wire_recovery_becomes_irrecoverable() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let sequence = 77;
        let mut position = position_packet_at(0, 500, 700, 0);
        position[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
        );
        assert!(
            state
                .lock()
                .expect("state")
                .queue_position_input_for_runtime(runtime, position)
        );

        for tick in 200..=200 + INPUT_RECOVERY_DEFER_TICKS * 4 {
            run_server_tick_at_tick(&state, Instant::now(), tick);
            let state = state.lock().expect("state");
            assert!(state.input_recovery_deferred_at.is_empty());
            assert!(
                state
                    .pending_position_inputs
                    .queued_count_for_player(player_id)
                    <= 1
            );
        }

        let state = state.lock().expect("state");
        assert!(state.clients[&player_id].last_position.is_none());
        assert!(state.clients[&player_id].last_position_sequence.is_none());
        assert_eq!(
            state
                .pending_position_inputs
                .queued_count_for_player(player_id),
            1,
            "one recoverable movement stays bounded independently of wall time"
        );
        assert!(state.input_recovery_deferred_at.is_empty());
    }

    #[test]
    fn input_history_recovery_tracks_actual_hole_across_wire_boundary_and_wrap() {
        for (base_sequence, missing_offset, exhausted) in
            [100_u16, u16::MAX - 30].into_iter().flat_map(|base| {
                [(1, true), (25, true), (26, false), (40, false), (56, false)]
                    .map(|(offset, exhausted)| (base, offset, exhausted))
            })
        {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (tx, _rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);
            let tail = base_sequence.wrapping_add(57);
            let missing = base_sequence.wrapping_add(missing_offset);
            let runtime;
            {
                let mut locked = state.lock().expect("state");
                runtime = locked
                    .active_player_runtime_identity(player_id)
                    .expect("runtime");
                let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
                let mut base = position_packet_at(0, x, y, 0);
                base[28..32].copy_from_slice(
                    &crate::continuum_position::zubspace_position_sequence_items(base_sequence)
                        .to_le_bytes(),
                );
                locked.set_client_position(
                    player_id,
                    large_position_from_client(player_id, &base).unwrap(),
                );
                let client = locked.clients.get_mut(&player_id).unwrap();
                let movement = crate::state::ServerSimMovement {
                    position_x: x as f32,
                    position_y: y as f32,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    orientation: 0.0,
                    energy: client.server_energy as f32,
                    max_energy: client.server_max_energy as f32,
                };
                client.alive = true;
                client.respawn_at_tick = None;
                client.last_position_sequence = Some(base_sequence);
                client.last_position_server_tick = Some(100);
                client.canonical_movement_witness = Some(crate::state::CanonicalPositionWitness {
                    sequence: base_sequence,
                    life_seq: client.life_seq,
                    state: movement,
                    wire_x: x,
                    wire_y: y,
                    wire_velocity_x: 0,
                    wire_velocity_y: 0,
                    direction: 0,
                    door_seed: 0,
                    door_phase_tick: 100,
                });
                client.server_sim_state = Some(movement);
                client.server_sim_anchor_tick = Some(100);
                client.last_input_seq = Some(tail);
                client.recent_input_records = (1..=57_u16)
                    .filter(|offset| *offset != missing_offset)
                    .map(|offset| crate::state::SequencedInputObservation {
                        sequence: base_sequence.wrapping_add(offset),
                        input: crate::state::ClientInputObservation {
                            bits: 0,
                            direction: 0,
                        },
                        door_seed: 0,
                        door_phase_tick: 100,
                    })
                    .collect();
                assert_eq!(
                    crate::sim_observation::first_missing_position_input_sequence(client, tail),
                    Some(missing),
                );
                assert_eq!(
                    crate::sim_observation::validate_position_input_witness(client, tail, 0, 200),
                    Err(crate::sim_observation::PositionInputWitnessError::Missing)
                );
                let mut position = position_packet_at(0, x, y, 0);
                position[28..32].copy_from_slice(
                    &crate::continuum_position::zubspace_position_sequence_items(tail)
                        .to_le_bytes(),
                );
                locked.pending_position_inputs.queue(runtime, position);
            }
            run_server_tick_at_tick(&state, Instant::now(), 200);
            {
                let locked = state.lock().unwrap();
                let client = &locked.clients[&player_id];
                assert_eq!(
                    client.pending_movement_correction.is_some(),
                    exhausted,
                    "base={base_sequence}, actual missing age={}",
                    tail.wrapping_sub(missing)
                );
                assert_eq!(client.last_position_sequence, Some(base_sequence));
                if exhausted {
                    assert!(locked.pending_position_inputs.is_empty());
                    continue;
                }
                assert_eq!(
                    locked
                        .pending_position_inputs
                        .queued_count_for_player(player_id),
                    1
                );
            }
            let mut packet = vec![0x62];
            packet.extend_from_slice(&missing.to_le_bytes());
            packet.push(1);
            packet.extend_from_slice(&0_u16.to_le_bytes());
            packet.extend_from_slice(&[0, 0]);
            packet.extend_from_slice(
                &crate::combat_timeline::CombatViewReceiptV1::unavailable(missing).encode(),
            );
            assert!(crate::connection::record_client_input_observation_at_tick(
                &state, runtime, &packet, 200
            ));
            run_server_tick_at_tick(&state, Instant::now(), 201);
            let locked = state.lock().unwrap();
            assert_eq!(
                locked.clients[&player_id].last_position_sequence,
                Some(tail)
            );
            assert!(
                locked.clients[&player_id]
                    .pending_movement_correction
                    .is_none()
            );
            assert!(locked.pending_position_inputs.is_empty());
        }
    }

    #[test]
    fn exhausted_input_history_opens_one_exact_correction_and_reanchors_at_application_tail() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let base_sequence = u16::MAX - 20;
        let detected_sequence = base_sequence.wrapping_add(1);
        // The first required record has just fallen one step beyond the
        // browser's exact 32-entry rolling wire bundle, while remaining well
        // inside the server's wider 64-entry retention horizon.
        let application_sequence = base_sequence.wrapping_add(34);
        let server_tick = 10_000_u32;
        let runtime;
        let anchor_state;
        let old_door_phase;
        {
            let mut locked = state.lock().expect("state");
            runtime = locked
                .active_player_runtime_identity(player_id)
                .expect("runtime");
            let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(locked.zone_map());
            let mut base_packet = position_packet_at(0, spawn_x, spawn_y, 0);
            base_packet[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(base_sequence)
                    .to_le_bytes(),
            );
            let large = large_position_from_client(player_id, &base_packet)
                .expect("established position fixture");
            locked.set_client_position(player_id, large);
            let client = locked.clients.get_mut(&player_id).expect("client");
            let door_state = crate::doors::ClientDoorState::for_challenge(0);
            client.door_state = Some(door_state);
            old_door_phase =
                crate::doors::client_door_phase_candidates_at_tick(Some(door_state), 100)
                    .into_iter()
                    .filter(|phase| {
                        crate::continuum::continuum_tick_diff(100, phase.started_tick) >= 0
                    })
                    .min_by_key(|phase| {
                        crate::continuum::continuum_tick_elapsed(100, phase.started_tick)
                    })
                    .expect("the authenticated old door phase has started");
            client.alive = true;
            client.respawn_at_tick = None;
            anchor_state = crate::state::ServerSimMovement {
                position_x: spawn_x as f32,
                position_y: spawn_y as f32,
                velocity_x: 17.0,
                velocity_y: -9.0,
                orientation: 0.25,
                energy: client.server_energy as f32,
                max_energy: client.server_max_energy as f32,
            };
            let witness = crate::state::CanonicalPositionWitness {
                sequence: base_sequence,
                life_seq: client.life_seq,
                state: anchor_state,
                wire_x: spawn_x,
                wire_y: spawn_y,
                wire_velocity_x: crate::continuum_position::protocol_velocity_component(17.0),
                wire_velocity_y: crate::continuum_position::protocol_velocity_component(-9.0),
                direction: crate::continuum_position::protocol_direction(0.25),
                door_seed: old_door_phase.seed,
                door_phase_tick: old_door_phase.started_tick,
            };
            client.last_position_sequence = Some(base_sequence);
            client.canonical_movement_witness = Some(witness);
            client.canonical_position_witnesses.push_back(witness);
            client.server_sim_state = Some(anchor_state);
            client.server_sim_anchor_tick = Some(server_tick - 1);
            client.last_input_seq = Some(application_sequence.wrapping_sub(1));
            let first_retained = base_sequence.wrapping_add(2);
            client.recent_input_records.extend(
                (0..crate::connection::CLIENT_INPUT_RECORD_MAX_INPUTS as u16).map(|offset| {
                    crate::state::SequencedInputObservation {
                        sequence: first_retained.wrapping_add(offset),
                        input: crate::state::ClientInputObservation {
                            bits: 0,
                            direction: 0,
                        },
                        door_seed: old_door_phase.seed,
                        door_phase_tick: old_door_phase.started_tick,
                    }
                }),
            );
            assert_eq!(
                client
                    .last_input_seq
                    .unwrap()
                    .wrapping_sub(detected_sequence),
                crate::connection::CLIENT_INPUT_RECORD_MAX_INPUTS as u16,
                "the exact first missing record is no longer retransmittable"
            );

            let mut detected = position_packet_at(0, spawn_x.wrapping_add(400), spawn_y, 0);
            detected[28..32].copy_from_slice(
                &crate::continuum_position::zubspace_position_sequence_items(detected_sequence)
                    .to_le_bytes(),
            );
            locked.pending_position_inputs.queue(runtime, detected);
        }

        run_server_tick_at_tick(&state, Instant::now(), server_tick);

        let proposal = {
            let locked = state.lock().expect("state");
            let transaction = locked.clients[&player_id]
                .pending_movement_correction
                .as_ref()
                .expect("history exhaustion must open one correction");
            assert_eq!(
                transaction.proposal.reason,
                crate::movement_correction::MovementCorrection::REASON_INPUT_HISTORY_GAP
            );
            assert_eq!(transaction.proposal.position_sequence, detected_sequence);
            assert_eq!(transaction.proposal.position_x, anchor_state.position_x);
            assert_eq!(transaction.proposal.position_y, anchor_state.position_y);
            assert!(transaction.offer_reliable_id.is_some());
            assert!(locked.pending_position_inputs.is_empty());
            assert_eq!(
                locked.clients[&player_id].last_position_sequence,
                Some(base_sequence),
                "unverifiable suffix cannot advance movement authority"
            );
            transaction.proposal
        };

        let receipt = {
            let mut locked = state.lock().expect("state");
            // Model the production ordering which exposed the bug: the exact
            // application-sequence record is still retained, but it belongs to
            // the unverifiable predecessor suffix and carries the old door
            // phase. The gap correction must establish current door-time
            // provenance rather than copy that stale record into the new
            // sequence lineage.
            locked
                .clients
                .get_mut(&player_id)
                .expect("client")
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: application_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: crate::continuum_position::protocol_direction(
                            anchor_state.orientation,
                        ),
                    },
                    door_seed: old_door_phase.seed,
                    door_phase_tick: old_door_phase.started_tick,
                });
            assert!(
                locked.clients[&player_id]
                    .recent_input_records
                    .iter()
                    .any(|record| {
                        record.sequence == application_sequence
                            && record.door_phase_tick == old_door_phase.started_tick
                    })
            );
            apply_movement_correction_application_at_tick(
                &mut locked,
                runtime,
                crate::movement_correction::MovementCorrectionApplied {
                    intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
                    server_session_id: proposal.server_session_id,
                    player_id,
                    life_seq: runtime.life.life_seq,
                    correction_generation: proposal.correction_generation,
                    detected_sequence,
                    application_sequence,
                },
                server_tick + 1,
            )
            .expect("exact current-runtime application must close the gap")
        };
        assert_eq!(
            receipt.outcome,
            crate::movement_correction::MovementCorrectionAccepted::OUTCOME_APPLIED
        );
        assert_eq!(receipt.application_sequence, application_sequence);
        assert_eq!(receipt.server_tick, server_tick + 1);
        assert_eq!(receipt.position_x, anchor_state.position_x);
        assert_eq!(receipt.position_y, anchor_state.position_y);
        let mut locked = state.lock().expect("state");
        let expected_application_phase = crate::doors::client_door_phase_candidates_at_tick(
            locked.clients[&player_id].door_state,
            server_tick + 1,
        )
        .into_iter()
        .filter(|phase| {
            crate::continuum::continuum_tick_diff(server_tick + 1, phase.started_tick) >= 0
        })
        .min_by_key(|phase| {
            crate::continuum::continuum_tick_elapsed(server_tick + 1, phase.started_tick)
        })
        .expect("the correction application has an authenticated current door phase");
        let client = &locked.clients[&player_id];
        assert_eq!(client.last_position_sequence, Some(application_sequence));
        assert_eq!(client.last_input_seq, Some(application_sequence));
        assert!(client.recent_input_records.is_empty());
        assert_eq!(
            client
                .canonical_movement_witness
                .map(|witness| witness.sequence),
            Some(application_sequence)
        );
        let application_witness = client
            .canonical_movement_witness
            .expect("application establishes a canonical witness");
        assert_eq!(
            application_witness.door_seed,
            expected_application_phase.seed
        );
        assert_eq!(
            application_witness.door_phase_tick, expected_application_phase.started_tick,
            "the new sequence lineage must not retain the historical shadow phase"
        );

        let next_sequence = application_sequence.wrapping_add(1);
        let next_tick = server_tick + 2;
        let next_phase =
            crate::doors::client_door_phase_candidates_at_tick(client.door_state, next_tick)
                .into_iter()
                .filter(|phase| {
                    crate::continuum::continuum_tick_diff(next_tick, phase.started_tick) >= 0
                })
                .min_by_key(|phase| {
                    crate::continuum::continuum_tick_elapsed(next_tick, phase.started_tick)
                })
                .expect("the first post-correction input has an authenticated phase");
        let direction = crate::continuum_position::protocol_direction(anchor_state.orientation);
        locked
            .clients
            .get_mut(&player_id)
            .expect("client")
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: next_sequence,
                input: crate::state::ClientInputObservation { bits: 0, direction },
                door_seed: next_phase.seed,
                door_phase_tick: next_phase.started_tick,
            });
        let map = locked.zone_map();
        let client = &locked.clients[&player_id];
        let upgrades = crate::sim_observation::movement_upgrades_for_client(client);
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(&map);
        let replay = crate::sim_observation::replay_canonical_position_gap(
            client,
            &map,
            false,
            false,
            false,
            upgrades,
            &[],
            next_sequence,
            next_tick,
            spawn_x,
            spawn_y,
        )
        .expect("the first honest post-correction input must replay from the rebased witness");
        assert_eq!(
            replay.witnesses.last().map(|witness| witness.sequence),
            Some(next_sequence)
        );
    }

    #[test]
    fn server_tick_discards_position_queued_by_an_earlier_life() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        {
            let mut state = state.lock().expect("state");
            state.queue_position_input(player_id, position_packet(1));
            let queued_identity = state
                .pending_position_inputs
                .queued_for_player(player_id)
                .and_then(|queue| queue.front())
                .map(|input| input.identity)
                .expect("queued input should retain its life");
            let current_identity = state
                .advance_client_life(player_id)
                .expect("client life should advance");
            assert_ne!(queued_identity.life, current_identity);
        }

        run_server_tick(&state, Instant::now());

        let state = state.lock().expect("state");
        assert!(state.pending_position_inputs.is_empty());
        assert!(state.clients[&player_id].last_position.is_none());
    }

    #[test]
    fn admitted_application_receipts_never_cross_a_reused_player_id_session() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (old_tx, old_rx) = mpsc::channel();
        let player_id = register_connection(&state, old_tx);
        state
            .lock()
            .expect("state")
            .life_seq_watermarks
            .insert(player_id, u32::MAX - 1);
        join_arena(&state, player_id, 0);
        let old_runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(player_id)
            .expect("old runtime");
        let sequence = u16::MAX;
        let (spawn_x, spawn_y, energy) = {
            let locked = state.lock().expect("state");
            let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
            (x, y, locked.clients[&player_id].server_energy)
        };
        let mut admitted_packet = position_packet_at(0, spawn_x, spawn_y, 0);
        admitted_packet[18..20].copy_from_slice(&energy.to_le_bytes());
        admitted_packet[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
        );
        {
            let mut locked = state.lock().expect("state");
            locked.queue_position_input(player_id, admitted_packet.clone());
            let _ = locked.drain_position_inputs();
            let snapshot =
                crate::continuum_position::parse_client_position_snapshot(&admitted_packet)
                    .expect("current-protocol exact fixture");
            locked
                .clients
                .get_mut(&player_id)
                .expect("client")
                .pending_frequency_baseline = Some(snapshot);
        }
        let (admitted_runtime, _) = accept_position_relay_for_runtime_at_tick(
            &state,
            old_runtime,
            &admitted_packet,
            10_000,
        )
        .expect("old runtime position should admit before the receipt pause");
        assert_eq!(admitted_runtime, old_runtime);

        // Deterministic pause between admission and receipt enqueue: replace
        // the connection with a new session occupying the same public id and
        // the same wrapped life/position/application values.
        disconnect_player(&state, player_id);
        {
            let mut locked = state.lock().expect("state");
            locked.next_player_id = player_id;
            // Model full generation wrap/reuse: every public application
            // field can collide, while the internal session id cannot.
            locked.life_seq_watermarks.insert(player_id, u32::MAX - 1);
        }
        let (replacement_tx, replacement_rx) = mpsc::channel();
        let replacement_id = register_connection(&state, replacement_tx);
        assert_eq!(replacement_id, player_id);
        join_arena(&state, replacement_id, 0);
        let replacement_runtime = state
            .lock()
            .expect("state")
            .active_player_runtime_identity(replacement_id)
            .expect("replacement runtime");
        assert_eq!(replacement_runtime.life.life_seq, old_runtime.life.life_seq);
        assert_ne!(replacement_runtime.session, old_runtime.session);

        drain_available(&old_rx);
        let mut joined = true;
        while let Ok(packet) = replacement_rx.try_recv() {
            if let Some((id, _)) = crate::reliable::reliable_parts(&packet) {
                crate::connection::handle_client_packet(
                    &state,
                    replacement_id,
                    crate::reliable::reliable_ack(id),
                    "replacement-receipt-peer",
                    &mut joined,
                );
            }
        }
        {
            let mut locked = state.lock().expect("state");
            let replacement = locked
                .clients
                .get_mut(&replacement_id)
                .expect("replacement");
            assert_eq!(replacement.outgoing_reliable.pending_len(), 0);
            replacement.outgoing_reliable.set_next_id_for_test(u32::MAX);
        }

        let baseline = lifecycle_baseline_accepted(
            u32::MAX,
            replacement_runtime.life.life_seq,
            sequence,
            10_000,
            (8200, 8200),
        );
        let warp = warp_relocation_accepted(u32::MAX, replacement_runtime.life.life_seq, sequence);
        assert!(!send_reliable_to_runtime(
            &state,
            admitted_runtime,
            baseline.clone(),
        ));
        assert!(!send_reliable_to_runtime(
            &state,
            admitted_runtime,
            warp.clone(),
        ));
        assert!(
            replacement_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );

        assert!(send_reliable_to_runtime(
            &state,
            replacement_runtime,
            baseline.clone(),
        ));
        assert!(send_reliable_to_runtime(
            &state,
            replacement_runtime,
            warp.clone(),
        ));
        let first = replacement_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("correct runtime receives baseline receipt");
        let second = replacement_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("correct runtime receives Warp receipt");
        assert_eq!(
            crate::reliable::reliable_parts(&first),
            Some((u32::MAX, baseline.as_slice()))
        );
        assert_eq!(
            crate::reliable::reliable_parts(&second),
            Some((0, warp.as_slice()))
        );

        assert_eq!(
            crate::reliable::resend_due_reliable_to_clients(
                &state,
                Instant::now() + Duration::from_secs(4),
            ),
            2,
            "lost correct-session receipts remain retained across id wrap"
        );
        let resent = [
            replacement_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            replacement_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert!(resent.contains(&first));
        assert!(resent.contains(&second));
    }

    #[test]
    fn server_tick_recharges_idle_players_without_position_packets() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_alpha, rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        drain_available(&rx_alpha);

        {
            let mut state = state.lock().expect("state");
            let position = large_position_from_client(alpha, &position_packet(0x1122_3344))
                .expect("position should encode");
            state.set_client_position(alpha, position);
            let client = state.clients.get_mut(&alpha).expect("alpha should exist");
            client.server_energy = 1_300;
            client.server_energy_remainder = 0.0;
            client.server_recharge_per_second =
                crate::arena_settings::arena_ship_initial_recharge_per_second(0);
            client.last_energy_recharge_tick = Some(crate::continuum::continuum_tick_sub(
                crate::wire::current_tick(),
                100,
            ));
        }

        run_server_tick(&state, Instant::now());

        let state = state.lock().expect("state");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert!(
            client.server_energy >= 1_319,
            "idle server tick should recharge authoritative energy, got {}",
            client.server_energy
        );
        let cached = client
            .last_position
            .as_ref()
            .expect("cached position should exist");
        assert_eq!(
            u16::from_le_bytes(cached[21..23].try_into().unwrap()),
            client.server_energy
        );
    }

    #[test]
    fn same_tick_position_after_recharge_charges_continuous_drain_once() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        drain_available(&rx);

        let start_tick = 100;
        let server_tick = 200;
        let initial_energy = 1_000;
        let max_energy = 2_000;
        {
            let mut state = state.lock().expect("state");
            let client = state
                .clients
                .get_mut(&player_id)
                .expect("player should exist");
            client.server_energy = initial_energy;
            client.server_max_energy = max_energy;
            client.server_energy_remainder = 0.0;
            client.server_recharge_per_second =
                crate::arena_settings::arena_ship_initial_recharge_per_second(0);
            client.last_energy_recharge_tick = Some(start_tick);
            client.last_continuous_energy_drain_tick = Some(start_tick);
        }

        let base_speed = crate::arena_settings::arena_ship_base_max_speed_pixels_per_second(0);
        let max_speed = crate::arena_settings::arena_ship_max_speed_pixels_per_second(0);
        assert!(max_speed > base_speed + 5.0);
        let afterburn_velocity = ((base_speed + 10.0) * 10.0).round() as i16;
        {
            let mut prior = position_packet_at(0, 499, 700, 0);
            prior[6..8].copy_from_slice(&afterburn_velocity.to_le_bytes());
            let prior = large_position_from_client(player_id, &prior).expect("prior position");
            let mut state = state.lock().expect("state");
            state.set_client_position(player_id, prior);
            // Continuous drain is owned by the exact authenticated input
            // bound to the prior accepted position. Speed alone is not proof:
            // Top Speed prizes and external impulses can legitimately exceed
            // the initial cap without an Afterburner key.
            state
                .clients
                .get_mut(&player_id)
                .expect("player should exist")
                .server_sim_input_bits = Some((1 << 4) | (1 << 0));
        }
        let mut first_position = position_packet_at(0, 500, 700, 0);
        first_position[6..8].copy_from_slice(&afterburn_velocity.to_le_bytes());
        synchronize_test_position_exact_state(&mut first_position);
        state
            .lock()
            .expect("state")
            .queue_position_input(player_id, first_position);

        run_server_tick_at_tick(&state, Instant::now(), server_tick);

        let mut expected = crate::authority::EnergyPool::new(initial_energy, max_energy, 0.0);
        assert!(expected.spend_continuous_energy(
            crate::arena_settings::arena_ship_afterburner_energy_per_second(0)
        ));
        expected.recharge_for_position_ticks(
            server_tick - start_tick,
            crate::arena_settings::arena_ship_initial_recharge_per_second(0),
        );
        let (energy_after_first, remainder_after_first) = {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("player should exist");
            assert_eq!(client.last_energy_recharge_tick, Some(server_tick));
            assert_eq!(client.last_continuous_energy_drain_tick, Some(server_tick));
            assert_eq!(client.server_energy, expected.current());
            assert!((client.server_energy_remainder - expected.recharge_remainder()).abs() < 0.001);
            (client.server_energy, client.server_energy_remainder)
        };

        // A second accepted position at the same authoritative tick must not
        // charge either the preceding recharge interval or its drain twice.
        let mut second_position = position_packet_at(0, 501, 700, 0);
        second_position[6..8].copy_from_slice(&afterburn_velocity.to_le_bytes());
        synchronize_test_position_exact_state(&mut second_position);
        state
            .lock()
            .expect("state")
            .queue_position_input(player_id, second_position);
        run_server_tick_at_tick(&state, Instant::now(), server_tick);

        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("player should exist");
        assert_eq!(client.server_energy, energy_after_first);
        assert!((client.server_energy_remainder - remainder_after_first).abs() < 0.001);
        assert_eq!(
            crate::arena::large_position_coordinates(
                client
                    .last_position
                    .as_deref()
                    .expect("second position accepted")
            ),
            Some((501, 700))
        );
    }

    #[test]
    fn same_tick_packet_permutations_cannot_move_the_continuous_drain_anchor() {
        fn replay(
            status_order: &[bool],
            append_equivalent_final_off: bool,
        ) -> (u16, f32, u32, usize, u8) {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (tx, _rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);
            let start_tick = 10_000;
            let server_tick = 10_100;

            let mut prior_packet = position_packet_at(0, 500, 700, 0);
            prior_packet[11] = crate::authority::POSITION_STATUS_STEALTH;
            let prior = large_position_from_client(player_id, &prior_packet).expect("prior");
            {
                let mut state = state.lock().expect("state");
                state.set_client_position(player_id, prior);
                {
                    let client = state.clients.get_mut(&player_id).expect("client");
                    client.server_energy = 1_500;
                    client.server_max_energy = 2_000;
                    client.server_energy_remainder = 0.0;
                    client.server_status.stealth_capable = true;
                    client.last_energy_recharge_tick = Some(start_tick);
                    client.last_continuous_energy_drain_tick = Some(start_tick);
                }
                for &status_on in status_order {
                    let mut packet = position_packet_at(0, 500, 700, 2);
                    packet[11] = if status_on {
                        crate::authority::POSITION_STATUS_STEALTH
                    } else {
                        0
                    };
                    seed_weapon_combat_view_decision(&mut state, player_id, &packet, server_tick);
                    state.queue_position_input(player_id, packet);
                }
                if append_equivalent_final_off {
                    state.queue_position_input(player_id, position_packet_at(0, 500, 700, 0));
                }
            }

            run_server_tick_at_tick(&state, Instant::now(), server_tick);
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            (
                client.server_energy,
                client.server_energy_remainder,
                client
                    .last_continuous_energy_drain_tick
                    .expect("drain anchor"),
                state.active_projectiles.len(),
                client.last_position.as_ref().expect("position")[13]
                    & crate::authority::POSITION_STATUS_STEALTH,
            )
        }

        let off_then_on = replay(&[false, true], false);
        let on_then_off = replay(&[true, false], false);
        assert_eq!(off_then_on.0, on_then_off.0);
        assert!((off_then_on.1 - on_then_off.1).abs() < 0.001);
        assert_eq!(off_then_on.2, 10_100);
        assert_eq!(on_then_off.2, 10_100);
        assert_eq!(off_then_on.3, 1, "cluster accepts only one cooled weapon");
        assert_eq!(on_then_off.3, 1, "cluster accepts only one cooled weapon");
        assert_ne!(off_then_on.4, 0, "last packet selects next-tick on state");
        assert_eq!(on_then_off.4, 0, "last packet selects next-tick off state");

        let reordered_a = replay(&[false, true, false], true);
        let reordered_b = replay(&[true, false, false], true);
        assert_eq!(
            reordered_a, reordered_b,
            "equivalent reordered batches match"
        );
    }

    #[test]
    fn gameplay_energy_replays_identically_across_wrap_despite_wall_pacing() {
        fn replay(wall_step: Duration) -> (u16, f32, Option<u32>, Option<u32>) {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (tx, _rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);

            let start_tick = 0x7fff_fff8;
            let emp_until_tick = 1;
            {
                let mut state = state.lock().expect("state");
                let client = state
                    .clients
                    .get_mut(&player_id)
                    .expect("player should exist");
                client.server_energy = 1_000;
                client.server_max_energy = 2_000;
                client.server_energy_remainder = 0.0;
                client.server_recharge_per_second =
                    crate::arena_settings::arena_ship_initial_recharge_per_second(0);
                client.last_energy_recharge_tick = Some(start_tick);
                client.server_emp_until_tick = Some(emp_until_tick);
            }

            let wall_start = Instant::now();
            for (index, tick) in [0x7fff_fffc, 0x7fff_fffe, 1, 3, 8].into_iter().enumerate() {
                run_server_tick_at_tick(&state, wall_start + wall_step * index as u32, tick);
            }

            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("player should exist");
            (
                client.server_energy,
                client.server_energy_remainder,
                client.last_energy_recharge_tick,
                client.server_emp_until_tick,
            )
        }

        let fast = replay(Duration::from_millis(1));
        let delayed = replay(Duration::from_millis(250));
        assert_eq!(fast, delayed);

        let mut expected = crate::authority::EnergyPool::new(1_000, 2_000, 0.0);
        // EMP ends at tick 1. Only ticks 1..8 recharge, even though the
        // sampled schedule jumps over the 31-bit wrap and the exact expiry.
        expected.recharge_for_position_ticks(
            7,
            crate::arena_settings::arena_ship_initial_recharge_per_second(0),
        );
        assert_eq!(fast.0, expected.current());
        assert!((fast.1 - expected.recharge_remainder()).abs() < 0.001);
        assert_eq!(fast.2, Some(8));
        assert_eq!(fast.3, None, "completed EMP deadline is swept");
    }

    #[test]
    fn swept_gameplay_timers_do_not_reactivate_at_half_range_or_later_cycles() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let start = 1_000;
        let deadline = crate::continuum::continuum_tick_add(start, 1);
        let claim = ServerPrizeClaim {
            collectible_id: start,
            x: 1,
            y: 1,
            prize_id: 123,
        };
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.server_super_until_tick = Some(deadline);
            client.server_shield_until_tick = Some(deadline);
            client.server_emp_until_tick = Some(deadline);
            client.server_repel_until_tick = Some(deadline);
            client.respawn_at_tick = Some(deadline);
            client.last_weapon_fire_tick = Some(start);
            client.last_gun_fire_tick = Some(start);
            client.last_bomb_fire_tick = Some(start);
            client.last_brick_drop_tick = Some(start);
            client.last_accepted_weapon_tick = Some(start);
            client.last_accepted_weapon_data = Some(2);
            client.life_started_tick = Some(start);
            client.recent_damagers.push((
                crate::authority_identity::LifeIdentity::new(player_id, client.life_seq),
                start,
            ));
            state.active_bricks.push(ServerBrick {
                tiles: vec![(1, 1)],
                packet: Vec::new(),
                expires_at_tick: deadline,
            });
            state.active_hidden_prizes.push(ServerHiddenPrize {
                collectible_id: start,
                x: 1,
                y: 1,
                prize_id: 123,
                expires_at_tick: deadline,
            });
            state
                .claimed_prizes
                .insert(claim.collectible_id, prize_receipt(start, 1, 1, 123, start));
            state
                .objectives
                .reset(crate::objectives::synthetic_profile(0, 1))
                .expect("synthetic objective generation should advance");
            state
                .objectives
                .create_powerball(crate::objectives::PowerballState::OnMap {
                    x: 16,
                    y: 16,
                    velocity_x: 0,
                    velocity_y: 0,
                    pickup_unlock_tick: Some(deadline),
                })
                .expect("synthetic powerball should fit profile");
            let mut expired_projectile = projectile(player_id, start);
            expired_projectile.expiry_tick = Some(deadline);
            state.record_projectile(expired_projectile);
        }

        let retention =
            maximum_projectile_retention_ticks().max(arena_prize_claim_retention_ticks());
        let swept_at = crate::continuum::continuum_tick_add(start, retention + 1);
        run_server_tick_at_tick(&state, Instant::now(), swept_at);

        let assert_swept = |state: &ServerState| {
            let client = state.clients.get(&player_id).expect("client");
            assert!(client.server_super_until_tick.is_none());
            assert!(client.server_shield_until_tick.is_none());
            assert!(client.server_emp_until_tick.is_none());
            assert!(client.server_repel_until_tick.is_none());
            assert_eq!(
                client.respawn_at_tick,
                Some(deadline),
                "a due respawn deadline remains latched until canonical admission"
            );
            assert!(client.last_weapon_fire_tick.is_none());
            assert!(client.last_gun_fire_tick.is_none());
            assert!(client.last_bomb_fire_tick.is_none());
            assert!(client.last_brick_drop_tick.is_none());
            assert!(client.last_accepted_weapon_tick.is_none());
            assert!(client.last_accepted_weapon_data.is_none());
            assert!(client.life_started_tick.is_none());
            assert!(client.recent_damagers.is_empty());
            assert!(state.active_bricks.is_empty());
            assert!(state.active_projectiles.is_empty());
            assert!(
                !state
                    .active_hidden_prizes
                    .iter()
                    .any(|prize| { prize.x == 1 && prize.y == 1 && prize.prize_id == 123 })
            );
            assert!(!state.claimed_prizes.contains_key(&claim.collectible_id));
            assert!(matches!(
                state.objectives.powerball(0).map(|ball| ball.state),
                Some(crate::objectives::PowerballState::OnMap {
                    pickup_unlock_tick: None,
                    ..
                })
            ));
        };
        assert_swept(&state.lock().expect("state"));

        // The exact signed half-range and subsequent wrapped cycles are where
        // an uncleared u32 anchor would invert meaning. Cleared state stays
        // cleared regardless of the sampled wire value.
        for tick in [
            crate::continuum::continuum_tick_add(start, 1 << 30),
            start,
            crate::continuum::continuum_tick_add(start, 1 << 30),
        ] {
            run_server_tick_at_tick(&state, Instant::now(), tick);
            assert_swept(&state.lock().expect("state"));
        }
    }

    // Legacy 0x3a batched MOVEMENT_POSITION fan-out was retired in
    // Phase 4 — `accept_position_relay` always returns an empty
    // recipient list, so `send_position_relays_batched` is a no-op.
    // The equivalent coverage for the snapshot stream lives in
    // `tests::snapshot::server_emits_one_snapshot_per_joined_client_at_interval_tick`.

    #[test]
    fn high_rate_bot_motion_matches_the_same_unique_gameplay_tick_schedule() {
        let run = |rate: u16| {
            let state = Arc::new(Mutex::new(ServerState {
                config: ServerConfig {
                    bot_count: 2,
                    tick_rate_hz: rate,
                    ..ServerConfig::default()
                },
                ..ServerState::default()
            }));
            {
                let mut guard = state.lock().expect("state");
                guard.initialize_configured_bots();
                // Compare identical mixed detector assignments at every rate;
                // production roster shuffling is intentionally random.
                for (index, bot) in guard.bots.iter_mut().enumerate() {
                    bot.xradar_selected = index == 0;
                    bot.xradar = bot.xradar_selected && bot.status.xradar_capable;
                }
            }
            for callback in 1..=u32::from(rate) * 2 {
                let tick = callback * 100 / u32::from(rate);
                super::bots::step_server_bots(&state, tick);
            }
            state
                .lock()
                .expect("state")
                .bots
                .iter()
                .map(|bot| {
                    (
                        bot.x.to_bits(),
                        bot.y.to_bits(),
                        bot.velocity_x.to_bits(),
                        bot.velocity_y.to_bits(),
                        bot.orientation.to_bits(),
                        bot.energy,
                        bot.objective_position_sequence,
                    )
                })
                .collect::<Vec<_>>()
        };
        let reference = run(100);
        for rate in [120, 240] {
            assert_eq!(
                run(rate),
                reference,
                "duplicate callbacks must not reduce integrated movement at {rate} Hz"
            );
        }
    }

    #[test]
    fn bot_position_timestamp_uses_snapshot_clock_after_late_arena_start_and_wrap() {
        for server_tick in [53_808, 65_535, 65_536, 131_073] {
            let state = Arc::new(Mutex::new(ServerState {
                config: ServerConfig {
                    bot_count: 2,
                    ..ServerConfig::default()
                },
                ..ServerState::default()
            }));
            state.lock().expect("state").initialize_configured_bots();
            run_server_tick_at_tick(&state, Instant::now(), server_tick);
            let guard = state.lock().expect("state");
            for bot in &guard.bots {
                let packet = bot.large_position_packet();
                assert_eq!(
                    u16::from_le_bytes([packet[2], packet[3]]),
                    server_tick as u16,
                    "bootstrap bot position must share the snapshot clock at tick {server_tick}"
                );
            }
        }
    }

    #[test]
    fn server_tick_keeps_configured_bot_positions_on_snapshot_stream() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 2,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        drain_available(&rx_alpha);

        run_server_tick(&state, Instant::now());

        assert!(
            rx_alpha.try_recv().is_err(),
            "server bot motion must not revive the retired reliable 0x3a relay; snapshots own AOI presentation",
        );
    }

    #[test]
    fn server_bots_patrol_materially_without_prejoin_life_churn() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 2,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let encounter_center = {
            let guard = state.lock().expect("state");
            crate::zone::server_spawn_coordinates(guard.zone_map())
        };
        let before = state
            .lock()
            .expect("state")
            .bots
            .iter()
            .map(|bot| (bot.player.id, bot.life_seq, bot.x, bot.y))
            .collect::<Vec<_>>();

        let mut patrol_max_distance = [0.0_f32; 2];
        for tick in 1..=600 {
            super::bots::step_server_bots(&state, tick);
            let guard = state.lock().expect("state");
            for (index, (_, life_seq, start_x, start_y)) in before.iter().enumerate() {
                let bot = &guard.bots[index];
                patrol_max_distance[index] =
                    patrol_max_distance[index].max((bot.x - start_x).hypot(bot.y - start_y));
                assert!(bot.alive());
                assert_eq!(bot.life_seq, *life_seq, "patrol must never spend a life");
                assert!(
                    (bot.x - f32::from(encounter_center.0))
                        .hypot(bot.y - f32::from(encounter_center.1))
                        <= crate::visibility::NEAR_AOI_RADIUS_PX as f32,
                    "patrol must stay within its leash at every tick"
                );
            }
        }

        let guard = state.lock().expect("state");
        assert!(guard.active_projectiles.is_empty());
        for (index, (player_id, life_seq, start_x, start_y)) in before.into_iter().enumerate() {
            let bot = guard
                .bot(player_id)
                .expect("configured bot remains rostered");
            assert!(bot.alive());
            assert_eq!(
                bot.life_seq, life_seq,
                "unobserved patrol must not spend lives"
            );
            // A patrol can return near its origin. Measure the excursion,
            // not the phase of the orbit at the final sample; the retained
            // regression travelled 215 px before ending 32 px from origin.
            assert!(
                patrol_max_distance[index] > 96.0,
                "bot {player_id} must translate materially while patrolling: start=({start_x},{start_y}) end=({},{})",
                bot.x,
                bot.y,
            );
            let encounter_distance = (bot.x - f32::from(encounter_center.0))
                .hypot(bot.y - f32::from(encounter_center.1));
            assert!(
                encounter_distance <= crate::visibility::NEAR_AOI_RADIUS_PX as f32,
                "unobserved bot {player_id} must remain inside the spawn encounter leash: center=({},{}) bot=({},{}) distance={encounter_distance}",
                encounter_center.0,
                encounter_center.1,
                bot.x,
                bot.y,
            );
        }
    }

    #[test]
    fn server_tick_steers_configured_bots_toward_live_player_positions() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        drain_available(&rx_alpha);

        let (bot_start_x, bot_start_y, target_x, target_y) = {
            let state = state.lock().expect("state");
            let map = state.zone_map();
            let mut corridor = None;
            'rows: for tile_y in 16_u16..1008 {
                for tile_x in 16_u16..975 {
                    let y = tile_y * 16 + 8;
                    let clear_hull_corridor = (tile_x - 1..=tile_x + 33).all(|x| {
                        (tile_y - 1..=tile_y + 1).all(|row| {
                            crate::zone::tile_at_world(map, x * 16 + 8, row * 16 + 8) == 0
                        })
                    });
                    let entirely_nonsafe = (tile_x..=tile_x + 32).all(|x| {
                        !crate::zone::world_position_in_safe_tile_for_map(map, x * 16 + 8, y)
                    });
                    if clear_hull_corridor && entirely_nonsafe {
                        corridor = Some((
                            f32::from(tile_x * 16 + 8),
                            f32::from(y),
                            (tile_x + 32) * 16 + 8,
                            y,
                        ));
                        break 'rows;
                    }
                }
            }
            corridor.expect("Chaos must contain a clear 512px nonsafe bot pursuit corridor")
        };
        let start_tick = crate::wire::current_tick();
        {
            let mut state = state.lock().expect("state");
            let bot = state.bots.first_mut().expect("configured bot should exist");
            bot.x = bot_start_x;
            bot.y = bot_start_y;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.weapons.gun_level = 0;
            let player_position = large_position_from_client(
                alpha,
                &position_packet_at(0x1122_3344, target_x, target_y, 0),
            )
            .expect("player position should encode");
            state.set_client_position(alpha, player_position);
            state
                .clients
                .get_mut(&alpha)
                .expect("joined player")
                .last_position_server_tick = Some(start_tick);
        }

        for offset in 1..=200 {
            run_server_tick_at_tick(
                &state,
                Instant::now(),
                crate::continuum::continuum_tick_add(start_tick, offset),
            );
        }

        let state = state.lock().expect("state");
        let bot = state.bots.first().expect("configured bot should exist");
        let initial_distance =
            (f32::from(target_x) - bot_start_x).hypot(f32::from(target_y) - bot_start_y);
        let final_distance = (f32::from(target_x) - bot.x).hypot(f32::from(target_y) - bot.y);
        assert!(
            bot.x > bot_start_x + 96.0 && final_distance + 96.0 < initial_distance,
            "bot must converge materially toward the live player even after braking inside the encounter envelope: start=({bot_start_x},{bot_start_y}) end=({},{}) target=({target_x},{target_y}) initial_distance={initial_distance} final_distance={final_distance}",
            bot.x,
            bot.y,
        );
        assert!(bot.direction > 0);
    }

    #[test]
    fn server_bot_routes_around_the_retained_chaos_wall_toward_a_hostile_human() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);

        // Exact authoritative geometry retained from the Firefox/Chaos
        // counterfire failure. The horizontal center wall blocks the direct
        // line; bot-id parity previously selected the westward tangent even
        // though the human was east, trapping the bot in a repeating loop.
        let (bot_start_x, bot_start_y) = (8_150.0_f32, 8_097.0_f32);
        let (target_x, target_y) = (8_392_u16, 8_516_u16);
        {
            let mut guard = state.lock().expect("state");
            let player_position =
                large_position_from_client(alpha, &position_packet_at(1, target_x, target_y, 0))
                    .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = bot_start_x;
            bot.y = bot_start_y;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.wall_follow_until_tick = None;
            bot.wall_follow_x = 0;
            bot.wall_follow_y = 0;
            bot.weapons.gun_level = 0;
        }

        let initial_distance =
            (f32::from(target_x) - bot_start_x).hypot(f32::from(target_y) - bot_start_y);
        let mut minimum_distance = initial_distance;
        for tick in 1..=900 {
            super::bots::step_server_bots(&state, tick);
            let guard = state.lock().expect("state");
            let bot = guard.bots.first().expect("configured bot");
            minimum_distance = minimum_distance
                .min((f32::from(target_x) - bot.x).hypot(f32::from(target_y) - bot.y));
        }

        let guard = state.lock().expect("state");
        let bot = guard.bots.first().expect("configured bot");
        assert!(
            minimum_distance <= super::bots::BOT_COMBAT_PURSUIT_DISTANCE,
            "bot must route around the retained Chaos wall into its firing envelope: initial={initial_distance} minimum={minimum_distance} final=({}, {})",
            bot.x,
            bot.y,
        );
    }

    #[test]
    fn server_bot_pursues_a_safe_human_for_presentation_without_firing() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let (safe_x, safe_y) = safe_tile_center(&state.lock().expect("state"));
        let (bot_x, bot_y) = {
            let guard = state.lock().expect("state");
            let map = guard.zone_map();
            (32_i32..=96)
                .flat_map(|distance| [(0, -distance), (distance, 0), (0, distance), (-distance, 0)])
                .map(|(dx, dy)| {
                    (
                        (i32::from(safe_x) + dx * 16).clamp(8, i32::from(u16::MAX) - 8) as u16,
                        (i32::from(safe_y) + dy * 16).clamp(8, i32::from(u16::MAX) - 8) as u16,
                    )
                })
                .find(|&(x, y)| {
                    !crate::zone::world_position_in_safe_tile_for_map(map, x, y)
                        && crate::zone::tile_at_world(map, x, y) == 0
                })
                .expect("Chaos must have a nonsafe presentation approach around its spawn")
        };
        let initial_distance =
            (f32::from(bot_x) - f32::from(safe_x)).hypot(f32::from(bot_y) - f32::from(safe_y));
        assert!(initial_distance >= 512.0);
        {
            let mut guard = state.lock().expect("state");
            let player_position =
                large_position_from_client(alpha, &position_packet_at(1, safe_x, safe_y, 0))
                    .expect("safe player position should encode");
            guard.set_client_position(alpha, player_position);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = f32::from(bot_x);
            bot.y = f32::from(bot_y);
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.last_fire_tick = None;
        }

        let mut settled_positions = Vec::new();
        // Six hundred callbacks of the configured 60-Hz driver are ten
        // seconds, not six hundred centiseconds. Keep a live stationary
        // pilot's authority fresh throughout that interval.
        for callback in 1..=600 {
            let tick = callback * 100 / u32::from(ServerConfig::default().tick_rate_hz);
            state
                .lock()
                .expect("state")
                .clients
                .get_mut(&alpha)
                .expect("pilot")
                .last_position_server_tick = Some(tick);
            super::bots::step_server_bots(&state, tick);
            if callback >= 400 && callback % 10 == 0 {
                let guard = state.lock().expect("state");
                let bot = guard.bots.first().expect("configured bot");
                settled_positions.push((bot.x, bot.y));
            }
        }

        let guard = state.lock().expect("state");
        let bot = guard.bots.first().expect("configured bot");
        let final_distance = (bot.x - f32::from(safe_x)).hypot(bot.y - f32::from(safe_y));
        assert!(
            final_distance + 96.0 < initial_distance,
            "safe pilot should pull the bot into a visible encounter without leaving protection: initial={initial_distance} final={final_distance} bot=({}, {})",
            bot.x,
            bot.y,
        );
        assert!(
            final_distance <= 384.0,
            "safe-pilot navigation must converge to the ordinary 320px pursuit envelope rather than orbit away: initial={initial_distance} final={final_distance} bot=({}, {})",
            bot.x,
            bot.y,
        );
        let settled_origin = settled_positions
            .first()
            .copied()
            .expect("safe-pilot orbit samples");
        let settled_displacement = settled_positions
            .iter()
            .map(|&(x, y)| (x - settled_origin.0).hypot(y - settled_origin.1))
            .fold(0.0_f32, f32::max);
        assert!(
            settled_displacement >= 64.0,
            "protected pilot must retain a material four-tile presentation patrol after convergence instead of leaving the bot rotating in place: displacement={settled_displacement} samples={settled_positions:?}",
        );
        assert!(guard.active_projectiles.is_empty());
        assert!(bot.last_fire_tick.is_none());
    }

    #[test]
    fn server_bot_aim_phase_brakes_inside_the_human_encounter_envelope() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let (target_x, target_y) = {
            let guard = state.lock().expect("state");
            let map = guard.zone_map();
            (16_u16..1008)
                .flat_map(|tile_y| (16_u16..1008).map(move |tile_x| (tile_x, tile_y)))
                .map(|(tile_x, tile_y)| (tile_x * 16 + 8, tile_y * 16 + 8))
                .find(|&(x, y)| {
                    !crate::zone::world_position_in_safe_tile_for_map(map, x, y)
                        && [-16_i32, 0, 16].into_iter().all(|offset_y| {
                            [-16_i32, 0, 16].into_iter().all(|offset_x| {
                                crate::zone::tile_at_world(
                                    map,
                                    (i32::from(x) + offset_x) as u16,
                                    (i32::from(y) + offset_y) as u16,
                                ) == 0
                            })
                        })
                })
                .expect("Chaos must have a clear nonsafe combat tile")
        };
        let initial_speed = {
            let mut guard = state.lock().expect("state");
            let player_position =
                large_position_from_client(alpha, &position_packet_at(1, target_x, target_y, 0))
                    .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = f32::from(target_x);
            bot.y = f32::from(target_y);
            bot.velocity_x = bot.top_speed_pixels_per_second;
            bot.velocity_y = 0.0;
            bot.last_fire_tick = None;
            bot.velocity_x
        };

        // Bot 768 is in its deterministic aim phase at tick 1. The old path
        // stopped thrust but retained full cruise speed for the entire phase,
        // carrying an on-screen bot through and beyond the camera.
        super::bots::step_server_bots(&state, 1);

        let guard = state.lock().expect("state");
        let bot = guard.bots.first().expect("configured bot");
        let final_speed = bot.velocity_x.hypot(bot.velocity_y);
        let expected_speed = initial_speed * 0.96_f32.powf(0.01 * 60.0);
        assert!(
            (final_speed - expected_speed).abs() < 0.0001 && final_speed < initial_speed,
            "aim-lock inside the encounter envelope must brake instead of coasting: initial={initial_speed} final={final_speed}"
        );
        assert!(
            (bot.x - f32::from(target_x)).hypot(bot.y - f32::from(target_y)) < initial_speed / 30.0,
            "one aim tick must remain locally continuous near the pilot"
        );
    }

    #[test]
    fn server_bot_aim_phase_closes_visible_gap_before_braking() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let (target_x, target_y) = {
            let guard = state.lock().expect("state");
            let map = guard.zone_map();
            (16_u16..1008)
                .flat_map(|tile_y| (16_u16..1008).map(move |tile_x| (tile_x, tile_y)))
                .map(|(tile_x, tile_y)| (tile_x * 16 + 8, tile_y * 16 + 8))
                .find(|&(x, y)| {
                    let bot_y = y.saturating_add(240);
                    !crate::zone::world_position_in_safe_tile_for_map(map, x, y)
                        && !crate::zone::world_position_in_safe_tile_for_map(map, x, bot_y)
                        && (y..=bot_y).step_by(4).all(|sample_y| {
                            [-16_i32, 0, 16].into_iter().all(|offset_x| {
                                crate::zone::tile_at_world(
                                    map,
                                    (i32::from(x) + offset_x) as u16,
                                    sample_y,
                                ) == 0
                            })
                        })
                })
                .expect("Chaos must have a clear 240px combat approach lane")
        };
        {
            let mut guard = state.lock().expect("state");
            let player_position =
                large_position_from_client(alpha, &position_packet_at(1, target_x, target_y, 0))
                    .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = f32::from(target_x);
            bot.y = f32::from(target_y) + 240.0;
            bot.orientation = 0.0;
            bot.direction = 0;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.last_fire_tick = None;
        }

        super::bots::step_server_bots(&state, 1);

        let guard = state.lock().expect("state");
        let bot = guard.bots.first().expect("configured bot");
        assert!(
            bot.velocity_y < 0.0,
            "aim phase must thrust toward a visible target"
        );
        assert!(
            bot.y < f32::from(target_y) + 240.0,
            "aim phase must close the visible gap before applying its close-range brake"
        );
    }

    #[test]
    fn server_bot_fires_authoritative_projectile_at_hostile_player() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let (bot_x, bot_y, bot_id, bot_life) = {
            let guard = state.lock().expect("state");
            let bot = &guard.bots[0];
            (bot.x as u16, bot.y as u16, bot.player.id, bot.life_seq)
        };
        {
            let mut guard = state.lock().expect("state");
            let player_position = large_position_from_client(
                alpha,
                &position_packet_at(1, bot_x, bot_y.saturating_sub(300), 0),
            )
            .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.direction = 0;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.last_fire_tick = None;
            open_bot_life_threat_window(&mut guard, alpha, bot_id, bot_life, 1);
        }

        run_server_tick_at_tick(&state, Instant::now(), 1);

        let guard = state.lock().expect("state");
        assert!(guard.active_projectiles.iter().any(|projectile| {
            projectile.owner_id == bot_id && projectile.owner_life_seq == bot_life
        }));
        assert!(guard.bots[0].last_fire_tick.is_some());
    }

    #[test]
    fn server_bot_aim_lock_defeats_stationary_hostile_recharge() {
        // This test shares the process-wide Continuum clock with the rest of
        // the server suite. Anchor every synthetic tick to its current value
        // so lifecycle/projectile deadlines created by `join_arena` remain on
        // the same wrapping timeline even when this test runs late in the
        // parallel suite.
        let start_tick = crate::wire::current_tick();
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let (bot_x, bot_y, bot_id, bot_life) = {
            let guard = state.lock().expect("state");
            let map = guard.zone_map();
            let mut clear_line = None;
            'tiles: for tile_y in 16_u16..1008 {
                for tile_x in 16_u16..1008 {
                    let x = tile_x * 16 + 8;
                    let y = tile_y * 16 + 8;
                    let target_y = y.saturating_sub(100);
                    if crate::zone::world_position_in_safe_tile_for_map(map, x, y)
                        || crate::zone::world_position_in_safe_tile_for_map(map, x, target_y)
                    {
                        continue;
                    }
                    let clear = (target_y..=y).step_by(4).all(|sample_y| {
                        [-16_i32, 0, 16].into_iter().all(|offset_x| {
                            crate::zone::tile_at_world(
                                map,
                                (i32::from(x) + offset_x).clamp(0, i32::from(u16::MAX)) as u16,
                                sample_y,
                            ) == 0
                        })
                    });
                    if clear {
                        clear_line = Some((x, y));
                        break 'tiles;
                    }
                }
            }
            let (bot_x, bot_y) =
                clear_line.expect("Chaos must contain a clear nonsafe bot firing lane");
            let bot = guard.bots.first().expect("configured bot");
            (bot_x, bot_y, bot.player.id, bot.life_seq)
        };
        {
            let mut guard = state.lock().expect("state");
            let player_position = large_position_from_client(
                alpha,
                &position_packet_at(1, bot_x, bot_y.saturating_sub(100), 0),
            )
            .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            // Keep the authored recharge rate, but begin from a damaged
            // combat state so the aim-lock proof terminates after the first
            // repeated-hit sequence. Full-energy lethality remains covered
            // by the real-browser replay without making this unit test
            // depend on long-lived projectile geometry.
            let target = guard.clients.get_mut(&alpha).expect("joined target");
            target.server_energy = 600;
            append_stationary_test_motion(target, start_tick, bot_x, bot_y.saturating_sub(100));
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = f32::from(bot_x);
            bot.y = f32::from(bot_y);
            bot.orientation = 0.0;
            bot.direction = 0;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            // Isolate aim/fire authority from the independently covered
            // navigation controller. A fixed hostile lane makes this proof
            // immune to unrelated test-thread sequence interleaving.
            bot.thrust_pixels_per_second_squared = 0.0;
            bot.top_speed_pixels_per_second = 0.0;
            bot.last_fire_tick = None;
            bot.last_step_tick = start_tick;
            guard.next_hidden_prize_spawn_at_tick =
                Some(crate::continuum::continuum_tick_add(start_tick, 10_000));
            open_bot_life_threat_window(&mut guard, alpha, bot_id, bot_life, start_tick);
        }

        let mut death_tick = None;
        // The playable AI cadence intentionally gives a human reaction
        // window; retain a strict 30-second bound for eventual lethality.
        for offset in 1..=1_800 {
            let tick = crate::continuum::continuum_tick_add(start_tick, offset);
            let player_position = large_position_from_client(
                alpha,
                &position_packet_at(tick, bot_x, bot_y.saturating_sub(100), 0),
            )
            .expect("player position should encode");
            {
                let mut guard = state.lock().expect("state");
                guard.set_client_position(alpha, player_position);
                let target = guard.clients.get_mut(&alpha).expect("joined target");
                append_stationary_test_motion(target, tick, bot_x, bot_y.saturating_sub(100));
            }
            run_server_tick_at_tick(&state, Instant::now(), tick);
            {
                let mut guard = state.lock().expect("state");
                open_active_projectile_threat_windows(&mut guard, alpha, bot_id, tick);
            }
            if !state.lock().expect("state").clients[&alpha].alive {
                death_tick = Some(tick);
                break;
            }
        }

        let guard = state.lock().expect("state");
        assert!(
            death_tick.is_some(),
            "a continuously engaged bot must overcome authored Chaos recharge: damage_seq={} energy={} projectiles={} last_fire={:?}",
            guard.next_damage_seq,
            guard.clients[&alpha].server_energy,
            guard.active_projectiles.len(),
            guard.bots[0].last_fire_tick,
        );
        assert!(guard.next_damage_seq >= 2);
        assert!(guard.clients[&alpha].server_energy <= 0);
    }

    #[test]
    fn server_bot_does_not_fire_from_and_navigates_out_of_an_arena_safe_tile() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let (safe_x, safe_y) = safe_tile_center(&state.lock().expect("state"));
        {
            let mut guard = state.lock().expect("state");
            let player_position = large_position_from_client(
                alpha,
                &position_packet_at(1, safe_x, safe_y.saturating_sub(300), 0),
            )
            .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = f32::from(safe_x);
            bot.y = f32::from(safe_y);
            bot.orientation = 0.0;
            bot.direction = 0;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.last_fire_tick = None;
        }

        run_server_tick_at_tick(&state, Instant::now(), 1);

        {
            let guard = state.lock().expect("state");
            assert!(guard.active_projectiles.is_empty());
            assert!(guard.bots[0].last_fire_tick.is_none());
        }
        for tick in 2..=300 {
            run_server_tick_at_tick(&state, Instant::now(), tick);
        }
        let guard = state.lock().expect("state");
        let bot = &guard.bots[0];
        assert!(!crate::zone::world_position_in_safe_tile_for_map(
            guard.zone_map(),
            bot.x.round() as u16,
            bot.y.round() as u16,
        ));
    }

    #[test]
    fn extreme_bot_escapes_the_retained_doomsy_safe_wall_corner_before_combat() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                default_zone: "extreme".to_string(),
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        {
            let mut guard = state.lock().expect("state");
            let player_position =
                large_position_from_client(alpha, &position_packet_at(1, 8_700, 7_900, 0))
                    .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            let map = guard.zone_map();
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = 8_376.0;
            bot.y = 8_386.0;
            bot.orientation = 0.0;
            bot.direction = 0;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.safe_egress_path.clear();
            assert!(crate::zone::world_position_in_safe_tile_for_map(
                map,
                bot.x as u16,
                bot.y as u16,
            ));
        }

        for tick in 1..=600 {
            run_server_tick_at_tick(&state, Instant::now(), tick);
            let guard = state.lock().expect("state");
            let bot = &guard.bots[0];
            if !crate::zone::world_position_in_safe_tile_for_map(
                guard.zone_map(),
                bot.x.round() as u16,
                bot.y.round() as u16,
            ) {
                drop(guard);
                run_server_tick_at_tick(&state, Instant::now(), tick + 1);
                assert!(
                    state.lock().expect("state").bots[0]
                        .safe_egress_path
                        .is_empty()
                );
                return;
            }
        }
        let guard = state.lock().expect("state");
        let bot = &guard.bots[0];
        panic!(
            "Extreme bot remained invulnerable in safety: x={} y={} velocity=({}, {}) path={:?}",
            bot.x, bot.y, bot.velocity_x, bot.velocity_y, bot.safe_egress_path
        );
    }

    #[test]
    fn extreme_bot_follows_the_safe_wall_instead_of_oscillating_in_place() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                default_zone: "extreme".to_string(),
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        let start = (8_431.5_f32, 8_298.0_f32);
        {
            let mut guard = state.lock().expect("state");
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.x = start.0;
            bot.y = start.1;
            bot.velocity_x = 1.23;
            bot.velocity_y = -0.83;
            bot.orientation = 0.0;
            bot.direction = 0;
            bot.last_step_tick = 0;
            assert!(!crate::zone::world_position_in_safe_tile_for_map(
                guard.zone_map(),
                start.0.round() as u16,
                start.1.round() as u16,
            ));
        }

        let mut min_y = start.1;
        let mut max_y = start.1;
        let mut follow_flips = 0;
        let mut previous_follow_y = 0;
        let mut samples = Vec::new();
        for tick in 1..=600 {
            run_server_tick_at_tick(&state, Instant::now(), tick);
            let mut guard = state.lock().expect("state");
            let door_seeds = guard.server_door_seeds(tick);
            let bricks = guard
                .active_bricks
                .iter()
                .flat_map(|brick| brick.tiles.iter().copied())
                .collect();
            let bot = &guard.bots[0];
            min_y = min_y.min(bot.y);
            max_y = max_y.max(bot.y);
            if bot.wall_follow_y != 0
                && previous_follow_y != 0
                && bot.wall_follow_y != previous_follow_y
            {
                follow_flips += 1;
            }
            if bot.wall_follow_y != 0 {
                previous_follow_y = bot.wall_follow_y;
            }
            if tick % 60 == 0 {
                samples.push((
                    tick,
                    bot.x,
                    bot.y,
                    bot.velocity_x,
                    bot.velocity_y,
                    bot.wall_follow_y,
                ));
            }
            assert!(super::bots::bot_hull_is_clear_for_test(
                bot,
                guard.zone_map(),
                &door_seeds,
                &bricks,
            ));
        }

        let guard = state.lock().expect("state");
        let bot = &guard.bots[0];
        let displacement = (bot.x - start.0).hypot(bot.y - start.1);
        assert!(
            displacement >= 96.0,
            "retained Extreme wall fixture must move materially instead of oscillating: start={start:?} end=({}, {}) velocity=({}, {}) follow=({}, {}) deadline={:?} range_y={min_y}..{max_y} flips={follow_flips} samples={samples:?} displacement={displacement}",
            bot.x,
            bot.y,
            bot.velocity_x,
            bot.velocity_y,
            bot.wall_follow_x,
            bot.wall_follow_y,
            bot.wall_follow_until_tick,
        );
    }

    #[test]
    fn authoritative_projectiles_do_not_target_a_player_in_an_arena_safe_tile() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx_shooter, _rx_shooter) = mpsc::channel();
        let shooter = register_connection(&state, tx_shooter);
        join_arena(&state, shooter, 0);
        let (tx_victim, _rx_victim) = mpsc::channel();
        let victim = register_connection(&state, tx_victim);
        join_arena(&state, victim, 0);
        let tick = 10_000;
        let (safe_x, safe_y) = safe_tile_center(&state.lock().expect("state"));
        {
            let mut guard = state.lock().expect("state");
            configure_damage_test_client(
                &mut guard,
                shooter,
                tick,
                safe_x.saturating_add(200),
                safe_y,
                1,
                1_700,
            );
            configure_damage_test_client(&mut guard, victim, tick, safe_x, safe_y, 2, 1_700);
            let shooter_life = guard.clients[&shooter].life_seq;
            let projectile =
                damage_test_projectile(shooter, shooter_life, 77, tick, 1, safe_x, safe_y);
            guard.record_projectile(projectile);
        }

        run_server_tick_at_tick(&state, Instant::now(), tick);

        let guard = state.lock().expect("state");
        assert_eq!(guard.clients[&victim].server_energy, 1_700);
        assert_eq!(guard.next_damage_seq, 0);
        assert!(guard.pending_damage_transactions.is_empty());
    }

    #[test]
    fn server_bot_claims_hidden_prize_and_applies_authored_loadout_effect() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (bot_id, x, y, multifire_before, bounty_before) = {
            let mut guard = state.lock().expect("state");
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.energy = 1;
            // Exercise acquisition after loss, even with a fully upgraded spawn.
            bot.weapons.multifire_capable = false;
            (
                bot.player.id,
                bot.x.round() as u16,
                bot.y.round() as u16,
                bot.weapons.multifire_capable,
                bot.bounty,
            )
        };
        state
            .lock()
            .expect("state")
            .active_hidden_prizes
            .push(ServerHiddenPrize {
                collectible_id: 900,
                x,
                y,
                prize_id: 15,
                expires_at_tick: 1_000,
            });

        run_server_tick_at_tick(&state, Instant::now(), 2);

        let guard = state.lock().expect("state");
        let bot = guard.bot(bot_id).expect("bot remains rostered");
        assert!(guard.active_hidden_prizes.is_empty());
        assert_eq!(bot.weapons.multifire_capable, !multifire_before);
        assert_eq!(bot.bounty, bounty_before.saturating_add(1));
    }

    #[test]
    fn server_bot_respawns_with_new_life_and_authored_energy() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let bot_id = {
            let mut guard = state.lock().expect("state");
            let bot = guard.bots.first_mut().expect("configured bot");
            bot.advance_life();
            bot.energy = 0;
            bot.respawn_at_tick = Some(10);
            bot.player.id
        };

        run_server_tick_at_tick(&state, Instant::now(), 10);

        let guard = state.lock().expect("state");
        let bot = guard.bot(bot_id).expect("bot remains rostered");
        assert!(bot.alive());
        assert_eq!(bot.life_seq, 2);
        assert_eq!(bot.energy, bot.max_energy);
        assert_eq!(
            bot.energy,
            crate::arena_settings::arena_ship_initial_energy(bot.player.ship)
        );
    }

    #[test]
    fn authoritative_snapshot_aoi_carries_bot_life_energy_and_bounty() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, _rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        {
            let mut guard = state.lock().expect("state");
            let (x, y) = (guard.bots[0].x as u16, guard.bots[0].y as u16);
            let player_position = large_position_from_client(
                alpha,
                &position_packet_at(1, x.saturating_add(20), y, 0),
            )
            .expect("player position should encode");
            guard.set_client_position(alpha, player_position);
            guard
                .clients
                .get_mut(&alpha)
                .expect("client")
                .snapshot_ready = true;
        }
        let guard = state.lock().expect("state");
        let bot = &guard.bots[0];
        let (near, _) = crate::snapshot::compute_aoi_entities(&guard, alpha);
        let entity = near
            .iter()
            .find_map(|(id, entity)| (*id == bot.player.id).then_some(entity))
            .expect("near bot snapshot");
        assert_eq!(entity.life_seq, bot.life_seq);
        assert_eq!(entity.energy, bot.energy);
        assert_eq!(entity.bounty, bot.bounty);
    }

    #[test]
    fn server_tick_clamps_configured_bot_velocity_to_arena_ship_top_speed() {
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                bot_count: 1,
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx_alpha, rx_alpha) = mpsc::channel();
        let alpha = register_connection(&state, tx_alpha);
        join_arena(&state, alpha, 0);
        drain_available(&rx_alpha);

        let center = {
            let state = state.lock().expect("state");
            crate::zone::server_spawn_coordinates(state.zone_map())
        };
        {
            let mut state = state.lock().expect("state");
            let bot = state.bots.first_mut().expect("configured bot should exist");
            let max_speed = arena_ship_max_speed_pixels_per_second(bot.player.ship);
            bot.x = center.0 as f32;
            bot.y = center.1 as f32;
            bot.velocity_x = max_speed * 4.0;
            bot.velocity_y = 0.0;
        }

        // Advance a logical tick explicitly; a fast test can otherwise run
        // inside birth tick zero, where duplicate-step idempotence is correct.
        run_server_tick_at_tick(&state, Instant::now(), 1);

        let state = state.lock().expect("state");
        let bot = state.bots.first().expect("configured bot should exist");
        let max_speed = arena_ship_max_speed_pixels_per_second(bot.player.ship);
        let speed = (bot.velocity_x * bot.velocity_x + bot.velocity_y * bot.velocity_y).sqrt();
        assert!(speed <= max_speed + 0.01);
    }

    fn projectile(owner_id: u16, created_tick: u32) -> ServerProjectile {
        ServerProjectile {
            id: 1,
            shot_group_id: 1,
            shot_index: 0,
            owner_id,
            owner_life_seq: 1,
            combat_timeline: None,
            presentation_source: None,
            weapon_data: 2,
            x: 500,
            y: 700,
            timestamp: 0x1122_3344,
            created_tick,
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: None,
            pos_x: 500.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 700.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        }
    }

    fn position_packet(timestamp: u32) -> Vec<u8> {
        position_packet_with_weapon(timestamp, 0)
    }

    #[test]
    fn position_fixture_sequences_are_isolated_from_parallel_tests() {
        let sequence = |packet: &[u8]| {
            crate::continuum_position::parse_client_position_snapshot(packet)
                .unwrap()
                .client_sequence
                .unwrap()
        };
        let first = sequence(&position_packet(0));
        std::thread::spawn(|| {
            for _ in 0..128 {
                position_packet(0);
            }
        })
        .join()
        .unwrap();
        assert_eq!(sequence(&position_packet(0)), first.wrapping_add(1));
    }

    fn position_packet_with_weapon(timestamp: u32, weapon_data: u16) -> Vec<u8> {
        position_packet_at(timestamp, 500, 700, weapon_data)
    }

    fn position_packet_at(timestamp: u32, x: u16, y: u16, weapon_data: u16) -> Vec<u8> {
        let mut packet = vec![0; 22];
        packet[0] = crate::arena::CLIENT_POSITION;
        packet[2..6].copy_from_slice(&timestamp.to_le_bytes());
        packet[8..10].copy_from_slice(&y.to_le_bytes());
        packet[12..14].copy_from_slice(&x.to_le_bytes());
        packet[18..20].copy_from_slice(&1_000_u16.to_le_bytes());
        packet[20..22].copy_from_slice(&weapon_data.to_le_bytes());
        packet.resize(crate::continuum_position::CLIENT_POSITION_EXTRA_LEN, 0);
        let sequence = NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| {
            let sequence = next.get();
            next.set(sequence.wrapping_add(1));
            sequence
        });
        packet[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
        );
        crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(0, x, y, 0, 0)
            .encode(&mut packet);
        packet
    }

    /// The server-tick fixtures enqueue already-decoded position work rather
    /// than passing through the real input-record decoder. Seed the exact
    /// receipt decision that a real connection would have recorded; focused
    /// receipt and browser tests exercise production verification itself.
    fn seed_weapon_combat_view_decision(
        state: &mut ServerState,
        player_id: u16,
        packet: &[u8],
        server_tick: u32,
    ) {
        let snapshot = crate::continuum_position::parse_client_position_snapshot(packet)
            .expect("weapon position fixture must parse");
        if snapshot.weapon == 0 {
            return;
        }
        let sequence = snapshot
            .client_sequence
            .expect("current weapon fixture carries an input sequence");
        state
            .clients
            .get_mut(&player_id)
            .expect("weapon fixture client")
            .observer_combat_timeline
            .seed_input_decision_for_test(sequence, server_tick)
            .expect("joined weapon fixture owns an exact combat-view decision");
    }

    fn drain_available(rx: &mpsc::Receiver<Vec<u8>>) {
        while rx.try_recv().is_ok() {}
    }

    fn synchronize_test_position_exact_state(packet: &mut Vec<u8>) {
        let exact = crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            packet[1],
            u16::from_le_bytes(packet[12..14].try_into().expect("x")),
            u16::from_le_bytes(packet[8..10].try_into().expect("y")),
            i16::from_le_bytes(packet[6..8].try_into().expect("vx")),
            i16::from_le_bytes(packet[14..16].try_into().expect("vy")),
        );
        let offset = crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_OFFSET;
        let end = crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET;
        let mut encoded = Vec::new();
        exact.encode(&mut encoded);
        packet[offset..end].copy_from_slice(&encoded);
    }
}
