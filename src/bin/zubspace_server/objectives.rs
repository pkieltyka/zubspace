#[path = "objectives/actions.rs"]
mod actions;
#[path = "objectives/bricks.rs"]
mod bricks;
pub(crate) use bricks::{brick_overlap_destination, queue_enemy_brick_overlap_corrections};
#[path = "objectives/engine.rs"]
mod engine;
#[path = "objectives/flags.rs"]
mod flags;
#[path = "objectives/packets.rs"]
mod packets;
#[path = "objectives/powerballs.rs"]
mod powerballs;
// Flag profiles are now authored by shipped zones, but powerball profiles are
// still constructible only by tests until the exact shipped-zone `[Soccer]`
// values are recovered, so the powerball-side validators appear dead in the
// production-only build even though the engine consumes the validated variants
// and the all-target test build exercises every path.
#[allow(dead_code)]
#[path = "objectives/profile.rs"]
mod profile;
#[path = "objectives/registry.rs"]
mod registry;
#[path = "objectives/soccer.rs"]
mod soccer;
#[path = "objectives/turrets.rs"]
mod turrets;

pub(crate) use actions::{
    legacy_flag_claim_packets, legacy_flag_drop_packets, mutation_packets,
    objective_action_rejection_packets, objective_action_rejection_packets_for_registry,
    objective_participants, objective_player, objective_state_replay_packets,
    player_lifecycle_packets, player_lifecycle_packets_for_context,
    process_reliable_objective_head, relay_objective_action_at_delivery,
    step_server_bot_objectives, sync_player_flag_counts,
};
#[cfg(test)]
pub(crate) use actions::{
    player_lifecycle_packets_for_context_with_tiles, process_pending_objective_actions,
    relay_objective_action,
};
#[cfg(test)]
pub(crate) use bricks::brick_tile_from_ship_position;
pub(crate) use bricks::{
    current_brick_packets_for_state_at_tick, expire_bricks, relay_drop_brick_for_runtime_at_tick,
};
#[cfg(test)]
pub(crate) use bricks::{relay_drop_brick, relay_drop_brick_at_tick};
pub(crate) use engine::{
    FlagDropTile, ObjectiveLifecycleReason, ObjectiveMutation, evaluate_flag_win,
    initialize_objectives, score_powerball_goal, step_objective_deadlines, step_powerball_motion,
};
#[cfg(test)]
pub(crate) use engine::{
    ObjectiveAward, ObjectiveMutationError, ObjectiveParticipant, ObjectivePlayer,
    apply_player_objective_lifecycle, authenticate_objective_action, ball_spawn_wire_position,
    claim_flag, fire_powerball, flag_drop_tile_near, flag_spawn_tile, pickup_powerball,
    powerball_game_winner, simulate_powerball_step,
};
pub(crate) use flags::authoritative_near_flag_timers;
#[cfg(test)]
pub(crate) use flags::current_flag_packets;
pub(crate) use flags::{current_flag_packets_for_state, relay_drop_flags, relay_flag_request};
#[cfg(test)]
pub(crate) use packets::{
    BRICK_DROPPED, FLAG_POSITION, POWERBALL_POSITION, destroy_turret_link, flag_claim_packet,
    flag_position_packet,
};
pub(crate) use packets::{
    clear_bricks_packet, create_turret_link, drop_flag_packet, powerball_removal_packet,
};
pub(crate) use powerballs::{
    current_powerball_packets_for_state, player_carries_powerball, relay_ball_fire,
    relay_ball_goal, relay_ball_pickup,
};
pub(crate) use profile::{
    FlagDropBehavior, FlagPolicy, FlagProfileSettings, FlagRewardSplit, FlagWinMode,
    MAX_SERVER_FLAGS, MAX_WRAP_SAFE_OBJECTIVE_DELAY_TICKS, ObjectiveSpawnArea, ObjectiveTeamLimits,
    ServerFlagProfile, ServerObjectiveProfile, ServerPowerballProfile,
};
#[cfg(test)]
pub(crate) use profile::{
    PowerballPolicy, PowerballProfileSettings, synthetic_flag_policy, synthetic_powerball_policy,
    synthetic_profile,
};

/// Product gate for flag and powerball objectives.
///
/// Flags and powerballs are deferred for this product iteration, so an authored
/// `[Objective]` profile is only activated behind an explicit operator opt-in.
/// With the gate closed even a zone (or operator-supplied dynamic zone) that
/// authors objectives runs the disabled profile; with it open, a zone that
/// authors none still runs none. Kept separate from `ServerState` so the
/// decision is unit-testable without an authored zone.
pub(crate) fn gated_objective_profile(
    enable_objectives: bool,
    authored: Option<&ServerObjectiveProfile>,
) -> ServerObjectiveProfile {
    if enable_objectives {
        authored
            .cloned()
            .unwrap_or_else(ServerObjectiveProfile::disabled)
    } else {
        ServerObjectiveProfile::disabled()
    }
}
#[cfg(test)]
pub(crate) use registry::{FlagIdentity, ObjectiveVersion, PowerballIdentity, PowerballSnapshot};
pub(crate) use registry::{
    FlagState, ObjectiveRegistry, ObjectiveRegistryError, PowerballMotion, PowerballState,
};
pub(crate) use soccer::soccer_goal_counts_world;
pub(crate) use turrets::{
    clear_player_turret_links, current_turret_packets_for_state, relay_attach_request_for_runtime,
    relay_drop_attached_for_runtime, remove_player_turret_link_packets,
};
#[cfg(test)]
pub(crate) use turrets::{current_turret_packets, relay_attach_request, relay_drop_attached};

pub(crate) const ATTACH_REQUEST: u8 = 0x10;
pub(crate) const FLAG_REQUEST: u8 = 0x13;
pub(crate) const DROP_ATTACHED_REQUEST: u8 = 0x14;
pub(crate) const DROP_FLAGS_REQUEST: u8 = 0x15;
pub(crate) const DROP_BRICK_REQUEST: u8 = 0x1c;
pub(crate) const BALL_FIRE_REQUEST: u8 = 0x1f;
pub(crate) const BALL_PICKUP_REQUEST: u8 = 0x20;
pub(crate) const BALL_GOAL_REQUEST: u8 = 0x21;
