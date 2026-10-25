#[path = "arena/lifecycle.rs"]
mod lifecycle;
#[path = "arena/portal_placements.rs"]
mod portal_placements;
#[path = "arena/portal_warps.rs"]
mod portal_warps;
#[path = "arena/positions.rs"]
mod positions;
#[path = "arena/roster_changes.rs"]
mod roster_changes;
#[path = "arena/zones.rs"]
mod zones;

#[cfg(test)]
pub(crate) use zones::select_arena_zone_for_login_in_state;

pub(crate) use lifecycle::disconnect_player_session_after_removal;
#[cfg(test)]
pub(crate) use lifecycle::{disconnect_player, register_connection};
pub(crate) use lifecycle::{
    disconnect_player_session, join_arena_for_login, leave_arena_for_runtime,
    register_connection_with_sender_in_state_at_tick,
};
#[cfg(test)]
pub(crate) use lifecycle::{
    join_arena, join_arena_at_tick, join_arena_at_tick_with_delivery_hooks,
    join_arena_at_tick_with_objective_capture_hook,
    join_arena_for_login_at_tick_with_zone_selection_hook, leave_arena,
    register_connection_with_sender,
};
pub(crate) use portal_placements::{PortalPlacementOutcome, apply_portal_placement_at_tick};
pub(crate) use portal_warps::{PortalWarpOutcome, apply_portal_warp_at_tick};
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use positions::{
    AcceptedPositionRelay, RuntimePositionAdmission, accept_position_relay,
    accept_position_relay_for_life, accept_position_relay_for_life_at_tick,
    accept_position_relay_for_life_in_state_at_tick,
    accept_position_relay_for_runtime_at_tick_outcome, accepted_position_weapon_applies_recoil,
    advance_server_gameplay_energy, advance_server_recharge,
    apply_movement_correction_application_at_tick, begin_input_history_gap_correction,
    broadcast_position, cancelled_movement_correction_receipt,
    cancelled_movement_correction_receipt_for_state, client_position_sequence_advances,
    combat_view_block_cause, frequency_baseline_from_large_position, large_position_coordinates,
    large_position_coordinates_and_velocity, large_position_from_client,
    pending_frequency_baseline_is_canonical, pending_ship_life_reset_position_is_canonical,
    retire_completed_repel_sequence_window, send_position_relays_batched, set_coordinates_packet,
};
#[cfg(test)]
pub(crate) use positions::{
    accept_position_relay_for_runtime_at_tick, broadcast_position_observable,
    preview_and_commit_position_weapon_for_test,
    preview_and_commit_position_weapon_with_ball_policy_for_test,
};
pub(crate) use roster_changes::{
    normalized_ship, update_frequency_for_runtime_instant_at_tick, update_ship_for_runtime_at_tick,
};
#[cfg(test)]
pub(crate) use roster_changes::{
    update_frequency, update_frequency_at_tick, update_frequency_instant_at_tick, update_ship,
    update_ship_at_tick,
};
pub(crate) const ARENA_LOGIN: u8 = 0x01;
/// Byte after the 27-byte Continuum arena login where browsers declare their
/// protocol revision (`ArenaLoginRequest::PROTOCOL_REVISION`).
pub(crate) const ARENA_LOGIN_PROTOCOL_REVISION_OFFSET: usize = 27;
pub(crate) const LEAVE_ARENA_REQUEST: u8 = 0x02;
pub(crate) const CLIENT_POSITION: u8 = 0x03;
/// Server-relayed large position packet type. Matches
/// `protocol::PlayerPositionUpdate::LARGE_TYPE` on the client; a single
/// constant lets writers and packet-type checks share one literal so they
/// can never desync from the parser.
pub(crate) const LARGE_POSITION_PACKET: u8 = 0x05;

#[cfg(test)]
#[test]
fn large_position_packet_matches_canonical_constant() {
    // Server's local constant must match the value used by every writer.
    // The `0x05` literal is the wire-format type byte for a Continuum large
    // position packet — drift here desyncs every server→client position
    // relay from every receiver's parser.
    assert_eq!(LARGE_POSITION_PACKET, 0x05);
}
pub(crate) const FREQUENCY_CHANGE_REQUEST: u8 = 0x0f;
pub(crate) const SHIP_CHANGE_REQUEST: u8 = 0x18;
pub(crate) const SET_COORDINATES: u8 = 0x32;

pub(crate) use roster_changes::force_spawn_idle_spectator_in_state;
#[allow(unused_imports)]
// connection-quality enforcement integration uses this server-only origin
pub(crate) use roster_changes::{RosterChangeOutcome, force_quality_spectator_in_state};
