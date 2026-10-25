use std::sync::{Arc, Mutex};

use crate::arena::AcceptedPositionRelay;
use crate::authority_projectiles::ProjectileRecord;
use crate::continuum_position::{
    canonical_discrete_heading_components, protocol_direction, protocol_direction_to_orientation,
};
use crate::hidden_prizes::remove_active_collectible;
use crate::shared_player_movement::{
    CollisionAxis, MovementVector, integrate_player_collision, movement_axis_overlaps_blocked_tile,
};
use crate::snapshot::{RealtimeEventDelivery, RealtimeEventPressureDisposition};
use crate::state::{ServerPrizeClaim, ServerState};
use crate::zone::{
    MAP_TILES, TILE_SIZE_PX, server_spawn_coordinates, world_position_in_safe_tile_for_map,
};

#[path = "bots/tactics.rs"]
mod tactics;
use tactics::{
    bot_bomb_safety_blocked, bot_combat_weapon, bot_fire_ready, bot_launch_velocity,
    bot_reaction_ready, bot_tactical_orbit_radius,
};

// Continuum presents 40 discrete headings (9 degrees apart). Wait until the
// nearest heading is within roughly half a step of the target so an emitted
// shot is credible against a ship-sized hull instead of merely facing the
// correct quadrant.
const BOT_AIM_ALIGNMENT: f32 = 0.995;
const BOT_COMBAT_ORBIT_RADIUS: f32 = 192.0;
const BOT_COMBAT_ORBIT_TURNS_PER_SECOND: f64 = 0.075;
const BOT_COMBAT_AIM_TICKS_PER_CYCLE: u32 = 120;
const BOT_COMBAT_CYCLE_TICKS: u32 = 180;
pub(super) const BOT_COMBAT_PURSUIT_DISTANCE: f32 = 320.0;
const BOT_COMBAT_AIM_STOP_DISTANCE: f32 = 96.0;
// Server bots have deterministic near-perfect aim during their aim phase.
// A small delay multiplier leaves a reaction window, while the one-bot-per-
// human assignment prevents focus fire. Four times the authored delay let
// Extreme recharge completely between sparse hits, making reciprocal combat
// nonlethal even during a sustained close-range engagement.
const BOT_FIRE_COOLDOWN_MULTIPLIER: u32 = 2;
const BOT_NAVIGATION_STOP_DISTANCE: f32 = 12.0;
const BOT_PRIZE_ENERGY_FRACTION: f32 = 0.65;
const BOT_PRIZE_SEEK_RADIUS: f32 = 640.0;
const BOT_HUMAN_TARGET_STALE_SECONDS: u32 = 2;
const BOT_MOVEMENT_TRACE_INTERVAL_TICKS: u32 = 5;

fn bot_movement_trace_due(
    movement_sequence: u16,
    bot_id: u16,
    collision_x: bool,
    collision_y: bool,
    navigation_avoiding_wall: bool,
    hull_clear: bool,
) -> bool {
    collision_x
        || collision_y
        || navigation_avoiding_wall
        || !hull_clear
        || u32::from(movement_sequence.wrapping_add(bot_id)) % BOT_MOVEMENT_TRACE_INTERVAL_TICKS
            == 0
}

#[derive(Default)]
pub(super) struct ServerBotTick {
    pub(super) positions: Vec<AcceptedPositionRelay>,
    pub(super) reliable_packets: Vec<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct BotTarget {
    player_id: Option<u16>,
    frequency: Option<u16>,
    x: f32,
    y: f32,
    velocity_x: f32,
    velocity_y: f32,
    in_safe_zone: bool,
    status: u8,
}

#[derive(Clone, Copy)]
struct PendingBotFire {
    bot_id: u16,
    life_seq: u32,
    source_runtime: crate::authority_identity::PlayerRuntimeIdentity,
    /// Human target whose exact per-recipient contract must be open. Bots have
    /// no remote-browser presentation boundary and therefore use `None`.
    target_human_player_id: Option<u16>,
    x: f32,
    y: f32,
    velocity_x: f32,
    velocity_y: f32,
    orientation: f32,
    direction: u8,
    weapon_data: u16,
}

pub(super) fn step_server_bots(
    shared: &Arc<Mutex<ServerState>>,
    server_tick: u32,
) -> ServerBotTick {
    let mut state = shared.lock().expect("server state should not be poisoned");
    if state.bots.is_empty() {
        return ServerBotTick::default();
    }
    let center = server_spawn_coordinates(state.zone_map());
    let map = state.zone_map();
    let door_seeds = state.server_door_seeds(server_tick);
    let blocking_bricks = state
        .active_bricks
        .iter()
        .flat_map(|brick| brick.tiles.iter().copied())
        .collect::<std::collections::BTreeSet<_>>();
    // A closed door can engulf a hull between two logical steps. Pick a
    // bounded, verified server destination before borrowing the bot roster.
    // Repeated driver callbacks must not consume entropy or relocate twice.
    let door_relocations = state
        .bots
        .iter()
        .filter(|bot| {
            bot.alive()
                && server_tick != bot.last_step_tick
                && bot_overlaps_closed_door(bot, map, &door_seeds)
        })
        .map(|bot| {
            let destination = crate::objectives::brick_overlap_destination(
                &state,
                bot.player.frequency,
                bot.player.ship,
                server_tick,
            )
            .filter(|&(x, y)| {
                bot_hull_is_clear(
                    f32::from(x),
                    f32::from(y),
                    crate::arena_settings::arena_ship_radius_pixels(bot.player.ship),
                    map,
                    &door_seeds,
                    &blocking_bricks,
                )
            });
            (bot.player.id, destination)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut human_targets = state
        .clients
        .iter()
        .filter(|(_, client)| client.joined && client.alive && client.player.ship < 8)
        .filter(|(_, client)| human_target_is_fresh(client.last_position_server_tick, server_tick))
        .filter_map(|(&player_id, client)| {
            client
                .last_position
                .as_deref()
                .and_then(crate::arena::large_position_coordinates_and_velocity)
                .map(|((x, y), (velocity_x, velocity_y))| BotTarget {
                    player_id: Some(player_id),
                    frequency: Some(client.player.frequency),
                    x: f32::from(x),
                    y: f32::from(y),
                    velocity_x: f32::from(velocity_x) / 10.0,
                    velocity_y: f32::from(velocity_y) / 10.0,
                    in_safe_zone: world_position_in_safe_tile_for_map(map, x, y),
                    status: client
                        .last_position
                        .as_deref()
                        .and_then(|position| position.get(13))
                        .copied()
                        .unwrap_or_default(),
                })
        })
        .collect::<Vec<_>>();
    human_targets.sort_unstable_by_key(|target| target.player_id);
    // Rotate the hunter roster on authoritative human life boundaries. A
    // lone pilot should not draw the same configured bot on every respawn.
    // Status toggles and elapsed time do not reroll an ongoing encounter.
    let human_assignment_rotation = human_assignment_rotation(
        human_targets.iter().filter_map(|target| {
            target
                .player_id
                .and_then(|id| state.clients.get(&id))
                .map(|client| client.life_seq)
        }),
        state.bots.len(),
    );
    let bot_count = state.bots.len();
    let joined_humans = state
        .clients
        .values()
        .filter(|client| client.joined)
        .count();
    let alive_humans = state
        .clients
        .values()
        .filter(|client| client.joined && client.alive && client.player.ship < 8)
        .count();
    let positioned_humans = state
        .clients
        .values()
        .filter(|client| {
            client.joined
                && client.alive
                && client.player.ship < 8
                && client.last_position.is_some()
        })
        .count();
    let bot_targets = state
        .bots
        .iter()
        .filter(|bot| bot.alive())
        .map(|bot| BotTarget {
            player_id: Some(bot.player.id),
            frequency: Some(bot.player.frequency),
            x: bot.x,
            y: bot.y,
            velocity_x: bot.velocity_x,
            velocity_y: bot.velocity_y,
            in_safe_zone: false,
            status: bot.position_status_bits(),
        })
        .collect::<Vec<_>>();
    // Keep the one-bot-per-human protection as an actual target partition.
    // Previously a surplus bot could target the bot assigned to the human;
    // with the ordinary two-bot roster that made the surplus bot repeatedly
    // kill the only bot capable of returning fire at the pilot. Surplus bots
    // may fight one another, but human-assigned bots belong exclusively to
    // their human encounter while that authority surface is fresh.
    let human_assigned_bot_ids =
        human_assigned_bot_ids(&state.bots, &human_targets, human_assignment_rotation);
    let surplus_bot_targets = surplus_bot_targets(&bot_targets, &human_assigned_bot_ids);
    let prizes = state.active_hidden_prizes.clone();
    let objective_targets = objective_targets(&state);
    let recipient_ids = state
        .clients
        .iter()
        .filter_map(|(&id, client)| client.joined.then_some(id))
        .collect::<Vec<_>>();
    let trace_bot_movement = state.config.sim_observe;
    let mut pending_fires = Vec::new();
    let mut safe_bot_sources = Vec::new();
    let mut pending_prize_claims = Vec::new();
    let mut pending_lifecycle_events = Vec::new();

    for (bot_index, bot) in state.bots.iter_mut().enumerate() {
        bot.maintain_timers(server_tick);
        if let Some(respawn_at_tick) = bot.respawn_at_tick {
            if !crate::continuum::continuum_tick_reached(server_tick, respawn_at_tick) {
                continue;
            }
            bot.respawn_at_tick = None;
            let (spawn_x, spawn_y, tangent_x, tangent_y) =
                bot_respawn_position(bot, bot_index, center, map, &door_seeds, &blocking_bricks);
            bot.x = spawn_x;
            bot.y = spawn_y;
            bot.velocity_x = tangent_x * 60.0;
            bot.velocity_y = tangent_y * 60.0;
            bot.reset_loadout_at_tick(server_tick);
            bot.begin_actor_motion_segment();
            pending_lifecycle_events.push(serde_json::json!({
                "type": "server_actor_respawn_committed",
                "actorKind": "bot",
                "serverTick": server_tick,
                "playerId": bot.player.id,
                "sessionId": bot.session_id_for_wire().to_string(),
                "lifeSeq": bot.life_seq,
                "x": bot.x,
                "y": bot.y,
                "velocityX": bot.velocity_x,
                "velocityY": bot.velocity_y,
            }));
        }
        let elapsed_ticks =
            crate::continuum::continuum_tick_elapsed(server_tick, bot.last_step_tick);
        // The configured driver may run above the 100 Hz Continuum clock, and
        // a formerly overdue driver could also issue a rapid catch-up frame.
        // A repeated logical tick is wholly idempotent: do not rotate, move,
        // fire, recharge, or append an identical exact-motion sample. The next
        // forward tick publishes the ordinary position/snapshot state.
        if elapsed_ticks == 0 {
            continue;
        }
        // Motion and recharge consume the same logical clock, not callback
        // count. Preserve the existing 100 ms maximum motion turn: a stalled
        // driver must not extrapolate an unobserved multi-second trajectory.
        // This cap does not slow ordinary 20/60/100/120/240 Hz schedules.
        let dt_seconds = elapsed_ticks.min(10) as f32 / 100.0;
        bot.last_step_tick = server_tick;
        bot.advance_energy_at_tick(elapsed_ticks, server_tick);

        if let Some(destination) = door_relocations.get(&bot.player.id) {
            let relocated =
                apply_bot_door_relocation(bot, *destination, map, &door_seeds, &blocking_bricks);
            let safe = world_position_in_safe_tile_for_map(
                map,
                bot.x.round() as u16,
                bot.y.round() as u16,
            );
            let appended = bot.append_actor_motion_sample(server_tick, safe);
            debug_assert_eq!(
                appended,
                crate::combat_timeline::HistoryAppendDisposition::Applied
            );
            if relocated {
                tracing::info!(
                    event = "door_overlap_bot_relocated",
                    player_id = bot.player.id,
                    life_seq = bot.life_seq,
                    server_tick,
                    destination_x = bot.x,
                    destination_y = bot.y
                );
            } else if server_tick / 100 != (server_tick.wrapping_sub(elapsed_ticks)) / 100 {
                tracing::warn!(
                    event = "door_overlap_bot_destination_unavailable",
                    player_id = bot.player.id,
                    life_seq = bot.life_seq,
                    server_tick
                );
            }
            // Never move or fire from an embedded hull, or add drift on the
            // relocation turn. Owned projectiles continue on their own clock.
            continue;
        }

        // A prize chain must never pull the roster away while there is no
        // fresh human authority surface. This matters during slow browser
        // bootstrap as well as between sessions: the scoreboard can already
        // contain bots while the client is still loading the map and has not
        // begun emitting canonical positions.
        let prize_target =
            bot_should_seek_prize(!human_targets.is_empty(), bot.energy, bot.max_energy)
                .then(|| nearest_prize(bot.x, bot.y, &prizes))
                .flatten();
        let bot_in_safe_zone = world_position_in_safe_tile_for_map(
            map,
            bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
        );
        if bot_in_safe_zone {
            safe_bot_sources.push(bot.runtime_identity());
        }
        let e2e_stationary_motion = bot.e2e_stationary_motion_active(bot_in_safe_zone, server_tick);
        let e2e_passive_fire = bot.e2e_passive_fire_active();
        let safe_escape_target = if bot_in_safe_zone {
            next_safe_egress_target(bot, map, &door_seeds, &blocking_bricks)
        } else {
            bot.safe_egress_path.clear();
            None
        };
        let escaping_safe = safe_escape_target.is_some();
        // Assign at most one configured bot to each live human. Letting every
        // bot focus the same pilot produced a deterministic spawn-kill loop:
        // two perfect-aim Extreme bots killed the browser before any of 71
        // genuine primary-fire edges could land. Extra bots still fight one
        // another, so the arena remains active without making a lone pilot
        // the target of the entire configured roster.
        let human_slot = human_assignment_slot(bot_index, bot_count, human_assignment_rotation);
        let assigned_human =
            assigned_hostile_human_slot(human_slot, bot.player.frequency, &human_targets);
        let hostile_human = assigned_hostile_human_target(
            human_slot,
            bot.player.frequency,
            &human_targets,
            bot.xradar,
        );
        // Without a live human authority surface, keep bots near the authored
        // spawn instead of letting an endless bot-versus-bot chase carry the
        // whole roster off-screen before the first pilot arrives.
        let hostile_bot = (!human_targets.is_empty() && assigned_human.is_none())
            .then(|| {
                nearest_hostile_target(
                    bot.player.id,
                    bot.player.frequency,
                    bot.x,
                    bot.y,
                    &[],
                    &surplus_bot_targets,
                    bot.xradar,
                )
            })
            .flatten();
        let target = safe_escape_target
            .or_else(|| {
                objective_targets.iter().copied().min_by(|left, right| {
                    distance_squared(bot.x, bot.y, left.x, left.y)
                        .total_cmp(&distance_squared(bot.x, bot.y, right.x, right.y))
                })
            })
            // Safe tiles protect combat, not presentation. Let bots navigate
            // toward a protected pilot so the encounter is visible and ready
            // when the pilot leaves safety; firing is suppressed below.
            .or(hostile_human)
            .or(prize_target)
            .or(hostile_bot)
            .unwrap_or(BotTarget {
                player_id: None,
                frequency: None,
                x: f32::from(center.0),
                y: f32::from(center.1),
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            });

        // A protected human remains a presentation/navigation target, never a
        // fire target. Approach the exact authority position until the bot is
        // inside the ordinary pursuit envelope, then follow the deterministic
        // orbit. Continuing to aim at the exact protected position after
        // convergence leaves the bot stopped and rotating in place; orbiting
        // immediately from far away can instead pull it away for the whole
        // browser staging window.
        let direct_target_distance = (target.x - bot.x).hypot(target.y - bot.y);
        let protected_target_orbiting = target.player_id.is_some()
            && target.in_safe_zone
            && direct_target_distance <= BOT_COMBAT_PURSUIT_DISTANCE;
        let reaction_ready = bot_reaction_ready(
            bot,
            target.player_id.filter(|_| !target.in_safe_zone),
            server_tick,
        );
        let combat_aiming = target.player_id.is_some()
            && !target.in_safe_zone
            && combat_aim_phase(bot, server_tick);
        let protected_target_approach =
            target.player_id.is_some() && target.in_safe_zone && !protected_target_orbiting;
        // The orbit point is meaningful only after the bot has entered the
        // authored orbit radius. Selecting it from farther away can put that
        // moving waypoint on the far side of the pilot, so the one-second
        // non-aim phase cancels most of the preceding approach and leaves a
        // stationary hostile just outside a repeatable firing lane. Close the
        // real gap first; aim phases still take the exact target all the way
        // to the close-range brake below.
        let hostile_target_approach =
            hostile_target_requires_direct_approach(target, direct_target_distance)
                && direct_target_distance > bot_tactical_orbit_radius(bot);
        let planned_weapon = bot_combat_weapon(bot, target, server_tick, |x, y| {
            bot_tile_is_blocked(
                (x / TILE_SIZE_PX).floor() as i32,
                (y / TILE_SIZE_PX).floor() as i32,
                map,
                &door_seeds,
                &blocking_bricks,
            )
        });
        let steering_target =
            if combat_aiming || protected_target_approach || hostile_target_approach {
                if combat_aiming {
                    planned_weapon.map_or(target, |weapon| {
                        bot_weapon_intercept_target(bot, target, weapon)
                    })
                } else {
                    target
                }
            } else {
                target
                    .player_id
                    .map_or(target, |_| combat_orbit_target(bot, target, server_tick))
            };
        let navigation_dx = steering_target.x - bot.x;
        let navigation_dy = steering_target.y - bot.y;
        let navigation_distance = navigation_dx.hypot(navigation_dy);
        // Continuum direction 0 faces north; orientation advances clockwise.
        let direct_orientation = navigation_dx
            .atan2(-navigation_dy)
            .rem_euclid(std::f32::consts::TAU)
            / std::f32::consts::TAU;
        let direct_steering_orientation = if combat_aiming {
            nearest_protocol_orientation(direct_orientation)
        } else {
            direct_orientation
        };
        let desired_orientation = collision_avoiding_orientation(
            bot,
            direct_orientation,
            combat_aiming,
            map,
            &door_seeds,
            &blocking_bricks,
        );
        let navigation_avoiding_wall =
            desired_orientation.to_bits() != direct_steering_orientation.to_bits();
        let current_orientation = bot.orientation;
        let turn_rate = if bot.shutdown_active(server_tick) {
            bot.rotation_per_second * 0.2
        } else {
            bot.rotation_per_second
        };
        let orientation = turn_towards(
            current_orientation,
            desired_orientation,
            turn_rate * dt_seconds,
        );
        bot.orientation = orientation;
        // At full upgraded speed, retaining cruise velocity while making a
        // large navigation turn sends the bot past wall escape tangents.
        // Brake during the turn; keep authored speed/thrust limits unchanged.
        let turn_remaining =
            ((desired_orientation - orientation + 0.5).rem_euclid(1.0) - 0.5).abs();
        if hostile_target_approach && turn_remaining > 0.125 && !e2e_stationary_motion {
            let brake = 0.92_f32.powf(dt_seconds * 60.0);
            bot.velocity_x *= brake;
            bot.velocity_y *= brake;
        }
        bot.direction = protocol_direction(orientation);
        let (heading_x, heading_y) = canonical_discrete_heading_components(orientation);
        let stop_distance = if escaping_safe {
            8.0
        } else if combat_aiming {
            // Keep closing a visible gap while aiming. Braking anywhere in
            // the full 320px engagement envelope made Extreme bots appear
            // frozen for two thirds of every combat cycle.
            if planned_weapon.is_some_and(|weapon| matches!(weapon & 0x1f, 3 | 4)) {
                bot_tactical_orbit_radius(bot)
            } else {
                BOT_COMBAT_AIM_STOP_DISTANCE
            }
        } else {
            BOT_NAVIGATION_STOP_DISTANCE
        };
        if e2e_stationary_motion {
            if bot.velocity_x != 0.0 || bot.velocity_y != 0.0 {
                tracing::info!(
                    server_tick,
                    bot_id = bot.player.id,
                    life_seq = bot.life_seq,
                    x = bot.x,
                    y = bot.y,
                    "server bot stationary E2E geometry fixture active"
                );
            }
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            bot.wall_follow_until_tick = None;
            bot.wall_follow_x = 0;
            bot.wall_follow_y = 0;
        } else if navigation_distance > stop_distance && !bot.shutdown_active(server_tick) {
            bot.velocity_x += heading_x * bot.thrust_pixels_per_second_squared * dt_seconds;
            bot.velocity_y += heading_y * bot.thrust_pixels_per_second_squared * dt_seconds;
        } else {
            // The long aim phase deliberately stops thrust inside the pursuit
            // envelope. It must also apply ordinary braking: retaining full
            // cruise velocity here made a bot coast straight through its
            // target for up to two seconds, leave the camera/AOI, and look
            // like an instantaneous disappearance when the final snapshot
            // proxy expired.
            bot.velocity_x *= 0.96_f32.powf(dt_seconds * 60.0);
            bot.velocity_y *= 0.96_f32.powf(dt_seconds * 60.0);
        }
        if !e2e_stationary_motion {
            release_bot_wall_following_for_clear_route(
                bot,
                direct_steering_orientation,
                map,
                &door_seeds,
                &blocking_bricks,
            );
            apply_active_bot_wall_following(bot, server_tick);
        }
        clamp_velocity(bot, server_tick);
        let collision =
            move_bot_with_collision(bot, dt_seconds, map, &door_seeds, &blocking_bricks);
        if !e2e_stationary_motion && !bot.shutdown_active(server_tick) {
            apply_bot_wall_following_impulse(
                bot,
                collision,
                server_tick,
                hostile_target_approach.then_some((steering_target.x, steering_target.y)),
            );
            clamp_velocity(bot, server_tick);
        }
        bot.objective_position_sequence = bot.objective_position_sequence.wrapping_add(1);

        let motion_safe = world_position_in_safe_tile_for_map(
            map,
            bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
        );
        let motion_append = bot.append_actor_motion_sample(server_tick, motion_safe);
        debug_assert_eq!(
            motion_append,
            crate::combat_timeline::HistoryAppendDisposition::Applied,
            "a simulated bot tick must advance exact actor history"
        );

        let hull_clear = bot_hull_is_clear(
            bot.x,
            bot.y,
            crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0),
            map,
            &door_seeds,
            &blocking_bricks,
        );
        if trace_bot_movement
            && !recipient_ids.is_empty()
            && bot_movement_trace_due(
                bot.objective_position_sequence,
                bot.player.id,
                collision.collision_x,
                collision.collision_y,
                navigation_avoiding_wall,
                hull_clear,
            )
        {
            tracing::info!(
                server_tick,
                bot_id = bot.player.id,
                life_seq = bot.life_seq,
                bot_xradar_selected = bot.xradar_selected,
                bot_xradar_active = bot.xradar,
                bot_xradar_capable = bot.status.xradar_capable,
                bot_energy = bot.energy,
                bot_recharge_per_second = bot.recharge_per_second,
                bot_xradar_energy_per_second =
                    crate::arena_settings::arena_ship_xradar_energy_per_second(bot.player.ship),
                x = bot.x,
                y = bot.y,
                velocity_x = bot.velocity_x,
                velocity_y = bot.velocity_y,
                speed = bot.velocity_x.hypot(bot.velocity_y),
                attempted_x = collision.attempted_x,
                attempted_y = collision.attempted_y,
                collision_x = collision.collision_x,
                collision_y = collision.collision_y,
                start_tile_x = (collision.start_x / TILE_SIZE_PX).floor() as i32,
                start_tile_y = (collision.start_y / TILE_SIZE_PX).floor() as i32,
                end_tile_x = (bot.x / TILE_SIZE_PX).floor() as i32,
                end_tile_y = (bot.y / TILE_SIZE_PX).floor() as i32,
                hull_clear,
                bot_in_safe_zone = world_position_in_safe_tile_for_map(
                    map,
                    bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                    bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                ),
                target_player_id = target.player_id.unwrap_or(u16::MAX),
                assigned_human_player_id = assigned_human
                    .and_then(|target| target.player_id)
                    .unwrap_or(u16::MAX),
                assigned_human_status = assigned_human.map_or(0, |target| target.status),
                human_assignment_rotation,
                human_assignment_slot = human_slot,
                detected_human_player_id = hostile_human
                    .and_then(|target| target.player_id)
                    .unwrap_or(u16::MAX),
                target_x = target.x,
                target_y = target.y,
                target_in_safe_zone = target.in_safe_zone,
                target_distance = (target.x - bot.x).hypot(target.y - bot.y),
                navigation_distance,
                navigation_avoiding_wall,
                combat_aiming,
                e2e_stationary_motion,
                e2e_passive_fire,
                repel_active = bot.repel_active(server_tick),
                repel_until_tick = bot.repel_until_tick.unwrap_or(0),
                hostile_target_approach,
                fresh_human_targets = human_targets.len(),
                joined_humans,
                alive_humans,
                positioned_humans,
                joined_recipients = recipient_ids.len(),
                center_distance = (bot.x - f32::from(center.0)).hypot(bot.y - f32::from(center.1)),
                "server bot movement trace"
            );
        }

        if let Some(prize) = prizes.iter().find(|prize| bot_overlaps_prize(bot, prize)) {
            pending_prize_claims.push((bot.player.id, prize.collectible_id));
        }

        if e2e_passive_fire {
            continue;
        }

        let Some(mut weapon_data) = bot_combat_weapon(bot, target, server_tick, |x, y| {
            bot_tile_is_blocked(
                (x / TILE_SIZE_PX).floor() as i32,
                (y / TILE_SIZE_PX).floor() as i32,
                map,
                &door_seeds,
                &blocking_bricks,
            )
        }) else {
            continue;
        };
        if bot_bomb_safety_blocked(
            bot,
            weapon_data,
            human_targets
                .iter()
                .chain(&bot_targets)
                .filter_map(|target| {
                    target
                        .frequency
                        .map(|frequency| (frequency, target.x, target.y))
                }),
        ) {
            let Some(gun) = bot.gun_weapon_data() else {
                continue;
            };
            weapon_data = gun;
        }
        // There is no multiplayer presentation or human authority surface to
        // exercise before the first joined observer. Let bots patrol, but do
        // not silently spend lives fighting each other before anyone arrives.
        if recipient_ids.is_empty() {
            continue;
        }
        let bot_x = bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        let bot_y = bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        if world_position_in_safe_tile_for_map(map, bot_x, bot_y) {
            continue;
        }
        let fire_target = bot_weapon_intercept_target(bot, target, weapon_data);
        let target_dx = fire_target.x - bot.x;
        let target_dy = fire_target.y - bot.y;
        let aim_distance = target_dx.hypot(target_dy);
        let target_distance = (target.x - bot.x).hypot(target.y - bot.y);
        if target.player_id.is_none() || target.in_safe_zone || aim_distance <= 1.0 {
            continue;
        }
        let aim = (target_dx / aim_distance) * heading_x + (target_dy / aim_distance) * heading_y;
        let lifetime_ticks =
            crate::arena_settings::arena_weapon_lifetime_ticks(weapon_data).unwrap_or(0);
        let tuning = crate::arena_settings::arena_projectile_initialization_tuning(bot.player.ship);
        let speed = if matches!(weapon_data & 0x1f, 3 | 4) {
            tuning.bomb_speed
        } else {
            tuning.bullet_speed
        };
        let in_range = bot_target_is_in_fire_range(target_distance, speed, lifetime_ticks);
        let ready = bot_fire_ready(bot, server_tick, weapon_data);
        let energy_cost =
            crate::arena_settings::arena_ship_weapon_energy_cost(bot.player.ship, weapon_data);
        let energy =
            crate::authority::EnergyPool::new(bot.energy, bot.max_energy, bot.energy_remainder);
        if in_range
            && reaction_ready
            && aim > BOT_AIM_ALIGNMENT
            && ready
            && !bot.shutdown_active(server_tick)
            && energy.can_spend_weapon_energy(f32::from(energy_cost))
        {
            pending_fires.push(PendingBotFire {
                bot_id: bot.player.id,
                life_seq: bot.life_seq,
                source_runtime: bot.runtime_identity(),
                target_human_player_id: target.player_id.filter(|player_id| {
                    human_targets
                        .iter()
                        .any(|human| human.player_id == Some(*player_id))
                }),
                x: bot.x,
                y: bot.y,
                velocity_x: bot.velocity_x,
                velocity_y: bot.velocity_y,
                orientation,
                direction: bot.direction,
                weapon_data,
            });
        }
    }

    // A bot can begin a respawn step in safety and leave it during motion.
    // Cancel earlier flight before any post-egress firing group is allocated.
    for source in safe_bot_sources {
        super::cancel_owner_weapons_for_admitted_safe_entry(&mut state, server_tick, source, None);
    }
    for event in pending_lifecycle_events {
        state.qa_traces.record_for_all(event);
    }
    let mut output = ServerBotTick::default();
    pending_prize_claims.sort_unstable_by_key(|&(bot_id, collectible_id)| (collectible_id, bot_id));
    pending_prize_claims.dedup_by_key(|claim| claim.1);
    for (bot_id, collectible_id) in pending_prize_claims {
        let Some(prize) = state
            .active_hidden_prizes
            .iter()
            .find(|prize| prize.collectible_id == collectible_id)
            .copied()
        else {
            continue;
        };
        if !remove_active_collectible(&mut state, collectible_id) {
            continue;
        }
        let claim = ServerPrizeClaim {
            collectible_id,
            x: prize.x,
            y: prize.y,
            prize_id: prize.prize_id,
        };
        let spawn = center;
        let mut prize_energy_event = None;
        if let Some(bot) = state.bot_mut(bot_id) {
            let energy_before = bot.energy;
            let warp = bot.apply_prize_at_tick(prize.prize_id, server_tick);
            if warp {
                bot.x = f32::from(spawn.0);
                bot.y = f32::from(spawn.1);
                bot.velocity_x = 0.0;
                bot.velocity_y = 0.0;
                bot.begin_actor_motion_segment();
                let motion_append = bot.append_actor_motion_sample(server_tick, true);
                debug_assert_eq!(
                    motion_append,
                    crate::combat_timeline::HistoryAppendDisposition::Applied,
                    "a bot Warp must establish its new motion segment"
                );
                tracing::info!(
                    event = "bot_warp_prize_relocated",
                    player_id = bot.player.id,
                    life_seq = bot.life_seq,
                    server_tick,
                    destination_x = bot.x,
                    destination_y = bot.y,
                    collectible_id,
                    prize_id = prize.prize_id,
                    "server bot Warp prize relocation"
                );
            }
            prize_energy_event = Some(serde_json::json!({
                "type": "server_bot_prize_committed",
                "serverTick": server_tick,
                "ownerId": bot_id,
                "ownerSessionId": bot.session_id_for_wire().to_string(),
                "ownerLifeSeq": bot.life_seq,
                "collectibleId": collectible_id,
                "prizeId": prize.prize_id,
                "energyBefore": energy_before,
                "energyAfter": bot.energy,
            }));
        }
        if let Some(event) = prize_energy_event {
            state.qa_traces.record_for_all(event);
        }
        output
            .reliable_packets
            .push(crate::player_events::player_prize_from_claim(bot_id, claim));
    }

    for fire in pending_fires {
        spawn_bot_weapon(&mut state, server_tick, fire);
    }
    output
        .reliable_packets
        .extend(crate::objectives::step_server_bot_objectives(
            &mut state,
            server_tick,
        ));
    output
        .positions
        .extend(
            state
                .bots
                .iter()
                .filter(|bot| bot.alive())
                .map(|bot| AcceptedPositionRelay {
                    position: bot.large_position_packet(),
                    recipient_ids: recipient_ids.clone(),
                    // Snapshot near/mid entities are the sole production
                    // presentation stream.  Letting the historical 0x3a
                    // relay escape here routes one bot packet per tick over
                    // the reliable DataChannel (0x3a is deliberately not a
                    // realtime packet).  On a higher-latency path those
                    // cross-channel updates can arrive after a newer AOI
                    // snapshot and resurrect a proxy the snapshot sweep just
                    // cleared, producing rapid visible pop-out/pop-in while
                    // also head-of-line blocking lifecycle traffic.
                    public_relay_suppressed: true,
                    warp_relocation_receipt: None,
                    repel_application_receipt: None,
                    movement_correction: None,
                }),
        );
    output
}

fn spawn_bot_weapon(state: &mut ServerState, tick: u32, mut fire: PendingBotFire) {
    let Some(bot) = state.bot(fire.bot_id) else {
        return;
    };
    if !bot.alive() || bot.life_seq != fire.life_seq {
        return;
    }
    if bot.runtime_identity() != fire.source_runtime {
        return;
    }
    let humans = state
        .clients
        .values()
        .filter(|client| client.joined && client.alive && client.player.ship < 8)
        .filter_map(|client| {
            let (x, y) =
                crate::arena::large_position_coordinates(client.last_position.as_deref()?)?;
            Some((client.player.frequency, f32::from(x), f32::from(y)))
        });
    let bots = state
        .bots
        .iter()
        .filter(|other| other.alive())
        .map(|other| (other.player.frequency, other.x, other.y));
    if bot_bomb_safety_blocked(bot, fire.weapon_data, humans.chain(bots)) {
        return;
    }
    // Recheck at the commit boundary as well as during AI selection: a
    // queued decision must not overspend energy or survive a new shutdown.
    let energy_cost =
        crate::arena_settings::arena_ship_weapon_energy_cost(bot.player.ship, fire.weapon_data);
    let super_active = bot
        .super_until_tick
        .is_some_and(|deadline| !crate::continuum::continuum_tick_reached(tick, deadline));
    let mut energy =
        crate::authority::EnergyPool::new(bot.energy, bot.max_energy, bot.energy_remainder);
    if bot.shutdown_active(tick)
        || !bot_fire_ready(bot, tick, fire.weapon_data)
        || !energy.can_spend_weapon_energy(f32::from(energy_cost))
    {
        return;
    }
    let energy_before = bot.energy;
    let energy_remainder_before = bot.energy_remainder;
    let max_energy = bot.max_energy;
    if let Some(target_player_id) = fire.target_human_player_id {
        let decision = state
            .clients
            .get(&target_player_id)
            .map(|target| {
                target
                    .threat_presentation
                    .readiness(fire.source_runtime, tick)
            })
            .unwrap_or(crate::threat_presentation::ThreatReadinessDecision {
                harmful: false,
                reason: crate::threat_presentation::ThreatReadinessReason::SourceLifeNotEmitted,
                contract: None,
            });
        if !decision.harmful {
            state.qa_traces.record_for_all(serde_json::json!({
                "type": "server_weapon_event_suppressed",
                "reason": decision.reason.as_str(),
                "serverTick": tick,
                "ownerId": fire.bot_id,
                "ownerSessionId": fire.source_runtime.session.session_id.get().to_string(),
                "ownerLifeSeq": fire.life_seq,
                "targetId": target_player_id,
                "weapon": fire.weapon_data,
                "firstEmittedTick": decision.contract.map(|contract| contract.first_emitted_tick),
                "conservativeOpenTick": decision.contract.map(|contract| contract.conservative_open_tick),
                "hardOpenTick": decision.contract.map(|contract| contract.hard_open_tick),
                "applicationObservedTick": decision.contract.and_then(|contract| contract.application_observed_tick),
            }));
            return;
        }
    }
    let recipients = state
        .clients
        .iter()
        .filter_map(|(&player_id, client)| {
            (client.joined && client.snapshot_ready)
                .then(|| state.active_player_runtime_identity(player_id))
                .flatten()
        })
        .collect::<Vec<_>>();
    let Some(reservation) = crate::snapshot::reserve_realtime_event_deliveries(
        state,
        &[&recipients],
        RealtimeEventPressureDisposition::Deferred,
    ) else {
        let realtime_queue_current = state.realtime_event_queue_current();
        state.qa_traces.record_for_all(serde_json::json!({
            "type": "server_weapon_event_deferred",
            "reason": "realtime_event_capacity",
            "serverTick": tick,
            "ownerId": fire.bot_id,
            "ownerLifeSeq": fire.life_seq,
            "targetId": fire.target_human_player_id,
            "weapon": fire.weapon_data,
            "realtimeQueueCurrent": realtime_queue_current,
            "realtimeQueueCapacity": crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
        }));
        return;
    };
    let (bot_ship, bot_frequency, bot_session_id) = {
        let bot = state
            .bot(fire.bot_id)
            .expect("validated bot remains present");
        (
            bot.player.ship,
            bot.player.frequency,
            bot.session_id_for_wire(),
        )
    };
    let Some(group_id) = state.allocate_projectile_group_id() else {
        return;
    };
    if let Some(bot) = state.bot_mut(fire.bot_id) {
        if !super_active {
            energy.spend_weapon_energy(f32::from(energy_cost));
            bot.energy = energy.current();
            bot.energy_remainder = energy.recharge_remainder();
        }
        bot.last_fire_tick = Some(tick);
        bot.last_fire_weapon_data = Some(fire.weapon_data);
        if matches!(fire.weapon_data & 0x1f, 1 | 2) {
            bot.consecutive_gun_volleys = bot.consecutive_gun_volleys.saturating_add(1).min(2);
        } else {
            bot.consecutive_gun_volleys = 0;
        }
        let velocity = bot_launch_velocity(
            bot.player.ship,
            fire.weapon_data,
            fire.direction,
            fire.velocity_x,
            fire.velocity_y,
        );
        if velocity.x != fire.velocity_x || velocity.y != fire.velocity_y {
            bot.velocity_x = velocity.x;
            bot.velocity_y = velocity.y;
            bot.begin_actor_impulse_segment();
            bot.append_actor_motion_sample(tick, false);
        }
        fire.velocity_x = velocity.x;
        fire.velocity_y = velocity.y;
    }
    let event = crate::realtime_event::RealtimeWeaponEvent {
        source_player_id: fire.bot_id,
        source_session_id: bot_session_id,
        source_life_seq: fire.life_seq,
        shot_group_id: group_id,
        x: fire.x,
        y: fire.y,
        velocity_x: fire.velocity_x,
        velocity_y: fire.velocity_y,
        orientation: fire.orientation,
        frequency: bot_frequency,
        ship: bot_ship,
        weapon_data: fire.weapon_data,
    };
    reservation
        .commit(
            state,
            vec![RealtimeEventDelivery {
                recipients,
                server_tick: tick,
                payload: crate::realtime_event::RealtimeEventPayload::Weapon(event),
            }],
        )
        .expect("bot weapon reservation remains exclusive");
    state.note_owner_weapon_group(event);
    tracing::debug!(
        event = "bot_weapon_committed",
        difficulty = state.bot(fire.bot_id).map(|bot| bot.difficulty.label()),
        server_tick = tick,
        owner_id = fire.bot_id,
        owner_life_seq = fire.life_seq,
        owner_session_id = bot_session_id,
        shot_group_id = group_id,
        ship = bot_ship,
        weapon = fire.weapon_data,
        energy_before,
        energy_remainder_before,
        max_energy,
        energy_required = energy_cost,
        energy_charged = if super_active { 0 } else { energy_cost },
        energy_after = state.bot(fire.bot_id).map(|bot| bot.energy),
        energy_remainder_after = state.bot(fire.bot_id).map(|bot| bot.energy_remainder),
        recharge_per_second = state.bot(fire.bot_id).map(|bot| bot.recharge_per_second),
        super_active,
        x = fire.x,
        y = fire.y,
        velocity_x = fire.velocity_x,
        velocity_y = fire.velocity_y,
        orientation = fire.orientation,
        direction = fire.direction,
        "server bot weapon accepted"
    );
    if state.qa_traces.is_recording() {
        state.qa_traces.record_for_all(serde_json::json!({
            "type": "server_weapon_event_committed",
            "serverTick": tick,
            "energyBefore": energy_before,
            "energyRemainderBefore": f64::from(energy_remainder_before),
            "maxEnergy": max_energy,
            "energyCost": if super_active { 0 } else { energy_cost },
            "energyAfter": state.bot(fire.bot_id).map(|bot| bot.energy),
            "energyRemainderAfter": state.bot(fire.bot_id).map(|bot| f64::from(bot.energy_remainder)),
            "superActive": super_active,
            "shotGroupId": group_id.to_string(),
            "ship": bot_ship,
            "frequency": bot_frequency,
            "ownerId": fire.bot_id,
            "ownerSessionId": bot_session_id.to_string(),
            "ownerLifeSeq": fire.life_seq,
            "targetId": fire.target_human_player_id,
            "weapon": fire.weapon_data,
            "threatReadiness": fire.target_human_player_id.and_then(|target_id| {
                state.clients.get(&target_id).map(|target| {
                    target.threat_presentation.readiness(fire.source_runtime, tick).reason.as_str()
                })
            }),
            "source": {
                "x": fire.x,
                "y": fire.y,
                "velocityX": fire.velocity_x,
                "velocityY": fire.velocity_y,
                "orientation": fire.orientation,
            },
        }));
    }
    let initialized = crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
        fire.weapon_data,
        crate::gameplay_rules::RulesVector::new(fire.x, fire.y),
        crate::gameplay_rules::RulesVector::new(fire.velocity_x, fire.velocity_y),
        fire.direction,
        crate::arena_settings::arena_projectile_initialization_tuning(bot_ship),
    );
    let expiry_tick = crate::arena_settings::arena_weapon_lifetime_ticks(fire.weapon_data)
        .map(|duration| crate::continuum::continuum_tick_add(tick, duration));
    let group: Vec<ProjectileRecord> = initialized
        .into_iter()
        .enumerate()
        .map(|(shot_index, projectile)| ProjectileRecord {
            id: state.allocate_projectile_id(),
            shot_group_id: group_id,
            shot_index: shot_index as u8,
            presentation_source: Some(event),
            owner_id: fire.bot_id,
            owner_life_seq: fire.life_seq,
            combat_timeline: None,
            weapon_data: fire.weapon_data,
            x: projectile.position.x.round().clamp(0.0, u16::MAX as f32) as u16,
            y: projectile.position.y.round().clamp(0.0, u16::MAX as f32) as u16,
            timestamp: tick,
            created_tick: tick,
            velocity_x: (projectile.velocity.x * 10.0)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            velocity_y: (projectile.velocity.y * 10.0)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            expiry_tick,
            pos_x: projectile.position.x,
            source_server_tick: Some(tick),
            precise_velocity: Some(projectile.velocity),
            pos_y: projectile.position.y,
            bounces_remaining: projectile.bounces_remaining,
            behavior: projectile.behavior,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        })
        .collect();
    for projectile in &group {
        if state.qa_traces.is_recording() {
            state.qa_traces.record_for_all(serde_json::json!({
                "type": "server_projectile_spawned",
                "projectileId": projectile.id,
                "shotGroupId": projectile.shot_group_id,
                "shotIndex": projectile.shot_index,
                "ownerId": projectile.owner_id,
                "ownerSessionId": bot_session_id.to_string(),
                "ownerLifeSeq": projectile.owner_life_seq,
                "serverTick": projectile.created_tick,
                "weapon": projectile.weapon_data,
                "x": projectile.x,
                "y": projectile.y,
                "velocityX": projectile.velocity_x,
                "velocityY": projectile.velocity_y,
                "expiryTick": projectile.expiry_tick,
                "bouncesRemaining": projectile.bounces_remaining,
            }));
        }
    }
    state.record_projectile_group(group);
}

fn nearest_hostile_target(
    bot_id: u16,
    bot_frequency: u16,
    x: f32,
    y: f32,
    humans: &[BotTarget],
    bots: &[BotTarget],
    xradar: bool,
) -> Option<BotTarget> {
    let nearest = |targets: &[BotTarget]| {
        targets
            .iter()
            .copied()
            .filter(|target| target.player_id != Some(bot_id))
            .filter(|target| {
                target
                    .frequency
                    .is_some_and(|frequency| frequency != bot_frequency)
            })
            .filter(|target| bot_can_detect_target(target, xradar))
            .min_by(|left, right| {
                distance_squared(x, y, left.x, left.y)
                    .total_cmp(&distance_squared(x, y, right.x, right.y))
            })
    };

    // Human pilots are the multiplayer authority surface this AI exists to
    // exercise. Keep bot-versus-bot combat as the fallback while ensuring a
    // newly joined pilot cannot be ignored because another bot is closer.
    nearest(humans).or_else(|| nearest(bots))
}

fn assigned_hostile_human_slot(
    bot_index: usize,
    bot_frequency: u16,
    humans: &[BotTarget],
) -> Option<BotTarget> {
    humans.get(bot_index).copied().filter(|target| {
        target
            .frequency
            .is_some_and(|frequency| frequency != bot_frequency)
    })
}

fn human_assignment_rotation(lives: impl Iterator<Item = u32>, bot_count: usize) -> usize {
    if bot_count == 0 {
        return 0;
    }
    lives.fold(0_u64, |sum, life| {
        (sum + u64::from(life.saturating_sub(1))) % bot_count as u64
    }) as usize
}

fn human_assignment_slot(bot_index: usize, bot_count: usize, rotation: usize) -> usize {
    let count = bot_count.max(1);
    ((bot_index % count) + count - (rotation % count)) % count
}

fn bot_can_detect_target(target: &BotTarget, xradar: bool) -> bool {
    // Cloak hides the ship, stealth hides radar; active X-radar sees both.
    let fully_hidden =
        crate::authority::POSITION_STATUS_STEALTH | crate::authority::POSITION_STATUS_CLOAK;
    xradar || target.status & fully_hidden != fully_hidden
}

fn assigned_hostile_human_target(
    bot_index: usize,
    bot_frequency: u16,
    humans: &[BotTarget],
    xradar: bool,
) -> Option<BotTarget> {
    assigned_hostile_human_slot(bot_index, bot_frequency, humans)
        .filter(|target| bot_can_detect_target(target, xradar))
}

fn human_assigned_bot_ids(
    bots: &[crate::state::ServerBot],
    humans: &[BotTarget],
    rotation: usize,
) -> std::collections::BTreeSet<u16> {
    bots.iter()
        .enumerate()
        .filter_map(|(bot_index, bot)| {
            // Concealment does not reassign this bot to another human or let
            // surplus bots attack a reserved bot while it waits for detection.
            assigned_hostile_human_slot(
                human_assignment_slot(bot_index, bots.len(), rotation),
                bot.player.frequency,
                humans,
            )
            .is_some()
            .then_some(bot.player.id)
        })
        .collect()
}

fn surplus_bot_targets(
    bot_targets: &[BotTarget],
    human_assigned_bot_ids: &std::collections::BTreeSet<u16>,
) -> Vec<BotTarget> {
    bot_targets
        .iter()
        .copied()
        .filter(|target| {
            target
                .player_id
                .is_some_and(|id| !human_assigned_bot_ids.contains(&id))
        })
        .collect()
}

fn human_target_is_fresh(last_position_tick: Option<u32>, server_tick: u32) -> bool {
    last_position_tick.is_none_or(|last_tick| {
        crate::continuum::continuum_tick_elapsed(server_tick, last_tick)
            <= crate::continuum::seconds_to_ticks_u32(BOT_HUMAN_TARGET_STALE_SECONDS as f32)
    })
}

fn bot_target_is_in_fire_range(
    target_distance: f32,
    projectile_speed: f32,
    lifetime_ticks: u32,
) -> bool {
    target_distance <= BOT_COMBAT_PURSUIT_DISTANCE
        && target_distance
            < projectile_speed * crate::continuum::ticks_to_seconds_u32(lifetime_ticks)
}

fn hostile_target_requires_direct_approach(target: BotTarget, target_distance: f32) -> bool {
    target.player_id.is_some() && !target.in_safe_zone && target_distance > BOT_COMBAT_ORBIT_RADIUS
}

fn bot_should_seek_prize(has_fresh_human_target: bool, energy: u16, max_energy: u16) -> bool {
    has_fresh_human_target && f32::from(energy) < f32::from(max_energy) * BOT_PRIZE_ENERGY_FRACTION
}

fn combat_orbit_target(
    bot: &crate::state::ServerBot,
    target: BotTarget,
    server_tick: u32,
) -> BotTarget {
    let seconds = f64::from(server_tick) / 100.0;
    let direction = if bot.player.id % 2 == 0 { 1.0 } else { -1.0 };
    let identity_phase =
        f64::from(bot.player.id) * 0.381_966_011_25 + f64::from(bot.life_seq) * 0.173_205_080_75;
    let phase =
        (identity_phase + direction * seconds * BOT_COMBAT_ORBIT_TURNS_PER_SECOND).rem_euclid(1.0);
    let angle = phase as f32 * std::f32::consts::TAU;
    let radius = bot_tactical_orbit_radius(bot);
    BotTarget {
        player_id: target.player_id,
        frequency: target.frequency,
        x: target.x + angle.cos() * radius,
        y: target.y + angle.sin() * radius,
        velocity_x: target.velocity_x,
        velocity_y: target.velocity_y,
        in_safe_zone: target.in_safe_zone,
        status: target.status,
    }
}

/// Return the heading point whose direction compensates for both the target's
/// velocity and the shooter's inherited projectile velocity. Continuum gun
/// projectiles start at `shooter_velocity + heading * bullet_speed`; aiming
/// at the target's current coordinates therefore misses systematically while
/// either actor is moving. If no positive intercept exists inside the
/// authored bullet lifetime, retain the direct target instead of inventing an
/// unbounded lead.
#[cfg(test)]
fn bot_bullet_intercept_target(bot: &crate::state::ServerBot, target: BotTarget) -> BotTarget {
    let Some(weapon_data) = bot.gun_weapon_data() else {
        return target;
    };
    bot_weapon_intercept_target(bot, target, weapon_data)
}

fn bot_weapon_intercept_target(
    bot: &crate::state::ServerBot,
    target: BotTarget,
    weapon_data: u16,
) -> BotTarget {
    let exact = bot_exact_weapon_intercept_target(bot, target, weapon_data);
    let fraction = bot.difficulty.lead_fraction();
    if bot.difficulty == crate::state::BotDifficulty::Hard {
        return exact;
    }
    let dx = target.x + (exact.x - target.x) * fraction - bot.x;
    let dy = target.y + (exact.y - target.y) * fraction - bot.y;
    // Smooth four-second aim drift, staggered per pilot and life. It changes
    // intent before steering/firing, never the projectile after launch. No
    // per-tick randomness, aim teleport or bypass of the normal facing gate.
    let phase = (bot.last_step_tick % 400) as f32 / 400.0 * std::f32::consts::TAU
        + f32::from(bot.player.id) * 0.73
        + (bot.life_seq % 100) as f32 * 0.41;
    let error = phase.sin() * bot.difficulty.aim_error_degrees().to_radians();
    let (sin, cos) = error.sin_cos();
    BotTarget {
        x: bot.x + dx * cos - dy * sin,
        y: bot.y + dx * sin + dy * cos,
        ..target
    }
}

fn bot_exact_weapon_intercept_target(
    bot: &crate::state::ServerBot,
    target: BotTarget,
    weapon_data: u16,
) -> BotTarget {
    let lifetime_ticks =
        crate::arena_settings::arena_weapon_lifetime_ticks(weapon_data).unwrap_or(0);
    let lifetime_seconds = crate::continuum::ticks_to_seconds_u32(lifetime_ticks);
    let tuning = crate::arena_settings::arena_projectile_initialization_tuning(bot.player.ship);
    let projectile_speed = if matches!(weapon_data & 0x1f, 3 | 4) {
        (tuning.bomb_speed - crate::arena_settings::arena_ship_bomb_recoil_speed(bot.player.ship))
            .max(0.0)
    } else {
        tuning.bullet_speed
    };
    let relative_x = target.x - bot.x;
    let relative_y = target.y - bot.y;
    let relative_velocity_x = target.velocity_x - bot.velocity_x;
    let relative_velocity_y = target.velocity_y - bot.velocity_y;
    let a = relative_velocity_x * relative_velocity_x + relative_velocity_y * relative_velocity_y
        - projectile_speed * projectile_speed;
    let b = 2.0 * (relative_x * relative_velocity_x + relative_y * relative_velocity_y);
    let c = relative_x * relative_x + relative_y * relative_y;
    let epsilon = 0.000_1;
    let intercept_seconds = if a.abs() <= epsilon {
        (b.abs() > epsilon).then(|| -c / b)
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            None
        } else {
            let root = discriminant.sqrt();
            let first = (-b - root) / (2.0 * a);
            let second = (-b + root) / (2.0 * a);
            [first, second]
                .into_iter()
                .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                .min_by(f32::total_cmp)
        }
    };
    let Some(intercept_seconds) =
        intercept_seconds.filter(|seconds| lifetime_seconds > 0.0 && *seconds <= lifetime_seconds)
    else {
        return target;
    };
    BotTarget {
        x: bot.x + relative_x + relative_velocity_x * intercept_seconds,
        y: bot.y + relative_y + relative_velocity_y * intercept_seconds,
        ..target
    }
}

fn combat_aim_phase(bot: &crate::state::ServerBot, server_tick: u32) -> bool {
    let ordinal = u32::from(
        bot.player
            .id
            .saturating_sub(crate::state::SERVER_BOT_ID_BASE),
    );
    let phase_offset = ordinal.wrapping_mul(BOT_COMBAT_CYCLE_TICKS / 2);
    server_tick.wrapping_add(phase_offset) % BOT_COMBAT_CYCLE_TICKS < BOT_COMBAT_AIM_TICKS_PER_CYCLE
}

fn collision_avoiding_orientation(
    bot: &crate::state::ServerBot,
    desired: f32,
    prefer_nearest_protocol_heading: bool,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> f32 {
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
    let probe_distance =
        (radius + 40.0 + bot.velocity_x.hypot(bot.velocity_y) * 0.2).min(radius + 96.0);
    let handedness = if bot.player.id % 2 == 0 { 1.0 } else { -1.0 };
    for offset in [
        0.0,
        handedness * 0.125,
        -handedness * 0.125,
        handedness * 0.25,
        -handedness * 0.25,
        0.5,
    ] {
        let continuous_candidate = (desired + offset).rem_euclid(1.0);
        // During the explicit aim phase the bot's weapon is authoritative in
        // Continuum's 40 legal headings. Pick the nearest heading before
        // probing and turning. Feeding an arbitrary continuous target into
        // the shared truncating wire adapter can otherwise stop one bin early:
        // the bot appears aimed but cannot fire for part of every nine-degree
        // sector. Preserve continuous steering during ordinary pursuit so
        // this correction does not rewrite the proven navigation controller.
        let candidate = if prefer_nearest_protocol_heading {
            nearest_protocol_orientation(continuous_candidate)
        } else {
            continuous_candidate
        };
        let (heading_x, heading_y) = canonical_discrete_heading_components(candidate);
        if bot_hull_segment_is_clear(
            bot.x,
            bot.y,
            heading_x,
            heading_y,
            probe_distance,
            radius,
            map,
            door_seeds,
            bricks,
        ) {
            return candidate;
        }
    }
    if prefer_nearest_protocol_heading {
        nearest_protocol_orientation(desired)
    } else {
        desired
    }
}

fn nearest_protocol_orientation(orientation: f32) -> f32 {
    let direction = ((orientation.rem_euclid(1.0) * 40.0).round() as u8) % 40;
    protocol_direction_to_orientation(direction)
}

fn bot_hull_segment_is_clear(
    start_x: f32,
    start_y: f32,
    heading_x: f32,
    heading_y: f32,
    distance: f32,
    radius: f32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    // Navigation must use movement's safe-entry policy. Otherwise it repeatedly
    // chooses a route through protection which the collision solver rejects.
    let in_safe = |x: f32, y: f32| {
        world_position_in_safe_tile_for_map(
            map,
            x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
        )
    };
    let reject_safe_entry = !in_safe(start_x, start_y);
    let sample_distance = TILE_SIZE_PX * 0.5;
    let steps = (distance.max(0.0) / sample_distance).ceil().max(1.0) as usize;
    (1..=steps).all(|step| {
        let progress = step as f32 / steps as f32;
        let x = start_x + heading_x * distance * progress;
        let y = start_y + heading_y * distance * progress;
        (!reject_safe_entry || !in_safe(x, y))
            && bot_hull_is_clear(x, y, radius, map, door_seeds, bricks)
    })
}

fn bot_respawn_position(
    bot: &crate::state::ServerBot,
    bot_index: usize,
    center: (u16, u16),
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> (f32, f32, f32, f32) {
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
    let base_phase = bot_index as f32 * 0.381_966_02 + bot.life_seq as f32 * 0.173_205_08;
    for spawn_radius in [320.0, 384.0, 256.0, 448.0, 192.0, 512.0, 128.0, 96.0] {
        for angular_step in 0..24 {
            let phase = base_phase + angular_step as f32 / 24.0;
            let angle = phase * std::f32::consts::TAU;
            let x = f32::from(center.0) + angle.cos() * spawn_radius;
            let y = f32::from(center.1) + angle.sin() * spawn_radius;
            if bot_hull_is_clear(x, y, radius, map, door_seeds, bricks)
                && !world_position_in_safe_tile_for_map(
                    map,
                    x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                    y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
                )
            {
                let tangent = angle
                    + if bot.player.id % 2 == 0 {
                        std::f32::consts::FRAC_PI_2
                    } else {
                        -std::f32::consts::FRAC_PI_2
                    };
                return (x, y, tangent.cos(), tangent.sin());
            }
        }
    }

    let angle = base_phase * std::f32::consts::TAU;
    let fallback_radius = 96.0 + (bot_index % 5) as f32 * 32.0;
    let tangent = angle
        + if bot.player.id % 2 == 0 {
            std::f32::consts::FRAC_PI_2
        } else {
            -std::f32::consts::FRAC_PI_2
        };
    (
        f32::from(center.0) + angle.cos() * fallback_radius,
        f32::from(center.1) + angle.sin() * fallback_radius,
        tangent.cos(),
        tangent.sin(),
    )
}

fn nearest_prize(x: f32, y: f32, prizes: &[crate::state::ServerHiddenPrize]) -> Option<BotTarget> {
    prizes
        .iter()
        .filter(|prize| {
            distance_squared(x, y, f32::from(prize.x), f32::from(prize.y))
                <= BOT_PRIZE_SEEK_RADIUS * BOT_PRIZE_SEEK_RADIUS
        })
        .min_by(|left, right| {
            distance_squared(x, y, f32::from(left.x), f32::from(left.y)).total_cmp(
                &distance_squared(x, y, f32::from(right.x), f32::from(right.y)),
            )
        })
        .map(|prize| BotTarget {
            player_id: None,
            frequency: None,
            x: f32::from(prize.x),
            y: f32::from(prize.y),
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        })
}

fn objective_targets(state: &ServerState) -> Vec<BotTarget> {
    let mut targets = Vec::new();
    for flag in state.objectives.replay().flags {
        if let crate::objectives::FlagState::OnMap { x, y, .. } = flag.state {
            targets.push(BotTarget {
                player_id: None,
                frequency: None,
                x: f32::from(x),
                y: f32::from(y),
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            });
        }
    }
    for ball in state.objectives.replay().powerballs {
        if let crate::objectives::PowerballState::OnMap { x, y, .. } = ball.state {
            targets.push(BotTarget {
                player_id: None,
                frequency: None,
                x: f32::from(x),
                y: f32::from(y),
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            });
        }
    }
    targets
}

fn next_safe_egress_target(
    bot: &mut crate::state::ServerBot,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> Option<BotTarget> {
    while bot
        .safe_egress_path
        .front()
        .is_some_and(|&(x, y)| (f32::from(x) - bot.x).hypot(f32::from(y) - bot.y) <= 8.0)
    {
        bot.safe_egress_path.pop_front();
    }
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
    if bot.safe_egress_path.front().is_some_and(|&(x, y)| {
        !bot_hull_is_clear(f32::from(x), f32::from(y), radius, map, door_seeds, bricks)
    }) {
        bot.safe_egress_path.clear();
    }
    if bot.safe_egress_path.is_empty() {
        bot.safe_egress_path = combat_tile_path(bot, map, door_seeds, bricks);
    }
    bot.safe_egress_path.front().map(|&(x, y)| BotTarget {
        player_id: None,
        frequency: None,
        x: f32::from(x),
        y: f32::from(y),
        velocity_x: 0.0,
        velocity_y: 0.0,
        in_safe_zone: false,
        status: 0,
    })
}

fn combat_tile_path(
    bot: &crate::state::ServerBot,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> std::collections::VecDeque<(u16, u16)> {
    let start = (
        (bot.x / TILE_SIZE_PX).floor() as i32,
        (bot.y / TILE_SIZE_PX).floor() as i32,
    );
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
    const SEARCH_RADIUS: i32 = 128;
    const SEARCH_DIAMETER: usize = SEARCH_RADIUS as usize * 2 + 1;
    const UNSEEN: usize = usize::MAX;
    let origin = (start.0 - SEARCH_RADIUS, start.1 - SEARCH_RADIUS);
    let local_index = |x: i32, y: i32| -> Option<usize> {
        let local_x = x - origin.0;
        let local_y = y - origin.1;
        ((0..SEARCH_DIAMETER as i32).contains(&local_x)
            && (0..SEARCH_DIAMETER as i32).contains(&local_y))
        .then_some(local_y as usize * SEARCH_DIAMETER + local_x as usize)
    };
    let world_tile = |index: usize| -> (i32, i32) {
        (
            origin.0 + (index % SEARCH_DIAMETER) as i32,
            origin.1 + (index / SEARCH_DIAMETER) as i32,
        )
    };
    let start_index = local_index(start.0, start.1).expect("search origin contains start");
    let mut frontier = std::collections::VecDeque::from([(start_index, 0_u16)]);
    let mut parents = vec![UNSEEN; SEARCH_DIAMETER * SEARCH_DIAMETER];
    parents[start_index] = start_index;

    while let Some((candidate_index, depth)) = frontier.pop_front() {
        let (candidate_x, candidate_y) = world_tile(candidate_index);
        let world_x = (candidate_x as f32 + 0.5) * TILE_SIZE_PX;
        let world_y = (candidate_y as f32 + 0.5) * TILE_SIZE_PX;
        let wire_x = world_x.round() as u16;
        let wire_y = world_y.round() as u16;
        if depth > 0 && !world_position_in_safe_tile_for_map(map, wire_x, wire_y) {
            let mut cursor = candidate_index;
            let mut reversed = vec![world_tile(cursor)];
            while parents[cursor] != start_index {
                cursor = parents[cursor];
                reversed.push(world_tile(cursor));
            }
            reversed.reverse();
            return reversed
                .into_iter()
                .map(|(x, y)| {
                    (
                        ((x as f32 + 0.5) * TILE_SIZE_PX).round() as u16,
                        ((y as f32 + 0.5) * TILE_SIZE_PX).round() as u16,
                    )
                })
                .collect();
        }
        if depth >= 128 {
            continue;
        }
        for next in [
            (candidate_x, candidate_y - 1),
            (candidate_x + 1, candidate_y),
            (candidate_x, candidate_y + 1),
            (candidate_x - 1, candidate_y),
        ] {
            let Some(next_index) = local_index(next.0, next.1) else {
                continue;
            };
            if next.0 < 0
                || next.1 < 0
                || next.0 >= MAP_TILES as i32
                || next.1 >= MAP_TILES as i32
                || parents[next_index] != UNSEEN
            {
                continue;
            }
            let next_x = (next.0 as f32 + 0.5) * TILE_SIZE_PX;
            let next_y = (next.1 as f32 + 0.5) * TILE_SIZE_PX;
            if !bot_hull_is_clear(next_x, next_y, radius, map, door_seeds, bricks) {
                continue;
            }
            parents[next_index] = candidate_index;
            frontier.push_back((next_index, depth + 1));
        }
    }
    std::collections::VecDeque::new()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BotCollisionStep {
    start_x: f32,
    start_y: f32,
    attempted_x: f32,
    attempted_y: f32,
    collision_x: bool,
    collision_y: bool,
}

fn apply_bot_wall_following_impulse(
    bot: &mut crate::state::ServerBot,
    collision: BotCollisionStep,
    server_tick: u32,
    hostile_target: Option<(f32, f32)>,
) {
    if !collision.collision_x && !collision.collision_y {
        return;
    }

    // Local look-ahead keeps ordinary steering away from walls, but a ship
    // already touching a long wall can still spend every tick cancelling its
    // reflected velocity with thrust toward the target. Give that collision
    // a stable tangential component. The shared axis/AABB integrator remains
    // the sole authority for the resulting position, so this cannot move a
    // hull through the obstacle or an authored safe boundary.
    if bot.wall_follow_until_tick.is_some_and(|deadline| {
        !crate::continuum::continuum_tick_reached(server_tick, deadline)
            && (bot.wall_follow_x != 0 || bot.wall_follow_y != 0)
    }) {
        apply_active_bot_wall_following(bot, server_tick);
        return;
    }

    let handedness = if bot.player.id % 2 == 0 { 1_i8 } else { -1_i8 };
    let away_x = if collision.attempted_x < collision.start_x {
        1
    } else {
        -1
    };
    let away_y = if collision.attempted_y < collision.start_y {
        1
    } else {
        -1
    };
    let target_x_direction = hostile_target
        .map(|(target_x, _)| signed_target_direction(target_x - bot.x, -handedness))
        .unwrap_or(-handedness);
    let target_y_direction = hostile_target
        .map(|(_, target_y)| signed_target_direction(target_y - bot.y, handedness))
        .unwrap_or(handedness);
    match (collision.collision_x, collision.collision_y) {
        (true, false) => {
            bot.wall_follow_x = away_x;
            bot.wall_follow_y = target_y_direction;
        }
        (false, true) => {
            bot.wall_follow_x = target_x_direction;
            bot.wall_follow_y = away_y;
        }
        (true, true) => {
            bot.wall_follow_x = away_x;
            bot.wall_follow_y = away_y;
        }
        (false, false) => {}
    }
    bot.wall_follow_until_tick = Some(crate::continuum::continuum_tick_add(
        server_tick,
        crate::continuum::seconds_to_ticks_u32(2.0),
    ));
    apply_active_bot_wall_following(bot, server_tick);
}

fn signed_target_direction(delta: f32, fallback: i8) -> i8 {
    if delta > 0.5 {
        1
    } else if delta < -0.5 {
        -1
    } else {
        fallback
    }
}

fn apply_active_bot_wall_following(bot: &mut crate::state::ServerBot, server_tick: u32) {
    let Some(deadline) = bot.wall_follow_until_tick else {
        return;
    };
    if crate::continuum::continuum_tick_reached(server_tick, deadline) {
        bot.wall_follow_until_tick = None;
        bot.wall_follow_x = 0;
        bot.wall_follow_y = 0;
        return;
    }
    let slide_speed = bot.top_speed_pixels_per_second * 0.45;
    if bot.wall_follow_x != 0 {
        bot.velocity_x = f32::from(bot.wall_follow_x) * bot.velocity_x.abs().max(slide_speed);
    }
    if bot.wall_follow_y != 0 {
        bot.velocity_y = f32::from(bot.wall_follow_y) * bot.velocity_y.abs().max(slide_speed);
    }
}

fn release_bot_wall_following_for_clear_route(
    bot: &mut crate::state::ServerBot,
    steering_orientation: f32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) {
    if bot.wall_follow_until_tick.is_none() {
        return;
    }
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
    let probe_distance =
        (radius + 40.0 + bot.velocity_x.hypot(bot.velocity_y) * 0.2).min(radius + 96.0);
    let (heading_x, heading_y) = canonical_discrete_heading_components(steering_orientation);
    if bot_hull_segment_is_clear(
        bot.x,
        bot.y,
        heading_x,
        heading_y,
        probe_distance,
        radius,
        map,
        door_seeds,
        bricks,
    ) {
        // Wall following is a detour while the desired route is blocked,
        // not a mandatory retreat lasting until a timer expires. Hand control
        // back without changing velocity or bypassing ordinary collision.
        bot.wall_follow_until_tick = None;
        bot.wall_follow_x = 0;
        bot.wall_follow_y = 0;
    }
}

fn move_bot_with_collision(
    bot: &mut crate::state::ServerBot,
    dt: f32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> BotCollisionStep {
    // The shared axis solver tests the leading tile at each endpoint. Keep
    // each displacement below a tile so a longer logical turn (or Repel
    // velocity) cannot skip an intervening wall. The caller bounds duration
    // and clamps authoritative speed; bounce only reduces it within this turn.
    let distance = bot.velocity_x.abs().max(bot.velocity_y.abs()) * dt;
    let steps = (distance / (TILE_SIZE_PX * 0.5)).ceil().max(1.0) as u32;
    let mut result = BotCollisionStep {
        start_x: bot.x,
        start_y: bot.y,
        attempted_x: bot.x + bot.velocity_x * dt,
        attempted_y: bot.y + bot.velocity_y * dt,
        ..BotCollisionStep::default()
    };
    for _ in 0..steps {
        let step = move_bot_collision_substep(bot, dt / steps as f32, map, door_seeds, bricks);
        result.collision_x |= step.collision_x;
        result.collision_y |= step.collision_y;
    }
    result
}

fn move_bot_collision_substep(
    bot: &mut crate::state::ServerBot,
    dt: f32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> BotCollisionStep {
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
    let start_x = bot.x;
    let start_y = bot.y;
    // Bots may spawn inside an authored safety region and must be allowed to
    // follow their egress route out. Once their center is outside, treat safe
    // tiles as stable collision for bot motion. Otherwise an orbiting bot can
    // repeatedly dip into Extreme's green, become damage-immune, and recharge
    // between genuine human hits without ever advancing life.
    let reject_safe_entry = !world_position_in_safe_tile_for_map(
        map,
        start_x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
        start_y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
    );
    let attempted_x = start_x + bot.velocity_x * dt;
    let attempted_y = start_y + bot.velocity_y * dt;
    let integrated = integrate_player_collision(
        MovementVector::new(start_x, start_y),
        MovementVector::new(bot.velocity_x, bot.velocity_y),
        dt,
        0.35,
        |candidate, axis, signed_delta| {
            bot_axis_overlaps_blocked_tile(
                candidate,
                radius,
                map,
                door_seeds,
                bricks,
                axis,
                signed_delta,
                reject_safe_entry,
            )
        },
    );
    bot.x = integrated.position.x;
    bot.y = integrated.position.y;
    bot.velocity_x = integrated.velocity.x;
    bot.velocity_y = integrated.velocity.y;
    BotCollisionStep {
        start_x,
        start_y,
        attempted_x,
        attempted_y,
        collision_x: integrated.position.x.to_bits() == start_x.to_bits()
            && attempted_x.to_bits() != start_x.to_bits(),
        collision_y: integrated.position.y.to_bits() == start_y.to_bits()
            && attempted_y.to_bits() != start_y.to_bits(),
    }
}

fn bot_axis_overlaps_blocked_tile(
    candidate: MovementVector,
    radius: f32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
    axis: CollisionAxis,
    signed_delta: f32,
    reject_safe_entry: bool,
) -> bool {
    if reject_safe_entry
        && world_position_in_safe_tile_for_map(
            map,
            candidate.x.round().clamp(0.0, f32::from(u16::MAX)) as u16,
            candidate.y.round().clamp(0.0, f32::from(u16::MAX)) as u16,
        )
    {
        return true;
    }
    movement_axis_overlaps_blocked_tile(
        candidate,
        radius,
        MAP_TILES,
        TILE_SIZE_PX,
        axis,
        signed_delta,
        |tile_x, tile_y| bot_tile_is_blocked(tile_x, tile_y, map, door_seeds, bricks),
    )
}

#[cfg(test)]
fn bot_tile_is_safe(tile_x: i32, tile_y: i32, map: &crate::zone::ZoneMap) -> bool {
    if tile_x < 0 || tile_y < 0 || tile_x >= MAP_TILES as i32 || tile_y >= MAP_TILES as i32 {
        return false;
    }
    map.tiles
        .get(tile_y as usize * MAP_TILES + tile_x as usize)
        .is_some_and(|&tile| tile == 171)
}

fn bot_overlaps_closed_door(
    bot: &crate::state::ServerBot,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
) -> bool {
    door_seeds.iter().any(|&seed| {
        crate::gameplay_rules::newly_closed_door_overlap(
            bot.x,
            bot.y,
            crate::arena_settings::arena_ship_radius_pixels(bot.player.ship),
            MAP_TILES,
            TILE_SIZE_PX,
            0,
            seed,
            |x, y| map.tiles.get(y as usize * MAP_TILES + x as usize).copied(),
            |tile, mask| crate::zone::position_blocking_tile_for_door_seeds(tile, &[mask]),
        )
    })
}

fn apply_bot_door_relocation(
    bot: &mut crate::state::ServerBot,
    destination: Option<(u16, u16)>,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    bot.velocity_x = 0.0;
    bot.velocity_y = 0.0;
    bot.safe_egress_path.clear();
    bot.wall_follow_until_tick = None;
    bot.wall_follow_x = 0;
    bot.wall_follow_y = 0;
    let Some((x, y)) = destination.filter(|&(x, y)| {
        bot_hull_is_clear(
            f32::from(x),
            f32::from(y),
            crate::arena_settings::arena_ship_radius_pixels(bot.player.ship),
            map,
            door_seeds,
            bricks,
        )
    }) else {
        return false;
    };
    bot.x = f32::from(x);
    bot.y = f32::from(y);
    bot.begin_actor_motion_segment();
    true
}

fn bot_tile_is_blocked(
    tile_x: i32,
    tile_y: i32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    if tile_x < 0 || tile_y < 0 || tile_x >= MAP_TILES as i32 || tile_y >= MAP_TILES as i32 {
        return true;
    }
    let tile_x = tile_x as usize;
    let tile_y = tile_y as usize;
    if bricks.contains(&(tile_x as u16, tile_y as u16)) {
        return true;
    }
    map.tiles
        .get(tile_y * MAP_TILES + tile_x)
        .is_none_or(|&tile| crate::zone::position_blocking_tile_for_door_seeds(tile, door_seeds))
}

fn bot_hull_is_clear(
    x: f32,
    y: f32,
    radius: f32,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    let arena = MAP_TILES as f32 * TILE_SIZE_PX;
    if !x.is_finite()
        || !y.is_finite()
        || !radius.is_finite()
        || radius < 0.0
        || x - radius < 0.0
        || y - radius < 0.0
        || x + radius >= arena
        || y + radius >= arena
    {
        return false;
    }
    let center_x = x.trunc();
    let center_y = y.trunc();
    let hull_min_x = center_x - radius;
    let hull_min_y = center_y - radius;
    let hull_max_x = center_x + radius;
    let hull_max_y = center_y + radius;
    let min_tile_x = (hull_min_x / TILE_SIZE_PX).floor() as i32;
    let min_tile_y = (hull_min_y / TILE_SIZE_PX).floor() as i32;
    let max_tile_x = (hull_max_x / TILE_SIZE_PX).floor() as i32;
    let max_tile_y = (hull_max_y / TILE_SIZE_PX).floor() as i32;
    for tile_y in min_tile_y..=max_tile_y {
        for tile_x in min_tile_x..=max_tile_x {
            if tile_x < 0 || tile_y < 0 || tile_x >= MAP_TILES as i32 || tile_y >= MAP_TILES as i32
            {
                return false;
            }
            if bot_tile_is_blocked(tile_x, tile_y, map, door_seeds, bricks)
                && crate::gameplay_rules::reference_game_player_hull_overlaps_tile(
                    hull_min_x,
                    hull_min_y,
                    hull_max_x,
                    hull_max_y,
                    tile_x as f32 * TILE_SIZE_PX,
                    tile_y as f32 * TILE_SIZE_PX,
                    (tile_x + 1) as f32 * TILE_SIZE_PX,
                    (tile_y + 1) as f32 * TILE_SIZE_PX,
                )
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
pub(super) fn bot_hull_is_clear_for_test(
    bot: &crate::state::ServerBot,
    map: &crate::zone::ZoneMap,
    door_seeds: &[u8],
    bricks: &std::collections::BTreeSet<(u16, u16)>,
) -> bool {
    bot_hull_is_clear(
        bot.x,
        bot.y,
        crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0),
        map,
        door_seeds,
        bricks,
    )
}

fn bot_overlaps_prize(
    bot: &crate::state::ServerBot,
    prize: &crate::state::ServerHiddenPrize,
) -> bool {
    let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(0.0);
    let half_prize = TILE_SIZE_PX * 0.5;
    bot.x + radius >= f32::from(prize.x) - half_prize
        && bot.x - radius < f32::from(prize.x) + half_prize
        && bot.y + radius >= f32::from(prize.y) - half_prize
        && bot.y - radius < f32::from(prize.y) + half_prize
}

fn clamp_velocity(bot: &mut crate::state::ServerBot, server_tick: u32) {
    let speed = bot.velocity_x.hypot(bot.velocity_y);
    let top_speed = if bot.repel_active(server_tick) {
        bot.top_speed_pixels_per_second
            .max(crate::arena_settings::arena_repel_speed_pixels_per_second())
    } else {
        bot.top_speed_pixels_per_second
    };
    if speed > top_speed && speed > f32::EPSILON {
        let scale = top_speed / speed;
        bot.velocity_x *= scale;
        bot.velocity_y *= scale;
    }
}

fn turn_towards(current: f32, desired: f32, maximum: f32) -> f32 {
    let delta = (desired - current + 0.5).rem_euclid(1.0) - 0.5;
    (current + delta.clamp(-maximum.max(0.0), maximum.max(0.0))).rem_euclid(1.0)
}

fn distance_squared(x: f32, y: f32, target_x: f32, target_y: f32) -> f32 {
    let dx = target_x - x;
    let dy = target_y - y;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::{
        BOT_COMBAT_ORBIT_RADIUS, BOT_COMBAT_ORBIT_TURNS_PER_SECOND, BOT_COMBAT_PURSUIT_DISTANCE,
        BotCollisionStep, BotTarget, CollisionAxis, MovementVector, PendingBotFire, TILE_SIZE_PX,
        apply_bot_door_relocation, apply_bot_wall_following_impulse, assigned_hostile_human_target,
        bot_axis_overlaps_blocked_tile, bot_bullet_intercept_target, bot_hull_is_clear,
        bot_hull_is_clear_for_test, bot_hull_segment_is_clear, bot_movement_trace_due,
        bot_overlaps_closed_door, bot_should_seek_prize, bot_target_is_in_fire_range,
        bot_tile_is_blocked, bot_tile_is_safe, combat_orbit_target, combat_tile_path,
        hostile_target_requires_direct_approach, human_assigned_bot_ids, human_target_is_fresh,
        integrate_player_collision, nearest_hostile_target, nearest_protocol_orientation,
        spawn_bot_weapon, step_server_bots, surplus_bot_targets,
    };

    fn bot_energy_fixture(
        ship: u8,
        weapon_data: u16,
    ) -> (crate::state::ServerState, PendingBotFire) {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                bot_ship_types: vec![ship],
                maxed_spawn_loadout: true,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let bot = &state.bots[0];
        let fire = PendingBotFire {
            bot_id: bot.player.id,
            life_seq: bot.life_seq,
            source_runtime: bot.runtime_identity(),
            target_human_player_id: None,
            x: bot.x,
            y: bot.y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            direction: 0,
            weapon_data,
        };
        (state, fire)
    }

    #[test]
    fn production_bot_step_selects_and_commits_a_bomb_for_a_visible_distant_opponent() {
        let (mut initial, _) = bot_energy_fixture(0, 1);
        let map = initial.zone_map();
        let (tile_x, tile_y) = (32..980)
            .find_map(|y| {
                (32..980).find_map(|x| {
                    (0..24)
                        .all(|dy| {
                            (0..5).all(|dx| map.tiles[(y + dy) * super::MAP_TILES + x + dx] == 0)
                        })
                        .then_some((x, y))
                })
            })
            .expect("open authored 5x24 tile combat corridor");
        let x = ((tile_x + 2) as f32 + 0.5) * TILE_SIZE_PX;
        let target_y = (tile_y as f32 + 0.5) * TILE_SIZE_PX;
        let bot = &mut initial.bots[0];
        bot.x = x;
        bot.y = target_y + 304.0;
        bot.orientation = 0.0;
        bot.direction = 0;
        bot.velocity_x = 0.0;
        bot.velocity_y = 0.0;
        bot.last_step_tick = 99;
        bot.consecutive_gun_volleys = 2;
        bot.actor_motion_history = Default::default();
        bot.begin_actor_motion_segment();
        bot.append_actor_motion_sample(99, false);
        let runtime = bot.runtime_identity();
        let state = std::sync::Arc::new(std::sync::Mutex::new(initial));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player, 0);
        {
            let mut guard = state.lock().unwrap();
            let client = guard.clients.get_mut(&player).unwrap();
            let mut position = vec![0; 27];
            position[4..6].copy_from_slice(&(x as u16).to_le_bytes());
            position[15..17].copy_from_slice(&(target_y as u16).to_le_bytes());
            client.last_position = Some(position);
            client.last_position_server_tick = Some(100);
            client.snapshot_ready = true;
            let contract = client.threat_presentation.observe_emitted(runtime, 1, 0);
            assert!(contract.hard_open_tick <= 100);
        }
        step_server_bots(&state, 100);
        let guard = state.lock().unwrap();
        assert_eq!(guard.active_projectiles.len(), 1);
        assert!(matches!(
            guard.active_projectiles.last().unwrap().weapon_data & 31,
            3 | 4
        ));
        assert_eq!(guard.bots[0].consecutive_gun_volleys, 0);
        assert!(guard.bots[0].energy < guard.bots[0].max_energy);
    }

    #[test]
    fn bot_tactics_commit_two_gun_volleys_then_a_safe_bomb_with_real_cost_and_cooldown() {
        for ship in [0, 3, 4, 6, 7] {
            let (mut state, mut fire) = bot_energy_fixture(ship, 1);
            let bot = &mut state.bots[0];
            bot.x = 1000.0;
            bot.y = 1000.0;
            bot.velocity_x = 0.0;
            bot.velocity_y = 0.0;
            fire.x = bot.x;
            fire.y = bot.y;
            let gun = bot.gun_weapon_data().unwrap();
            let target = BotTarget {
                player_id: Some(1),
                frequency: Some(65535),
                x: 1000.0,
                y: 690.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            };
            let delay =
                crate::arena_settings::arena_ship_weapon_fire_delay_ticks(ship, (gun & 31) as u8)
                    * super::BOT_FIRE_COOLDOWN_MULTIPLIER;
            fire.weapon_data = gun;
            for shot in 0..2 {
                let tick = 100 + shot * delay;
                assert_eq!(
                    super::bot_combat_weapon(&state.bots[0], target, tick, |_, _| false),
                    Some(gun)
                );
                spawn_bot_weapon(&mut state, tick, fire);
                assert_eq!(state.bots[0].consecutive_gun_volleys, (shot + 1) as u8);
            }
            let tick = 100 + 2 * delay;
            let bomb =
                super::bot_combat_weapon(&state.bots[0], target, tick, |_, _| false).unwrap();
            assert!(
                matches!(bomb & 31, 3 | 4),
                "ship {ship}: safe follow-up must be a bomb"
            );
            assert_eq!(
                crate::continuum_weapon::continuum_weapon_level(bomb),
                if ship == 3 { 3 } else { 2 }
            );
            let before = state.bots[0].energy;
            let cost = crate::arena_settings::arena_ship_weapon_energy_cost(ship, bomb);
            fire.weapon_data = bomb;
            spawn_bot_weapon(&mut state, tick, fire);
            assert_eq!(state.bots[0].energy, before - cost);
            assert_eq!(state.bots[0].consecutive_gun_volleys, 0);
            assert_eq!(state.bots[0].last_fire_weapon_data, Some(bomb));
            assert_eq!(
                state.bots[0].velocity_y,
                crate::arena_settings::arena_ship_bomb_recoil_speed(ship)
            );
            assert_eq!(state.active_projectiles.last().unwrap().weapon_data, bomb);
            let bomb_delay =
                crate::arena_settings::arena_ship_weapon_fire_delay_ticks(ship, (bomb & 31) as u8)
                    * super::BOT_FIRE_COOLDOWN_MULTIPLIER;
            let count = state.active_projectiles.len();
            let after_bomb = state.bots[0].energy;
            fire.weapon_data = gun;
            spawn_bot_weapon(&mut state, tick + bomb_delay - 1, fire);
            assert_eq!(state.active_projectiles.len(), count);
            assert_eq!(state.bots[0].energy, after_bomb);
            spawn_bot_weapon(&mut state, tick + bomb_delay, fire);
            assert!(state.active_projectiles.len() > count);
        }
    }

    #[test]
    fn reference_bomb_cooldown_blocks_the_retained_proximity_to_gun_early_fire() {
        let (mut state, mut fire) = bot_energy_fixture(0, 4);
        fire.weapon_data = state.bots[0].bomb_weapon_data().unwrap();
        assert_eq!(fire.weapon_data & 31, 4);
        spawn_bot_weapon(&mut state, 1178, fire);
        assert_eq!(state.active_projectiles.len(), 1);
        let energy = state.bots[0].energy;
        fire.weapon_data = state.bots[0].gun_weapon_data().unwrap();
        // Chaos BombFireDelay=150, with the intentional bot multiplier2.
        // The retained browser shot at1456 passed the wrong mine delay125.
        for tick in [1456, 1477] {
            spawn_bot_weapon(&mut state, tick, fire);
            assert_eq!(state.active_projectiles.len(), 1, "early shot at {tick}");
            assert_eq!(state.bots[0].energy, energy);
        }
        spawn_bot_weapon(&mut state, 1478, fire);
        assert_eq!(state.active_projectiles.len(), 2);
        assert_eq!(state.bots[0].energy, energy - 60);
    }

    #[test]
    fn bot_tactics_reject_self_blast_wall_low_energy_and_unhittable_bomb_shots() {
        let (mut state, _) = bot_energy_fixture(0, 1);
        let bot = &mut state.bots[0];
        bot.x = 1000.0;
        bot.y = 1000.0;
        bot.velocity_x = 0.0;
        bot.velocity_y = 0.0;
        bot.consecutive_gun_volleys = 2;
        let target = BotTarget {
            player_id: Some(1),
            frequency: Some(65535),
            x: 1000.0,
            y: 690.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        };
        let gun = bot.gun_weapon_data();
        assert!(matches!(
            super::bot_combat_weapon(bot, target, 100, |_, _| false).unwrap() & 31,
            3 | 4
        ));
        assert_eq!(
            super::bot_combat_weapon(bot, BotTarget { y: 900.0, ..target }, 100, |_, _| false),
            gun
        );
        assert_eq!(
            super::bot_combat_weapon(bot, target, 100, |_, y| y < 950.0),
            gun
        );
        assert_eq!(
            super::bot_combat_weapon(
                bot,
                BotTarget {
                    velocity_x: 1000.0,
                    ..target
                },
                100,
                |_, _| false
            ),
            gun
        );
        bot.velocity_y = -400.0;
        assert_eq!(
            super::bot_combat_weapon(bot, target, 100, |_, _| false),
            gun
        );
        bot.velocity_y = 0.0;
        bot.energy = bot.max_energy / 4;
        assert_eq!(
            super::bot_combat_weapon(bot, target, 100, |_, _| false),
            gun
        );
        bot.energy = bot.max_energy;
        bot.xradar = false;
        assert_eq!(
            super::bot_combat_weapon(
                bot,
                BotTarget {
                    status: crate::authority::POSITION_STATUS_CLOAK
                        | crate::authority::POSITION_STATUS_STEALTH,
                    ..target
                },
                100,
                |_, _| false
            ),
            gun
        );
    }

    #[test]
    fn bot_proximity_bomb_commit_checks_other_hostiles_and_keeps_energy_on_rejection() {
        let (mut state, fire) = bot_energy_fixture(0, 4);
        state.config.bot_count = 2;
        state.initialize_configured_bots();
        state.bots[0].x = 1000.0;
        state.bots[0].y = 1000.0;
        state.bots[1].x = 1030.0;
        state.bots[1].y = 1000.0;
        assert_ne!(
            state.bots[0].player.frequency,
            state.bots[1].player.frequency
        );
        let energy = state.bots[0].energy;
        spawn_bot_weapon(&mut state, 100, fire);
        assert!(state.active_projectiles.is_empty());
        assert_eq!(state.bots[0].energy, energy);
        assert!(state.bots[0].last_fire_tick.is_none());
        state.bots[1].x = 2000.0;
        spawn_bot_weapon(&mut state, 100, fire);
        assert_eq!(state.active_projectiles.len(), 1);
        assert!(state.bots[0].energy < energy);
    }

    #[test]
    fn bot_weapon_energy_charges_once_per_shot_and_rejects_insufficient_energy() {
        for ship in 0..8 {
            for (weapon, alternate) in [
                (1, false),
                (2, false),
                (3, false),
                (4, false),
                (3, true),
                (4, true),
            ] {
                let weapon_data =
                    crate::gameplay_rules::encode_weapon_data(weapon, 3, alternate, 0, 1, false);
                let (mut state, fire) = bot_energy_fixture(ship, weapon_data);
                let cost = crate::arena_settings::arena_ship_weapon_energy_cost(ship, weapon_data);
                assert!(cost > 0);
                let before = cost + 10;
                state.bots[0].energy = before;
                spawn_bot_weapon(&mut state, 100, fire);
                assert_eq!(state.bots[0].energy, 10, "ship={ship} weapon={weapon}");
                let expected_barrels = if ship == 4 && matches!(weapon, 1 | 2) {
                    2
                } else {
                    1
                };
                assert_eq!(state.active_projectiles.len(), expected_barrels);
                assert_eq!(state.bots[0].last_fire_tick, Some(100));
                state.bots[0].energy = cost - 1;
                let next_id = state.next_projectile_id;
                spawn_bot_weapon(&mut state, 200, fire);
                assert_eq!(state.bots[0].energy, cost - 1);
                assert_eq!(state.next_projectile_id, next_id);
                assert_eq!(state.bots[0].last_fire_tick, Some(100));
                assert_eq!(state.active_projectiles.len(), expected_barrels);
            }
        }
    }

    #[test]
    fn bot_fire_obeys_strict_fractional_energy_boundary_even_with_super() {
        let mut failures = Vec::new();
        for ship in 0..8 {
            for weapon in [2, 4] {
                let weapon_data =
                    crate::gameplay_rules::encode_weapon_data(weapon, 3, false, 0, 1, false);
                let cost = crate::arena_settings::arena_ship_weapon_energy_cost(ship, weapon_data);
                for super_active in [false, true] {
                    for (energy, remainder) in [
                        (0, 0.0),
                        (cost - 1, 0.9),
                        (cost, 0.0),
                        (cost, 0.25),
                        (cost + 1, 0.0),
                    ] {
                        let (mut state, fire) = bot_energy_fixture(ship, weapon_data);
                        state.bots[0].energy = energy;
                        state.bots[0].energy_remainder = remainder;
                        state.bots[0].super_until_tick = super_active.then_some(101);
                        // The reference game checks energy_cost < self.energy before
                        // Super waives payment, for both bullets and bombs.
                        let should_fire = energy as f32 + remainder > cost as f32;
                        spawn_bot_weapon(&mut state, 100, fire);
                        if !state.active_projectiles.is_empty() != should_fire {
                            failures.push(format!(
                                "ship={ship} weapon={weapon} super={super_active} energy={energy}+{remainder} required={cost} fired={}",
                                !state.active_projectiles.is_empty()
                            ));
                        }
                        let expected = if should_fire && !super_active {
                            energy - cost
                        } else {
                            energy
                        };
                        if state.bots[0].energy != expected
                            || (state.bots[0].energy_remainder - remainder).abs() > 0.00001
                            || state.bots[0].last_fire_tick != should_fire.then_some(100)
                        {
                            failures.push(format!(
                                "ship={ship} weapon={weapon} super={super_active}: energy/cooldown changed incorrectly"
                            ));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn bot_weapon_energy_capacity_and_shutdown_rejections_do_not_charge() {
        let (initial, fire) = bot_energy_fixture(0, 1);
        let state = std::sync::Arc::new(std::sync::Mutex::new(initial));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player, 0);
        let mut state = state.lock().unwrap();
        state.clients.get_mut(&player).unwrap().snapshot_ready = true;
        spawn_bot_weapon(&mut state, 100, fire);
        let source = state
            .active_projectiles
            .last()
            .unwrap()
            .presentation_source
            .unwrap();
        let recipient = state.active_player_runtime_identity(player).unwrap();
        let envelope = crate::snapshot::PendingRealtimeEventEnvelope {
            flight_transition: false,
            recipient,
            event_id: 42,
            server_tick: 100,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(source),
        };
        state
            .clients
            .get_mut(&player)
            .unwrap()
            .pending_realtime_events
            .queue = std::iter::repeat_n(
            envelope,
            crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
        )
        .collect();
        let energy = state.bots[0].energy;
        let next_id = state.next_projectile_id;
        spawn_bot_weapon(&mut state, 200, fire);
        assert_eq!(state.bots[0].energy, energy);
        assert_eq!(state.bots[0].last_fire_tick, Some(100));
        assert_eq!(state.next_projectile_id, next_id);
        state
            .clients
            .get_mut(&player)
            .unwrap()
            .pending_realtime_events
            .queue
            .clear();
        spawn_bot_weapon(&mut state, 200, fire);
        let cost = crate::arena_settings::arena_ship_weapon_energy_cost(0, fire.weapon_data);
        assert_eq!(state.bots[0].energy, energy - cost);
        assert_eq!(state.bots[0].last_fire_tick, Some(200));
        state.bots[0].engine_shutdown_until_tick = Some(400);
        spawn_bot_weapon(&mut state, 300, fire);
        assert_eq!(state.bots[0].energy, energy - cost);
        assert_eq!(state.bots[0].last_fire_tick, Some(200));
    }

    #[test]
    fn bot_emp_preserves_normal_weapon_eligibility_and_energy_charge() {
        let weapon = crate::gameplay_rules::encode_weapon_data(1, 3, false, 0, 1, false);
        let (mut state, fire) = bot_energy_fixture(0, weapon);
        let energy = state.bots[0].energy;
        state.bots[0].emp_until_tick = Some(400);
        assert!(state.bots[0].emp_active(100));
        spawn_bot_weapon(&mut state, 100, fire);
        let cost = crate::arena_settings::arena_ship_weapon_energy_cost(0, weapon);
        assert_eq!(state.bots[0].energy, energy - cost);
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(state.bots[0].last_fire_tick, Some(100));
    }

    #[test]
    fn bot_weapon_energy_super_exemption_ends_at_its_deadline() {
        let weapon_data = crate::gameplay_rules::encode_weapon_data(1, 3, false, 0, 1, false);
        let (mut state, fire) = bot_energy_fixture(0, weapon_data);
        let cost = crate::arena_settings::arena_ship_weapon_energy_cost(0, weapon_data);
        state.bots[0].energy = cost + 1;
        // Test the exact expiry after the real first-shot cooldown elapses.
        state.bots[0].super_until_tick = Some(200);
        spawn_bot_weapon(&mut state, 100, fire);
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(state.bots[0].energy, cost + 1);
        spawn_bot_weapon(&mut state, 200, fire);
        assert_eq!(state.active_projectiles.len(), 2);
        assert_eq!(state.bots[0].energy, 1);
        assert_eq!(state.bots[0].last_fire_tick, Some(200));
    }

    #[test]
    fn bot_weapon_energy_near_whole_fraction_matches_shared_human_arithmetic() {
        let weapon = crate::gameplay_rules::encode_weapon_data(2, 3, false, 0, 1, false);
        let (mut state, fire) = bot_energy_fixture(6, weapon);
        let cost = crate::arena_settings::arena_ship_weapon_energy_cost(6, weapon);
        assert_eq!(cost, 60);
        let bot = &mut state.bots[0];
        bot.energy = 1042;
        bot.energy_remainder = f32::from_bits(0x3f7f_ffff);
        let mut human =
            crate::authority::EnergyPool::new(bot.energy, bot.max_energy, bot.energy_remainder);
        assert_eq!(human.available(), 1043.0);
        assert!(human.can_spend_weapon_energy(f32::from(cost)));
        human.spend_weapon_energy(f32::from(cost));
        spawn_bot_weapon(&mut state, 100, fire);
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(state.bots[0].energy, human.current());
        assert_eq!(state.bots[0].energy_remainder, human.recharge_remainder());
        assert_eq!(state.bots[0].energy, 983);
        assert_eq!(state.bots[0].energy_remainder, 0.0);
        assert_ne!(state.bots[0].energy, 1042 - cost);
    }

    #[test]
    fn bot_weapon_energy_sustained_fire_accounts_for_configured_recharge() {
        for ship in 0..8 {
            let (mut state, mut fire) = bot_energy_fixture(ship, 1);
            // This probe isolates weapon cost and regeneration; detector
            // status drain has its own production energy-order regression.
            state.bots[0].xradar = false;
            fire.weapon_data = state.bots[0].gun_weapon_data().unwrap();
            let bot = &state.bots[0];
            let cost = crate::arena_settings::arena_ship_weapon_energy_cost(ship, fire.weapon_data);
            let cooldown = crate::arena_settings::arena_ship_weapon_fire_delay_ticks(
                ship,
                crate::continuum_weapon::continuum_weapon_type(fire.weapon_data),
            )
            .saturating_mul(super::BOT_FIRE_COOLDOWN_MULTIPLIER);
            let recharge = bot.recharge_per_second;
            let maximum = bot.max_energy;
            let mut total_spent = 0u32;
            for shot in 0..20 {
                let before = state.bots[0].energy;
                spawn_bot_weapon(&mut state, 100 + shot * cooldown, fire);
                assert_eq!(state.bots[0].energy, before - cost);
                total_spent += u32::from(cost);
                let bot = &mut state.bots[0];
                let mut pool = crate::authority::EnergyPool::new(
                    bot.energy,
                    bot.max_energy,
                    bot.energy_remainder,
                );
                pool.recharge_for_position_ticks(cooldown, recharge);
                bot.energy = pool.current();
                bot.energy_remainder = pool.recharge_remainder();
            }
            println!(
                "bot-energy ship={} max={} gun_cost={} cooldown_ticks={} recharge_per_second={} spent_20_shots={} energy_after_recharge={}",
                ship + 1,
                maximum,
                cost,
                cooldown,
                recharge,
                total_spent,
                state.bots[0].energy
            );
        }
    }

    #[test]
    fn bot_fire_ignores_codec_ack_and_opens_at_the_bounded_server_deadline() {
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut guard = state.lock().expect("state");
        let bot_id = guard.bots[0].player.id;
        let bot_life = guard.bots[0].life_seq;
        let source_runtime = guard.bots[0].runtime_identity();
        let fire = PendingBotFire {
            bot_id,
            life_seq: bot_life,
            source_runtime,
            target_human_player_id: Some(player_id),
            x: guard.bots[0].x,
            y: guard.bots[0].y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            direction: 0,
            weapon_data: 66,
        };
        let energy_before = guard.bots[0].energy;
        let energy_cost = crate::arena_settings::arena_ship_weapon_energy_cost(
            guard.bots[0].player.ship,
            fire.weapon_data,
        );
        spawn_bot_weapon(&mut guard, 100, fire);
        assert_eq!(guard.bots[0].energy, energy_before);
        assert!(
            guard.active_projectiles.is_empty(),
            "harm is closed before the exact source life is emitted"
        );

        let snapshot_id = 17;
        let client = guard.clients.get_mut(&player_id).expect("client");
        let contract = client
            .threat_presentation
            .observe_emitted(source_runtime, snapshot_id, 101);
        // Preserve a codec ACK to prove it is deliberately irrelevant to this
        // separately typed presentation contract.
        client.last_acked_snapshot_id = Some(snapshot_id);
        spawn_bot_weapon(&mut guard, contract.conservative_open_tick, fire);
        assert!(guard.active_projectiles.is_empty());
        spawn_bot_weapon(&mut guard, contract.hard_open_tick - 1, fire);
        assert!(guard.active_projectiles.is_empty());
        assert_eq!(guard.bots[0].energy, energy_before);
        spawn_bot_weapon(&mut guard, contract.hard_open_tick, fire);
        assert_eq!(guard.bots[0].energy, energy_before - energy_cost);
        assert_eq!(guard.active_projectiles.len(), 1);

        guard.bots[0].advance_life();
        let replacement = guard.bots[0].runtime_identity();
        let replacement_fire = PendingBotFire {
            bot_id,
            life_seq: replacement.life.life_seq,
            source_runtime: replacement,
            ..fire
        };
        spawn_bot_weapon(&mut guard, contract.hard_open_tick + 1, replacement_fire);
        assert!(
            guard.active_projectiles.len() == 1,
            "a respawned bot requires a new exact-life emission contract"
        );
    }

    #[test]
    fn bot_bullets_spawn_on_the_bot_centreline_for_every_cardinal_heading() {
        // The report was "bot bullets sometimes emerge from the left or right of the bot's ship rather than its peak".
        // Bots fly single-barrel ship 0, whose authored layout is RulesVector::ZERO lateral offset - the only Chaos
        // ship with a sideways muzzle is the double-barrel profile (index 4), and no bot becomes it. Pin the
        // single-barrel case across the four cardinal headings so a muzzle-offset regression cannot hide. The fire
        // decision itself sets `x`/`y` from the bot's own position (`step_server_bots`), and this asserts the
        // authoritative projectile inherits exactly that origin with no lateral shift.
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);

        let mut guard = state.lock().expect("state");
        let bot_id = guard.bots[0].player.id;
        let bot_life = guard.bots[0].life_seq;
        let source_runtime = guard.bots[0].runtime_identity();
        assert_eq!(
            guard.bots[0].player.ship, 0,
            "the reported bot is the single-barrel profile, so any lateral offset would be a bug"
        );
        let snapshot_id = 41;
        let contract = guard
            .clients
            .get_mut(&player_id)
            .expect("joined observer")
            .threat_presentation
            .observe_emitted(source_runtime, snapshot_id, 101);

        for (step, (x, y)) in [
            (1000.0_f32, 2000.0_f32),
            (1400.0, 2000.0),
            (1800.0, 2000.0),
            (2200.0, 2000.0),
        ]
        .into_iter()
        .enumerate()
        {
            let fire = PendingBotFire {
                bot_id,
                life_seq: bot_life,
                source_runtime,
                target_human_player_id: Some(player_id),
                x,
                y,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: step as f32 * 0.25,
                direction: (step as u8) * 10,
                weapon_data: 66,
            };
            let before = guard.active_projectiles.len();
            let delay = crate::arena_settings::arena_ship_weapon_fire_delay_ticks(0, 2)
                * super::BOT_FIRE_COOLDOWN_MULTIPLIER;
            spawn_bot_weapon(
                &mut guard,
                contract.hard_open_tick + 1 + step as u32 * delay,
                fire,
            );
            assert_eq!(
                guard.active_projectiles.len(),
                before + 1,
                "one single-barrel shot per launch"
            );
            let shot = guard
                .active_projectiles
                .last()
                .expect("the launch records a projectile");
            assert_eq!(
                shot.pos_x, x,
                "single-barrel bot bullet must spawn on the centreline, not beside the ship"
            );
            assert_eq!(shot.pos_y, y);
        }
    }

    #[test]
    fn movement_trace_samples_idle_motion_but_retains_every_safety_edge() {
        let bot_id = crate::state::SERVER_BOT_ID_BASE;
        let sampled = (0_u16..20)
            .filter(|sequence| bot_movement_trace_due(*sequence, bot_id, false, false, false, true))
            .collect::<Vec<_>>();
        assert_eq!(sampled.len(), 4);
        assert!(bot_movement_trace_due(1, bot_id, true, false, false, true));
        assert!(bot_movement_trace_due(1, bot_id, false, true, false, true));
        assert!(bot_movement_trace_due(1, bot_id, false, false, true, true));
        assert!(bot_movement_trace_due(
            1, bot_id, false, false, false, false
        ));
    }

    #[test]
    fn repeated_logical_tick_is_an_idempotent_bot_noop() {
        let state = std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        }));
        state.lock().expect("state").initialize_configured_bots();

        let first = step_server_bots(&state, 100);
        let after_first = state.lock().expect("state").bots[0].clone();
        let duplicate = step_server_bots(&state, 100);
        let after_duplicate = state.lock().expect("state").bots[0].clone();

        assert_eq!(first.positions.len(), 1);
        assert_eq!(duplicate.positions.len(), 1);
        assert!(duplicate.positions[0].public_relay_suppressed);
        assert_eq!(duplicate.positions[0].position, first.positions[0].position);
        assert!(duplicate.reliable_packets.is_empty());
        assert_eq!(after_duplicate, after_first);
    }

    fn isolated_solid_tile_with_open_upper_left() -> (usize, usize) {
        let map = crate::zone::zone_map();
        let blocked = |x: usize, y: usize| {
            map.tiles
                .get(y * crate::zone::MAP_TILES + x)
                .is_none_or(|&tile| crate::zone::position_blocking_tile_for_door_seeds(tile, &[]))
        };
        for y in 2..crate::zone::MAP_TILES - 1 {
            for x in 2..crate::zone::MAP_TILES - 1 {
                if blocked(x, y)
                    && !blocked(x - 1, y)
                    && !blocked(x, y - 1)
                    && !blocked(x - 1, y - 1)
                {
                    return (x, y);
                }
            }
        }
        panic!("authored map must contain an exposed wall corner");
    }

    #[test]
    fn bot_hull_rejects_diagonal_wall_corner_overlap() {
        let map = crate::zone::zone_map();
        let (tile_x, tile_y) = isolated_solid_tile_with_open_upper_left();
        let radius = 14.0;
        let x = tile_x as f32 * TILE_SIZE_PX - radius + 1.0;
        let y = tile_y as f32 * TILE_SIZE_PX - radius + 1.0;
        assert!(
            !bot_hull_is_clear(x, y, radius, map, &[], &Default::default()),
            "the complete player-first AABB catches the one-pixel diagonal corner overlap"
        );
    }

    #[test]
    fn bot_live_motion_uses_shared_axis_wall_collision() {
        let map = crate::zone::zone_map();
        let (tile_x, tile_y) = isolated_solid_tile_with_open_upper_left();
        let radius = 14.0;
        let start = MovementVector::new(
            tile_x as f32 * TILE_SIZE_PX - radius - 1.0,
            tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
        );
        let bricks = Default::default();
        assert!(bot_hull_is_clear(
            start.x,
            start.y,
            radius,
            map,
            &[],
            &bricks
        ));
        let result = integrate_player_collision(
            start,
            MovementVector::new(240.0, 0.0),
            1.0 / 60.0,
            0.35,
            |candidate, axis, signed_delta| {
                bot_axis_overlaps_blocked_tile(
                    candidate,
                    radius,
                    map,
                    &[],
                    &bricks,
                    axis,
                    signed_delta,
                    false,
                )
            },
        );
        assert_eq!(result.position.x.to_bits(), start.x.to_bits());
        assert!(result.bounced);
        assert!(result.velocity.x < 0.0);
        assert!(bot_axis_overlaps_blocked_tile(
            MovementVector::new(start.x + 4.0, start.y),
            radius,
            map,
            &[],
            &bricks,
            CollisionAxis::X,
            4.0,
            false,
        ));
    }

    #[test]
    fn bot_diagonal_motion_cannot_enter_wall_corner() {
        let map = crate::zone::zone_map();
        let (tile_x, tile_y) = isolated_solid_tile_with_open_upper_left();
        let radius = 14.0;
        let start = MovementVector::new(
            tile_x as f32 * TILE_SIZE_PX - radius - 1.0,
            tile_y as f32 * TILE_SIZE_PX - radius - 1.0,
        );
        let bricks = Default::default();
        assert!(bot_hull_is_clear(
            start.x,
            start.y,
            radius,
            map,
            &[],
            &bricks
        ));
        let result = integrate_player_collision(
            start,
            MovementVector::new(240.0, 240.0),
            1.0 / 60.0,
            0.35,
            |candidate, axis, signed_delta| {
                bot_axis_overlaps_blocked_tile(
                    candidate,
                    radius,
                    map,
                    &[],
                    &bricks,
                    axis,
                    signed_delta,
                    false,
                )
            },
        );
        assert!(result.bounced);
        assert!(bot_hull_is_clear(
            result.position.x,
            result.position.y,
            radius,
            map,
            &[],
            &bricks
        ));
    }

    #[test]
    fn bot_navigation_probe_rejects_a_clear_endpoint_beyond_an_intervening_wall() {
        let map = crate::zone::zone_map_for_id("extreme").expect("embedded Extreme map");
        let blocked = |x: usize, y: usize| {
            map.tiles
                .get(y * crate::zone::MAP_TILES + x)
                .is_none_or(|&tile| crate::zone::position_blocking_tile_for_door_seeds(tile, &[]))
        };
        let (wall_x, wall_y) = (2..crate::zone::MAP_TILES - 2)
            .flat_map(|y| (2..crate::zone::MAP_TILES - 2).map(move |x| (x, y)))
            .find(|&(x, y)| {
                blocked(x, y)
                    && (y - 1..=y + 1).all(|sample_y| {
                        !blocked(x - 2, sample_y)
                            && !blocked(x - 1, sample_y)
                            && !blocked(x + 1, sample_y)
                            && !blocked(x + 2, sample_y)
                    })
            })
            .expect("Extreme must retain a thin wall with open space on both sides");
        let radius = 14.0;
        let start_x = wall_x as f32 * TILE_SIZE_PX - radius - 1.0;
        let y = (wall_y as f32 + 0.5) * TILE_SIZE_PX;
        let distance = radius * 2.0 + TILE_SIZE_PX + 2.0;
        let end_x = start_x + distance;
        let bricks = Default::default();
        assert!(bot_hull_is_clear(start_x, y, radius, map, &[], &bricks));
        assert!(bot_hull_is_clear(end_x, y, radius, map, &[], &bricks));
        assert!(!bot_hull_segment_is_clear(
            start_x,
            y,
            1.0,
            0.0,
            distance,
            radius,
            map,
            &[],
            &bricks,
        ));
    }

    #[test]
    fn bot_wall_collision_builds_stable_tangential_escape_velocity() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                default_zone: "extreme".to_string(),
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let bot = state.bots.first_mut().expect("configured bot");
        bot.velocity_x = 1.25;
        bot.velocity_y = -0.8;
        let before_y = bot.velocity_y;
        apply_bot_wall_following_impulse(
            bot,
            super::BotCollisionStep {
                collision_x: true,
                ..super::BotCollisionStep::default()
            },
            10,
            None,
        );
        assert!(
            bot.velocity_y > before_y,
            "the retained Extreme vertical-wall trace must keep one stable tangential direction"
        );
        assert_eq!(bot.wall_follow_until_tick, Some(210));
    }

    #[test]
    fn bot_wall_following_chooses_the_tangent_toward_the_authoritative_target() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                default_zone: "chaos-zone".to_string(),
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let bot = state.bots.first_mut().expect("configured bot");
        bot.x = 8_150.0;
        bot.y = 8_097.0;
        bot.velocity_x = 1.0;
        bot.velocity_y = 1.0;
        let start_x = bot.x;
        let start_y = bot.y;

        apply_bot_wall_following_impulse(
            bot,
            super::BotCollisionStep {
                start_x,
                start_y,
                attempted_x: start_x,
                attempted_y: start_y + 1.0,
                collision_y: true,
                ..super::BotCollisionStep::default()
            },
            10,
            Some((8_392.0, 8_516.0)),
        );

        assert_eq!(
            bot.wall_follow_x, 1,
            "the failed Firefox lane requires the eastward tangent toward the human, not bot-id parity"
        );
        assert_eq!(
            bot.wall_follow_y, -1,
            "the collision axis must still move away from the horizontal wall"
        );
        assert!(bot.velocity_x > 0.0);
    }

    #[test]
    fn bounded_bot_motion_cannot_skip_a_thin_brick_wall() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let map = state.zone_map();
        let (cx, cy) = (9..crate::zone::MAP_TILES - 9)
            .flat_map(|y| (9..crate::zone::MAP_TILES - 9).map(move |x| (x, y)))
            .find(|&(x, y)| {
                (y - 8..=y + 8).all(|row| {
                    (x - 8..=x + 8).all(|col| map.tiles[row * crate::zone::MAP_TILES + col] == 0)
                })
            })
            .expect("open collision fixture region");
        let center_x = (cx as f32 + 0.5) * TILE_SIZE_PX;
        let center_y = (cy as f32 + 0.5) * TILE_SIZE_PX;
        let bot = &mut state.bots[0];
        for horizontal in [false, true] {
            for sign in [-1.0, 1.0] {
                let bricks = (-3..=3)
                    .map(|offset| {
                        if horizontal {
                            (cx as u16, (cy as i32 + offset) as u16)
                        } else {
                            ((cx as i32 + offset) as u16, cy as u16)
                        }
                    })
                    .collect();
                bot.x = center_x - if horizontal { sign * 48.0 } else { 0.0 };
                bot.y = center_y - if horizontal { 0.0 } else { sign * 48.0 };
                bot.velocity_x = 0.0;
                bot.velocity_y = 0.0;
                bot.wall_follow_until_tick = Some(200);
                bot.wall_follow_x = -1;
                bot.wall_follow_y = -1;
                let toward_wall = if horizontal {
                    if sign > 0.0 { 0.25 } else { 0.75 }
                } else if sign > 0.0 {
                    0.5
                } else {
                    0.0
                };
                super::release_bot_wall_following_for_clear_route(
                    bot,
                    toward_wall,
                    map,
                    &[],
                    &bricks,
                );
                assert_eq!(
                    bot.wall_follow_until_tick,
                    Some(200),
                    "a blocked desired route must retain the detour"
                );
                super::release_bot_wall_following_for_clear_route(
                    bot,
                    (toward_wall + 0.5) % 1.0,
                    map,
                    &[],
                    &bricks,
                );
                assert_eq!(
                    bot.wall_follow_until_tick, None,
                    "a clear desired route must release the detour before its deadline"
                );
                assert_eq!(
                    (bot.velocity_x, bot.velocity_y),
                    (0.0, 0.0),
                    "release must not inject motion"
                );
                bot.velocity_x = if horizontal { sign * 1_200.0 } else { 0.0 };
                bot.velocity_y = if horizontal { 0.0 } else { sign * 1_200.0 };
                let collision = super::move_bot_with_collision(bot, 0.1, map, &[], &bricks);
                assert!(
                    collision.collision_x || collision.collision_y,
                    "a bounded motion turn must detect the intermediate wall"
                );
                let displacement = if horizontal {
                    bot.x - center_x
                } else {
                    bot.y - center_y
                };
                assert!(displacement * sign < 0.0, "bot crossed the brick wall");
            }
        }
    }

    #[test]
    fn closed_door_relocation_preserves_bot_life_loadout_and_owned_flight_once() {
        let weapon = crate::gameplay_rules::encode_weapon_data(1, 3, false, 0, 0, false);
        let (mut state, fire) = bot_energy_fixture(0, weapon);
        spawn_bot_weapon(&mut state, 100, fire);
        assert!(!state.active_projectiles.is_empty());
        let owned_flight = state.active_projectiles.clone();
        let tick = (10_000..12_000)
            .find(|&tick| {
                state
                    .server_door_seeds(tick)
                    .iter()
                    .any(|&seed| crate::zone::position_blocking_tile_for_door_seeds(169, &[seed]))
            })
            .expect("a closed Chaos door phase");
        let bot = &mut state.bots[0];
        bot.x = 513.5 * TILE_SIZE_PX;
        bot.y = 507.5 * TILE_SIZE_PX;
        bot.velocity_x = 160.0;
        bot.velocity_y = -100.0;
        bot.last_step_tick = tick - 1;
        bot.recharge_per_second = 0.0;
        bot.xradar = false;
        bot.energy = 1234;
        bot.wall_follow_until_tick = Some(tick + 50);
        bot.wall_follow_x = 1;
        bot.safe_egress_path.push_back((513, 506));
        bot.actor_motion_history = Default::default();
        bot.begin_actor_motion_segment();
        assert_eq!(
            bot.append_actor_motion_sample(tick - 1, false),
            crate::combat_timeline::HistoryAppendDisposition::Applied
        );
        let before = bot.clone();
        let previous_segment = before.actor_motion_history.current_segment().unwrap();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(state));
        step_server_bots(&shared, tick);
        let after = shared.lock().unwrap().bots[0].clone();
        assert_ne!((after.x, after.y), (before.x, before.y));
        assert_eq!((after.velocity_x, after.velocity_y), (0.0, 0.0));
        assert_eq!(after.runtime_identity(), before.runtime_identity());
        assert_eq!(after.player, before.player);
        assert_eq!(after.energy, before.energy);
        assert_eq!(after.weapons, before.weapons);
        assert_eq!(after.status, before.status);
        assert_eq!(after.bounty, before.bounty);
        assert!(after.safe_egress_path.is_empty());
        assert!(after.wall_follow_until_tick.is_none());
        let new_segment = after.actor_motion_history.current_segment().unwrap();
        assert_eq!(
            new_segment.discontinuity_generation,
            previous_segment.discontinuity_generation + 1
        );
        assert_eq!(
            after.actor_motion_history.current_samples().count(),
            1,
            "one new-segment sample"
        );
        assert_eq!(
            after
                .actor_motion_history
                .publication_sample_at(tick)
                .unwrap()
                .segment,
            new_segment
        );
        let mut locked = shared.lock().unwrap();
        let seeds = locked.server_door_seeds(tick);
        assert!(bot_hull_is_clear_for_test(
            &after,
            locked.zone_map(),
            &seeds,
            &Default::default()
        ));
        assert_eq!(locked.active_projectiles, owned_flight);
        drop(locked);
        step_server_bots(&shared, tick);
        assert_eq!(
            shared.lock().unwrap().bots[0],
            after,
            "duplicate tick cannot re-warp"
        );
        assert_eq!(shared.lock().unwrap().active_projectiles, owned_flight);
    }

    #[test]
    fn door_overlap_uses_live_hull_and_relocation_never_accepts_an_unverified_fallback() {
        let (mut state, _) = bot_energy_fixture(0, 0);
        let map = crate::zone::zone_map();
        let bot = &mut state.bots[0];
        let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship);
        // A tiny overlap would be lost by truncating before testing the hull.
        bot.x = 513.5 * TILE_SIZE_PX;
        bot.y = 507.0 * TILE_SIZE_PX - radius + 0.25;
        assert!(bot_overlaps_closed_door(bot, map, &[255]));
        assert!(!bot_overlaps_closed_door(bot, map, &[0]));
        let original = (bot.x, bot.y);
        let segment = bot.actor_motion_history.current_segment();
        for destination in [None, Some((0, 0)), Some((513 * 16 + 8, 507 * 16 + 8))] {
            bot.velocity_x = 100.0;
            assert!(!apply_bot_door_relocation(
                bot,
                destination,
                map,
                &[255],
                &Default::default()
            ));
            assert_eq!((bot.x, bot.y), original);
            assert_eq!(bot.actor_motion_history.current_segment(), segment);
            assert_eq!(bot.velocity_x, 0.0);
        }
    }

    #[test]
    fn bot_routes_follow_authoritative_animated_door_phase() {
        let map = crate::zone::zone_map();
        let bricks = Default::default();
        let door_x = 513_i32;
        let door_y = 507_i32;
        let door_tile = map.tiles[door_y as usize * crate::zone::MAP_TILES + door_x as usize];
        assert_eq!(
            door_tile, 169,
            "fixture must remain the Chaos center door row"
        );
        assert!(
            !crate::zone::position_blocking_tile_for_door_seeds(door_tile, &[0]),
            "seed zero presents this animated door as open to human collision"
        );
        assert!(!bot_tile_is_blocked(door_x, door_y, map, &[0], &bricks));
        assert!(bot_tile_is_blocked(door_x, door_y, map, &[255], &bricks));

        let radius = 14.0;
        let start = MovementVector::new(
            (door_x as f32 + 0.5) * TILE_SIZE_PX,
            door_y as f32 * TILE_SIZE_PX - radius - 1.0,
        );
        for (seed, blocked) in [(0, false), (255, true)] {
            let result = integrate_player_collision(
                start,
                MovementVector::new(0.0, 240.0),
                1.0 / 60.0,
                0.35,
                |candidate, axis, signed_delta| {
                    bot_axis_overlaps_blocked_tile(
                        candidate,
                        radius,
                        map,
                        &[seed],
                        &bricks,
                        axis,
                        signed_delta,
                        false,
                    )
                },
            );
            assert_eq!(result.bounced, blocked, "seed {seed}");
            if blocked {
                assert_eq!(result.position.y.to_bits(), start.y.to_bits());
            } else {
                assert!(result.position.y > start.y);
            }
            assert!(bot_hull_is_clear(
                result.position.x,
                result.position.y,
                radius,
                map,
                &[seed],
                &bricks,
            ));
        }
    }

    #[test]
    fn extreme_bot_outside_safety_cannot_reenter_and_recharge_between_hits() {
        let map = crate::zone::zone_map_for_id("extreme").expect("embedded Extreme map");
        let (safe_x, safe_y) = (1..crate::zone::MAP_TILES - 1)
            .flat_map(|y| (2..crate::zone::MAP_TILES - 1).map(move |x| (x, y)))
            .find(|&(x, y)| {
                bot_tile_is_safe(x as i32, y as i32, map)
                    && !bot_tile_is_blocked(x as i32 - 1, y as i32, map, &[], &Default::default())
                    && !bot_tile_is_safe(x as i32 - 1, y as i32, map)
            })
            .expect("Extreme must retain an exposed authored safe boundary");
        let radius = 14.0;
        let start = MovementVector::new(
            safe_x as f32 * TILE_SIZE_PX - 1.0,
            (safe_y as f32 + 0.5) * TILE_SIZE_PX,
        );
        let candidate = MovementVector::new(start.x + 4.0, start.y);
        let bricks = Default::default();

        assert!(
            !bot_hull_segment_is_clear(start.x, start.y, 1.0, 0.0, 4.0, radius, map, &[], &bricks),
            "navigation must reject the same safe entry as actual bot movement"
        );

        assert!(!bot_axis_overlaps_blocked_tile(
            candidate,
            radius,
            map,
            &[],
            &bricks,
            CollisionAxis::X,
            4.0,
            false,
        ));
        assert!(bot_axis_overlaps_blocked_tile(
            candidate,
            radius,
            map,
            &[],
            &bricks,
            CollisionAxis::X,
            4.0,
            true,
        ));
        assert!(!bot_axis_overlaps_blocked_tile(
            MovementVector::new(start.x - 4.0, start.y),
            radius,
            map,
            &[],
            &bricks,
            CollisionAxis::X,
            -4.0,
            true,
        ));
    }

    #[test]
    fn extreme_safe_egress_uses_a_hull_clear_route_around_the_doomsy_wall_corner() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                default_zone: "extreme".to_string(),
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let map = state.zone_map();
        let bot = state.bots.first_mut().expect("configured bot");
        // Retained Chrome/Extreme authority evidence pinned this exact safe
        // position. The old cardinal destination search selected a clear
        // tile south of the nearby wall but could not reach it, leaving the
        // bot permanently protected from human damage.
        bot.x = 8_376.0;
        bot.y = 8_386.0;
        let radius = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship).max(1.0);
        let bricks = Default::default();
        assert!(crate::zone::world_position_in_safe_tile_for_map(
            map,
            bot.x as u16,
            bot.y as u16,
        ));

        let started = std::time::Instant::now();
        let path = combat_tile_path(bot, map, &[], &bricks);
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "bounded egress planning must stay outside the tick budget tail: {elapsed:?}"
        );
        assert!(
            path.len() > 1 && path.len() <= 128,
            "fixture must exercise a bounded multi-step wall-corner egress: {path:?}"
        );
        let mut previous = (bot.x, bot.y);
        for (x, y) in path {
            assert!(
                bot_hull_is_clear(f32::from(x), f32::from(y), radius, map, &[], &bricks),
                "every returned egress waypoint must clear the complete ship hull"
            );
            assert!(
                (f32::from(x) - previous.0).hypot(f32::from(y) - previous.1) <= TILE_SIZE_PX * 1.6,
                "egress must return the first reachable step rather than an occluded destination"
            );
            previous = (f32::from(x), f32::from(y));
        }
        assert!(!crate::zone::world_position_in_safe_tile_for_map(
            map,
            previous.0.round() as u16,
            previous.1.round() as u16,
        ));
    }

    #[test]
    fn server_bot_prefers_hostile_human_over_closer_bot() {
        let humans = [BotTarget {
            player_id: Some(7),
            frequency: Some(2),
            x: 100.0,
            y: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        }];
        let bots = [BotTarget {
            player_id: Some(768),
            frequency: Some(3),
            x: 1.0,
            y: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        }];

        let target = nearest_hostile_target(769, 1, 0.0, 0.0, &humans, &bots, false)
            .expect("hostile target");

        assert_eq!(target.player_id, Some(7));
    }

    #[test]
    fn configured_bots_assign_at_most_one_hostile_per_live_human() {
        let humans = [
            BotTarget {
                player_id: Some(7),
                frequency: Some(2),
                x: 100.0,
                y: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            },
            BotTarget {
                player_id: Some(9),
                frequency: Some(4),
                x: 200.0,
                y: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            },
        ];

        assert_eq!(
            assigned_hostile_human_target(0, 768, &humans, false)
                .and_then(|target| target.player_id),
            Some(7)
        );
        assert_eq!(
            assigned_hostile_human_target(1, 769, &humans, false)
                .and_then(|target| target.player_id),
            Some(9)
        );
        assert!(assigned_hostile_human_target(2, 770, &humans, false).is_none());
        assert!(assigned_hostile_human_target(0, 2, &humans, false).is_none());

        let mut concealed = humans;
        concealed[0].status =
            crate::authority::POSITION_STATUS_STEALTH | crate::authority::POSITION_STATUS_CLOAK;
        assert!(assigned_hostile_human_target(0, 768, &concealed, false).is_none());
        assert_eq!(
            assigned_hostile_human_target(1, 769, &concealed, false)
                .unwrap()
                .player_id,
            Some(9)
        );
        assert!(assigned_hostile_human_target(2, 770, &concealed, false).is_none());
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 3,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        assert_eq!(
            human_assigned_bot_ids(&state.bots, &concealed, 0)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![768, 769]
        );
    }

    #[test]
    fn life_rotation_spreads_hunters_without_stacking_or_revealing_concealed_pilots() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 15,
                ..Default::default()
            },
            ..Default::default()
        };
        state.initialize_configured_bots();
        let human = BotTarget {
            player_id: Some(7),
            frequency: Some(0),
            x: 8200.0,
            y: 8200.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        };
        let mut seen = std::collections::BTreeSet::new();
        for deaths in 0..30_u16 {
            let rotation = super::human_assignment_rotation(
                [u32::from(deaths) + 1].into_iter(),
                state.bots.len(),
            );
            let assigned = human_assigned_bot_ids(&state.bots, &[human], rotation);
            assert_eq!(assigned.len(), 1, "one human still gets at most one hunter");
            assert_eq!(assigned.first().copied(), Some(768 + deaths % 15));
            seen.extend(assigned);
            let concealed = BotTarget {
                status: crate::authority::POSITION_STATUS_STEALTH
                    | crate::authority::POSITION_STATUS_CLOAK,
                ..human
            };
            assert_eq!(
                human_assigned_bot_ids(&state.bots, &[concealed], rotation).len(),
                1,
                "concealment retains the reserved slot"
            );
            for (i, bot) in state.bots.iter().enumerate() {
                let slot = super::human_assignment_slot(i, state.bots.len(), rotation);
                let visible =
                    assigned_hostile_human_target(slot, bot.player.frequency, &[human], false);
                let hidden =
                    assigned_hostile_human_target(slot, bot.player.frequency, &[concealed], false);
                let xradar =
                    assigned_hostile_human_target(slot, bot.player.frequency, &[concealed], true);
                assert!(hidden.is_none());
                assert_eq!(visible.is_some(), xradar.is_some());
            }
        }
        assert_eq!(
            seen.len(),
            15,
            "every configured bot gets a turn across 15 human deaths"
        );
        for deaths in [0, 1, 14, 15, u16::MAX] {
            let humans = [
                human,
                BotTarget {
                    player_id: Some(9),
                    frequency: Some(1),
                    ..human
                },
            ];
            let rotation = super::human_assignment_rotation(
                [u32::from(deaths) + 1, 4].into_iter(),
                state.bots.len(),
            );
            let assigned = human_assigned_bot_ids(&state.bots, &humans, rotation);
            assert_eq!(assigned.len(), 2);
            let bot_targets = state
                .bots
                .iter()
                .map(|b| BotTarget {
                    player_id: Some(b.player.id),
                    frequency: Some(b.player.frequency),
                    ..human
                })
                .collect::<Vec<_>>();
            assert!(
                surplus_bot_targets(&bot_targets, &assigned)
                    .iter()
                    .all(|t| !assigned.contains(&t.player_id.unwrap()))
            );
        }
        for count in [0, 1, 2, 14, 15, 16, 30] {
            let humans = (0..count)
                .map(|i| BotTarget {
                    player_id: Some(100 + i),
                    frequency: Some(i),
                    ..human
                })
                .collect::<Vec<_>>();
            for rotation in 0..15 {
                let mut targeted = std::collections::BTreeSet::new();
                for (index, bot) in state.bots.iter().enumerate() {
                    let slot = super::human_assignment_slot(index, state.bots.len(), rotation);
                    if let Some(target) =
                        assigned_hostile_human_target(slot, bot.player.frequency, &humans, true)
                    {
                        assert!(
                            targeted.insert(target.player_id.unwrap()),
                            "two hunters cannot share one human"
                        );
                    }
                }
                assert_eq!(targeted.len(), usize::from(count).min(state.bots.len()));
            }
        }
        assert_eq!(
            super::human_assignment_rotation([u32::MAX].into_iter(), 0),
            0
        );
    }

    #[test]
    fn bot_target_acquisition_respects_combined_stealth_and_cloak() {
        use crate::authority::{POSITION_STATUS_CLOAK, POSITION_STATUS_STEALTH};
        for concealed_status in [
            0,
            POSITION_STATUS_STEALTH,
            POSITION_STATUS_CLOAK,
            POSITION_STATUS_STEALTH | POSITION_STATUS_CLOAK,
        ] {
            for other_bits in [
                0,
                crate::authority::POSITION_STATUS_FLASH,
                crate::authority::POSITION_STATUS_SAFETY,
            ] {
                let status = concealed_status | other_bits;
                let humans = [BotTarget {
                    player_id: Some(7),
                    frequency: Some(2),
                    x: 100.0,
                    y: 0.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    in_safe_zone: false,
                    status,
                }];
                for xradar in [false, true] {
                    let target = assigned_hostile_human_target(0, 768, &humans, xradar);
                    assert_eq!(
                        target.is_some(),
                        xradar
                            || concealed_status
                                != (POSITION_STATUS_STEALTH | POSITION_STATUS_CLOAK),
                        "detection must depend on active X-radar and visible ship/radar surfaces; status={status} xradar={xradar}"
                    );
                    assert_eq!(
                        nearest_hostile_target(768, 768, 0.0, 0.0, &humans, &[], xradar).is_some(),
                        target.is_some()
                    );
                }
            }
        }
    }

    #[test]
    fn surplus_bots_cannot_target_the_human_assigned_bot() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 2,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let humans = [BotTarget {
            player_id: Some(7),
            frequency: Some(0),
            x: 8_200.0,
            y: 8_200.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        }];
        let bot_targets = state
            .bots
            .iter()
            .map(|bot| BotTarget {
                player_id: Some(bot.player.id),
                frequency: Some(bot.player.frequency),
                x: bot.x,
                y: bot.y,
                velocity_x: bot.velocity_x,
                velocity_y: bot.velocity_y,
                in_safe_zone: false,
                status: 0,
            })
            .collect::<Vec<_>>();

        let assigned = human_assigned_bot_ids(&state.bots, &humans, 0);
        let surplus = surplus_bot_targets(&bot_targets, &assigned);

        assert_eq!(assigned.into_iter().collect::<Vec<_>>(), vec![768]);
        assert_eq!(
            surplus
                .into_iter()
                .filter_map(|target| target.player_id)
                .collect::<Vec<_>>(),
            vec![769]
        );
    }

    #[test]
    fn bot_named_seconds_follow_gameplay_clock_not_driver_rate() {
        for rate in [20, 60, 120, 240] {
            let mut state = crate::state::ServerState {
                config: crate::config::ServerConfig {
                    bot_count: 1,
                    tick_rate_hz: rate,
                    ..crate::config::ServerConfig::default()
                },
                ..crate::state::ServerState::default()
            };
            state.initialize_configured_bots();
            let bot = &mut state.bots[0];
            for start in [10_000, 0x7fff_fff0] {
                let deadline = crate::continuum::continuum_tick_add(start, 200);
                assert!(
                    human_target_is_fresh(Some(start), deadline),
                    "two real seconds must remain fresh at driver rate {rate}"
                );
                assert!(!human_target_is_fresh(
                    Some(start),
                    crate::continuum::continuum_tick_add(deadline, 1),
                ));
                bot.wall_follow_until_tick = None;
                apply_bot_wall_following_impulse(
                    bot,
                    BotCollisionStep {
                        collision_x: true,
                        ..BotCollisionStep::default()
                    },
                    start,
                    None,
                );
                assert_eq!(bot.wall_follow_until_tick, Some(deadline));
            }
            let target = BotTarget {
                player_id: Some(1),
                frequency: Some(1),
                x: 0.0,
                y: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                in_safe_zone: false,
                status: 0,
            };
            let first = combat_orbit_target(bot, target, 0);
            let next = combat_orbit_target(bot, target, 100);
            let delta =
                (next.y.atan2(next.x) - first.y.atan2(first.x)).rem_euclid(std::f32::consts::TAU);
            assert!(
                (delta - BOT_COMBAT_ORBIT_TURNS_PER_SECOND as f32 * std::f32::consts::TAU).abs()
                    < 1e-5
            );
        }
    }

    #[test]
    fn server_bot_human_target_freshness_is_wrap_safe_and_bounded() {
        assert!(human_target_is_fresh(None, 10_000));
        assert!(human_target_is_fresh(Some(9_800), 10_000));
        assert!(!human_target_is_fresh(Some(9_799), 10_000));
        assert!(human_target_is_fresh(Some(0x7fff_ffff - 40), 39));
        assert!(!human_target_is_fresh(Some(0x7fff_ffff - 200), 39));
    }

    #[test]
    fn server_bot_fire_stays_inside_the_visible_encounter_envelope() {
        assert!(bot_target_is_in_fire_range(
            BOT_COMBAT_PURSUIT_DISTANCE,
            410.0,
            200,
        ));
        assert!(!bot_target_is_in_fire_range(
            BOT_COMBAT_PURSUIT_DISTANCE + 1.0,
            410.0,
            200,
        ));
        assert!(!bot_target_is_in_fire_range(300.0, 100.0, 200));
    }

    #[test]
    fn server_bot_aims_at_the_nearest_legal_protocol_heading() {
        // Retained Firefox/Extreme counterfire geometry. The shared wire
        // adapter correctly truncates arbitrary client orientation, but a
        // server-authored bot must choose a legal direction first. Direction
        // 29 is nearer and clears the 0.995 fire gate; direction 28 does not.
        let target_orientation = 0.716_423_4_f32;
        let aimed_orientation = nearest_protocol_orientation(target_orientation);
        assert_eq!(
            crate::continuum_position::protocol_direction(aimed_orientation),
            29
        );
        assert_eq!(aimed_orientation.to_bits(), 0.725_f32.to_bits());

        let (heading_x, heading_y) =
            crate::continuum_position::canonical_discrete_heading_components(aimed_orientation);
        let angle = target_orientation * std::f32::consts::TAU;
        let target_heading_x = angle.sin();
        let target_heading_y = -angle.cos();
        let alignment = heading_x * target_heading_x + heading_y * target_heading_y;
        assert!(alignment > super::BOT_AIM_ALIGNMENT);

        assert_eq!(
            nearest_protocol_orientation(0.99).to_bits(),
            0.0_f32.to_bits()
        );
    }

    #[test]
    fn hostile_bot_closes_the_real_gap_before_selecting_an_orbit_waypoint() {
        let hostile = BotTarget {
            player_id: Some(7),
            frequency: Some(1),
            x: 500.0,
            y: 700.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        };
        assert!(hostile_target_requires_direct_approach(
            hostile,
            BOT_COMBAT_ORBIT_RADIUS + 0.01,
        ));
        assert!(!hostile_target_requires_direct_approach(
            hostile,
            BOT_COMBAT_ORBIT_RADIUS,
        ));
        assert!(!hostile_target_requires_direct_approach(
            BotTarget {
                in_safe_zone: true,
                status: 0,
                ..hostile
            },
            BOT_COMBAT_ORBIT_RADIUS + 100.0,
        ));
        assert!(!hostile_target_requires_direct_approach(
            BotTarget {
                player_id: None,
                ..hostile
            },
            BOT_COMBAT_ORBIT_RADIUS + 100.0,
        ));
    }

    #[test]
    fn server_bot_prize_seek_cannot_defeat_the_no_human_encounter_leash() {
        assert!(!bot_should_seek_prize(false, 1, 1_700));
        assert!(bot_should_seek_prize(true, 1, 1_700));
        assert!(!bot_should_seek_prize(true, 1_700, 1_700));
    }

    #[test]
    fn bot_bullet_intercept_compensates_for_inherited_shooter_and_target_velocity() {
        let mut state = crate::state::ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..crate::state::ServerState::default()
        };
        state.initialize_configured_bots();
        let bot = &mut state.bots[0];
        bot.player.ship = 0;
        bot.weapons.gun_level = 2;
        bot.x = 1_000.0;
        bot.y = 1_000.0;
        bot.velocity_x = 0.0;
        bot.velocity_y = 180.0;
        let crossing_target = BotTarget {
            player_id: Some(7),
            frequency: Some(1),
            x: 1_200.0,
            y: 1_000.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        };

        let compensated_shooter = bot_bullet_intercept_target(bot, crossing_target);
        assert!(compensated_shooter.y < crossing_target.y);

        bot.velocity_y = 0.0;
        let moving_target = BotTarget {
            velocity_y: 180.0,
            ..crossing_target
        };
        let compensated_target = bot_bullet_intercept_target(bot, moving_target);
        assert!(compensated_target.y > moving_target.y);
    }

    #[test]
    fn difficulty_changes_accuracy_without_changing_projectile_physics() {
        use crate::state::BotDifficulty::{Easy, Hard, Medium};
        let (mut state, _) = bot_energy_fixture(0, 1);
        let bot = &mut state.bots[0];
        bot.x = 1000.0;
        bot.y = 1000.0;
        bot.velocity_x = 0.0;
        bot.velocity_y = 0.0;
        let target = BotTarget {
            player_id: Some(7),
            frequency: Some(1),
            x: 1300.0,
            y: 1000.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            in_safe_zone: false,
            status: 0,
        };
        let weapon = bot.gun_weapon_data().unwrap();
        let original_tuning =
            crate::arena_settings::arena_projectile_initialization_tuning(bot.player.ship);
        let mut errors = [0.0_f32; 3];
        for tick in 0..400 {
            bot.last_step_tick = tick;
            for (index, tier) in [Hard, Medium, Easy].into_iter().enumerate() {
                bot.difficulty = tier;
                let aim = super::bot_weapon_intercept_target(bot, target, weapon);
                let angle = (aim.y - bot.y).atan2(aim.x - bot.x).abs().to_degrees();
                assert!(angle <= tier.aim_error_degrees() + 0.001);
                errors[index] += angle;
                assert!(((aim.x - bot.x).hypot(aim.y - bot.y) - 300.0).abs() < 0.01);
            }
        }
        assert_eq!(errors[0], 0.0);
        assert!(errors[1] > 100.0 && errors[2] > errors[1] * 2.0);
        bot.difficulty = Hard;
        let moving = BotTarget {
            velocity_y: 150.0,
            ..target
        };
        let exact = super::bot_exact_weapon_intercept_target(bot, moving, weapon);
        let hard = super::bot_weapon_intercept_target(bot, moving, weapon);
        assert_eq!((hard.x, hard.y), (exact.x, exact.y));
        assert!(exact.y > moving.y);
        for tier in [Medium, Easy] {
            bot.difficulty = tier;
            let aim = super::bot_weapon_intercept_target(bot, moving, weapon);
            let expected = (moving.x + (exact.x - moving.x) * tier.lead_fraction() - bot.x)
                .hypot(moving.y + (exact.y - moving.y) * tier.lead_fraction() - bot.y);
            assert!(((aim.x - bot.x).hypot(aim.y - bot.y) - expected).abs() < 0.01);
        }
        let current_tuning =
            crate::arena_settings::arena_projectile_initialization_tuning(bot.player.ship);
        assert_eq!(original_tuning.bullet_speed, current_tuning.bullet_speed);
    }

    #[test]
    fn difficulty_reaction_delay_resets_on_new_target_and_respawn_across_tick_wrap() {
        use crate::state::BotDifficulty::{Easy, Hard, Medium};
        let (mut state, _) = bot_energy_fixture(0, 1);
        let bot = &mut state.bots[0];
        for tier in [Hard, Medium, Easy] {
            bot.difficulty = tier;
            for start in [100, crate::continuum::CONTINUUM_TICK_MASK - 10] {
                bot.combat_target = None;
                assert_eq!(super::bot_reaction_ready(bot, Some(9), start), tier == Hard);
                let delay = tier.reaction_ticks();
                if delay > 0 {
                    assert!(!super::bot_reaction_ready(
                        bot,
                        Some(9),
                        crate::continuum::continuum_tick_add(start, delay - 1)
                    ));
                }
                let ready = crate::continuum::continuum_tick_add(start, delay);
                assert!(super::bot_reaction_ready(bot, Some(9), ready));
                assert_eq!(
                    super::bot_reaction_ready(bot, Some(10), ready),
                    tier == Hard
                );
                assert!(!super::bot_reaction_ready(bot, None, ready));
                bot.reset_loadout_at_tick(ready);
                assert_eq!(bot.difficulty, tier);
                assert_eq!(bot.combat_target, None);
            }
        }
    }

    #[test]
    fn all_difficulties_obey_ship_cooldowns_and_weapon_energy_costs() {
        use crate::state::BotDifficulty::{Easy, Hard, Medium};
        for ship in 0..8 {
            for tier in [Hard, Medium, Easy] {
                let (mut state, mut fire) = bot_energy_fixture(ship, 1);
                state.bots[0].difficulty = tier;
                let gun = state.bots[0].gun_weapon_data().unwrap();
                fire.weapon_data = gun;
                let cost = crate::arena_settings::arena_ship_weapon_energy_cost(ship, gun);
                let before = state.bots[0].energy;
                spawn_bot_weapon(&mut state, 100, fire);
                assert_eq!(state.bots[0].energy, before - cost);
                let delay = crate::arena_settings::arena_ship_weapon_fire_delay_ticks(
                    ship,
                    (gun & 31) as u8,
                ) * (super::BOT_FIRE_COOLDOWN_MULTIPLIER + tier.cooldown_extra());
                let charged = state.bots[0].energy;
                spawn_bot_weapon(&mut state, 100 + delay - 1, fire);
                assert_eq!(state.bots[0].energy, charged);
                assert_eq!(state.bots[0].last_fire_tick, Some(100));
                spawn_bot_weapon(&mut state, 100 + delay, fire);
                assert_eq!(state.bots[0].energy, charged - cost);
                assert_eq!(state.bots[0].last_fire_tick, Some(100 + delay));
            }
        }
    }
}

#[cfg(test)]
mod configured_ship_lifecycle_tests {
    use super::*;

    #[test]
    fn mixed_ship_bots_keep_ship_frequency_and_authored_loadout_after_respawn() {
        let state = Arc::new(Mutex::new(ServerState::new(crate::config::ServerConfig {
            bot_count: 8,
            bot_ship_types: (0..8).collect(),
            ..crate::config::ServerConfig::default()
        })));
        let names;
        {
            let mut guard = state.lock().unwrap();
            guard.initialize_configured_bots();
            names = guard
                .bots
                .iter()
                .map(|bot| bot.player.name.clone())
                .collect::<Vec<_>>();
            for bot in &mut guard.bots {
                bot.advance_life();
                bot.energy = 0;
                bot.weapons = crate::authority::WeaponInventory::cleared();
                bot.respawn_at_tick = Some(10);
            }
        }
        step_server_bots(&state, 10);
        let guard = state.lock().unwrap();
        for (ship, bot) in guard.bots.iter().enumerate() {
            assert_eq!(bot.player.ship, ship as u8);
            assert_eq!(bot.player.name, names[ship]);
            assert_eq!(bot.player.frequency, bot.player.id);
            assert_eq!(bot.life_seq, 2);
            assert_eq!(bot.respawn_at_tick, None);
            assert_eq!(
                bot.energy,
                crate::arena_settings::arena_ship_initial_energy(ship as u8)
            );
            assert_eq!(
                bot.weapons,
                crate::authority::WeaponInventory::maximized(
                    crate::state::weapon_inventory_tuning(ship as u8)
                )
            );
            assert_eq!(
                bot.rotation_per_second,
                crate::arena_settings::arena_ship_max_rotation_per_second(ship as u8)
            );
            assert_eq!(
                bot.top_speed_pixels_per_second,
                crate::arena_settings::arena_ship_max_speed_pixels_per_second(ship as u8)
            );
        }
    }
}
